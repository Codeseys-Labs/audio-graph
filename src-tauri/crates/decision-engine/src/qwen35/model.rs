//! Qwen3.5 text backbone as a prompt encoder: one forward pass over one prompt, no cache, no LM head.
//! Heads (the d3 code readout, a future Clef-flash schema head) live with their engines and read the
//! final-norm hidden states this module returns.
//!
//! The Gated DeltaNet layers are always causal. The softmax (full) attention layers are bidirectional or
//! causal per [`Attention`]; d3 checkpoints declare `noncausal_full_attention`, and the stock Qwen3.5
//! forward is causal. RoPE positions are `0..L-1` on all three mRoPE sections, which for text reduces to
//! standard rotate-half RoPE on the first `head_dim * partial_rotary_factor` dims.

use super::config::TextConfig;
use super::gdn;
use candle_core::{D, DType, Device, Module, Tensor};
use candle_core::{Result, bail};
use candle_nn::VarBuilder;
use std::path::{Path, PathBuf};

/// How the full-attention layers read the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attention {
    /// Every position reads every prompt position (d3 `noncausal_full_attention`).
    Bidirectional,
    /// Each position reads itself and earlier positions (the stock Qwen3.5 forward).
    Causal,
}

/// Where and how to load the backbone.
#[derive(Debug, Clone)]
pub struct LoadOptions<'a> {
    /// Tensor name prefix of the text model, e.g. `language_model` for `Qwen3_5Model` checkpoints.
    pub prefix: &'a str,
    pub attention: Attention,
    /// Compute dtype of the layers (F32 or BF16). The embedding table stays in its stored dtype on the host.
    pub dtype: DType,
    /// Dtype the projection weights are kept in. Equal to `dtype` keeps one copy in the compute dtype; BF16
    /// with an F32 `dtype` runs FP32 numerics on BF16-stored weights (the checkpoints store BF16, so the
    /// values are identical) at half the memory, upcasting each weight for its matmul.
    pub weights: DType,
}

/// A bias-free projection whose weight may be stored in a narrower dtype than the activations.
struct Linear {
    weight: Tensor,
}

impl Module for Linear {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        if self.weight.dtype() == x.dtype() {
            candle_nn::Linear::new(self.weight.clone(), None).forward(x)
        } else {
            candle_nn::Linear::new(self.weight.to_dtype(x.dtype())?, None).forward(x)
        }
    }
}

fn linear(vb: &VarBuilder, out_dim: usize, in_dim: usize, name: &str) -> Result<Linear> {
    Ok(Linear {
        weight: vb.get((out_dim, in_dim), &format!("{name}.weight"))?,
    })
}

/// Zero-centered RMSNorm (`x * rsqrt(mean(x^2) + eps) * (1 + w)`), as `Qwen3_5RMSNorm`.
struct RmsNorm {
    alpha: Tensor,
    eps: f32,
}

impl RmsNorm {
    fn load(vb: &VarBuilder, size: usize, eps: f64, dtype: DType) -> Result<Self> {
        let w = vb.get(size, "weight")?;
        let alpha = (w.to_dtype(DType::F32)? + 1.0)?.to_dtype(dtype)?;
        Ok(Self {
            alpha,
            eps: eps as f32,
        })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        candle_nn::ops::rms_norm(&x.contiguous()?, &self.alpha, self.eps)
    }
}

struct Mlp {
    gate: Linear,
    up: Linear,
    down: Linear,
}

impl Mlp {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let h = (self.gate.forward(x)?.silu()? * self.up.forward(x)?)?;
        self.down.forward(&h)
    }
}

struct FullAttention {
    q_proj: Linear,
    k_proj: Linear,
    v_proj: Linear,
    o_proj: Linear,
    q_norm: RmsNorm,
    k_norm: RmsNorm,
    heads: usize,
    kv_heads: usize,
    head_dim: usize,
    rotary_dim: usize,
}

impl FullAttention {
    fn rope(&self, x: &Tensor, cos: &Tensor, sin: &Tensor) -> Result<Tensor> {
        // x: (1, heads, L, head_dim); rotate the first rotary_dim dims, pass the rest through.
        let rot = x.narrow(D::Minus1, 0, self.rotary_dim)?.contiguous()?;
        let rot = candle_nn::rotary_emb::rope(&rot, cos, sin)?;
        let pass = x.narrow(D::Minus1, self.rotary_dim, self.head_dim - self.rotary_dim)?;
        Tensor::cat(&[rot, pass], D::Minus1)
    }

    /// `mask`: additive (L, L) causal mask in the compute dtype, or None for bidirectional attention.
    fn forward(
        &self,
        x: &Tensor,
        cos: &Tensor,
        sin: &Tensor,
        mask: Option<&Tensor>,
    ) -> Result<Tensor> {
        let len = x.dim(0)?;
        let (h, kvh, hd) = (self.heads, self.kv_heads, self.head_dim);
        let qg = self.q_proj.forward(x)?.reshape((len, h, 2 * hd))?;
        let q = self.q_norm.forward(&qg.narrow(2, 0, hd)?)?;
        let gate = qg.narrow(2, hd, hd)?.reshape((len, h * hd))?;
        let k = self
            .k_norm
            .forward(&self.k_proj.forward(x)?.reshape((len, kvh, hd))?)?;
        let v = self.v_proj.forward(x)?.reshape((len, kvh, hd))?;
        let q = self.rope(&q.transpose(0, 1)?.unsqueeze(0)?, cos, sin)?;
        let k = self.rope(&k.transpose(0, 1)?.unsqueeze(0)?, cos, sin)?;
        let v = v.transpose(0, 1)?.contiguous()?;
        // GQA without materializing repeated K/V: query heads are grouped per kv head.
        let rep = h / kvh;
        let q = q.reshape((kvh, rep * len, hd))?;
        let k = k.reshape((kvh, len, hd))?;
        let scale = (hd as f64).powf(-0.5);
        let scores = (q.matmul(&k.t()?)? * scale)?;
        let scores = match mask {
            // One prompt and no padding: bidirectional attention needs no mask at all.
            None => scores,
            Some(mask) => scores
                .reshape((kvh, rep, len, len))?
                .broadcast_add(mask)?
                .reshape((kvh, rep * len, len))?,
        };
        let probs = candle_nn::ops::softmax_last_dim(&scores)?;
        let out = probs.matmul(&v)?; // (kvh, rep*L, hd)
        let out = out
            .reshape((h, len, hd))?
            .transpose(0, 1)?
            .reshape((len, h * hd))?;
        let out = (out * candle_nn::ops::sigmoid(&gate)?)?;
        self.o_proj.forward(&out)
    }
}

struct GatedDeltaNet {
    in_proj_qkv: Linear,
    in_proj_z: Linear,
    in_proj_b: Linear,
    in_proj_a: Linear,
    conv_weight: Tensor, // (K, conv_dim)
    neg_a: Tensor,       // -exp(A_log), FP32
    dt_bias: Tensor,     // FP32
    norm_weight: Tensor,
    out_proj: Linear,
    k_heads: usize,
    v_heads: usize,
    dk: usize,
    dv: usize,
    eps: f64,
}

impl GatedDeltaNet {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        let len = x.dim(0)?;
        let dims = gdn::Dims {
            k_heads: self.k_heads,
            v_heads: self.v_heads,
            dk: self.dk,
            dv: self.dv,
        };
        let qkv = self.in_proj_qkv.forward(x)?; // (L, conv_dim)
        let a = self.in_proj_a.forward(x)?;
        let b = self.in_proj_b.forward(x)?;
        let weights = gdn::PrepWeights {
            conv: &self.conv_weight,
            dt_bias: &self.dt_bias,
            neg_a: &self.neg_a,
        };
        let prepared = gdn::prepare(&qkv, &a, &b, &weights, dims)?;
        let core = gdn::recurrence(&prepared)?;
        let z = self.in_proj_z.forward(x)?; // (L, nv*dv)
        let gated = gdn::gated_norm(&core, &z, &self.norm_weight, self.eps)?;
        debug_assert_eq!(gated.dims(), [len, self.v_heads * self.dv]);
        self.out_proj.forward(&gated)
    }
}

enum Mixer {
    Full(FullAttention),
    Linear(GatedDeltaNet),
}

struct Layer {
    input_norm: RmsNorm,
    mixer: Mixer,
    post_norm: RmsNorm,
    mlp: Mlp,
}

pub struct Encoder {
    embed: Tensor, // (vocab, hidden) on CPU, stored dtype: only L rows are gathered per prompt
    layers: Vec<Layer>,
    norm: RmsNorm,
    inv_freq: Vec<f32>,
    attention: Attention,
    dtype: DType,
    device: Device,
}

/// The safetensors files of a checkpoint: the shards of `model.safetensors.index.json`, else
/// `model.safetensors`.
pub fn weight_files(root: &Path) -> Result<Vec<PathBuf>> {
    let index = root.join("model.safetensors.index.json");
    if index.is_file() {
        let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&index)?)
            .map_err(|e| candle_core::Error::Msg(format!("{}: {e}", index.display())))?;
        let mut files: Vec<String> = doc["weight_map"]
            .as_object()
            .ok_or_else(|| {
                candle_core::Error::Msg("model.safetensors.index.json has no weight_map".into())
            })?
            .values()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        files.sort();
        files.dedup();
        Ok(files.into_iter().map(|f| root.join(f)).collect())
    } else {
        Ok(vec![root.join("model.safetensors")])
    }
}

impl Encoder {
    pub fn load(
        root: &Path,
        cfg: &TextConfig,
        options: &LoadOptions,
        device: &Device,
    ) -> Result<Self> {
        let (prefix, dtype) = (options.prefix, options.dtype);
        let files = weight_files(root)?;
        // SAFETY: the checkpoint files are not modified while mapped.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&files, options.weights, device)? }
            .pp(prefix);
        let cpu =
            unsafe { VarBuilder::from_mmaped_safetensors(&files, DType::BF16, &Device::Cpu)? };
        let embed = cpu.get(
            (cfg.vocab_size, cfg.hidden_size),
            &format!("{prefix}.embed_tokens.weight"),
        )?;
        let (hs, eps) = (cfg.hidden_size, cfg.rms_norm_eps);
        let mut layers = Vec::with_capacity(cfg.num_hidden_layers);
        for i in 0..cfg.num_hidden_layers {
            let lb = vb.pp(format!("layers.{i}"));
            let mixer = match cfg.layer_types[i].as_str() {
                "full_attention" => {
                    let ab = lb.pp("self_attn");
                    let (h, kvh, hd) = (
                        cfg.num_attention_heads,
                        cfg.num_key_value_heads,
                        cfg.head_dim,
                    );
                    Mixer::Full(FullAttention {
                        q_proj: linear(&ab, h * hd * 2, hs, "q_proj")?,
                        k_proj: linear(&ab, kvh * hd, hs, "k_proj")?,
                        v_proj: linear(&ab, kvh * hd, hs, "v_proj")?,
                        o_proj: linear(&ab, hs, h * hd, "o_proj")?,
                        q_norm: RmsNorm::load(&ab.pp("q_norm"), hd, eps, dtype)?,
                        k_norm: RmsNorm::load(&ab.pp("k_norm"), hd, eps, dtype)?,
                        heads: h,
                        kv_heads: kvh,
                        head_dim: hd,
                        rotary_dim: cfg.rotary_dim(),
                    })
                }
                "linear_attention" => {
                    let gb = lb.pp("linear_attn");
                    let (nk, nv, dk, dv) = (
                        cfg.linear_num_key_heads,
                        cfg.linear_num_value_heads,
                        cfg.linear_key_head_dim,
                        cfg.linear_value_head_dim,
                    );
                    let conv_dim = 2 * nk * dk + nv * dv;
                    let kernel = cfg.linear_conv_kernel_dim;
                    let conv = gb.get((conv_dim, 1, kernel), "conv1d.weight")?;
                    let a_log =
                        gb.get_with_hints_dtype(nv, "A_log", Default::default(), DType::F32)?;
                    Mixer::Linear(GatedDeltaNet {
                        in_proj_qkv: linear(&gb, conv_dim, hs, "in_proj_qkv")?,
                        in_proj_z: linear(&gb, nv * dv, hs, "in_proj_z")?,
                        in_proj_b: linear(&gb, nv, hs, "in_proj_b")?,
                        in_proj_a: linear(&gb, nv, hs, "in_proj_a")?,
                        conv_weight: conv.squeeze(1)?.t()?.to_dtype(dtype)?.contiguous()?,
                        neg_a: a_log.exp()?.neg()?,
                        dt_bias: gb.get_with_hints_dtype(
                            nv,
                            "dt_bias",
                            Default::default(),
                            DType::F32,
                        )?,
                        norm_weight: gb.get(dv, "norm.weight")?.to_dtype(dtype)?,
                        out_proj: linear(&gb, hs, nv * dv, "out_proj")?,
                        k_heads: nk,
                        v_heads: nv,
                        dk,
                        dv,
                        eps,
                    })
                }
                other => bail!("layer {i}: unsupported layer type {other:?}"),
            };
            let mb = lb.pp("mlp");
            layers.push(Layer {
                input_norm: RmsNorm::load(&lb.pp("input_layernorm"), hs, eps, dtype)?,
                mixer,
                post_norm: RmsNorm::load(&lb.pp("post_attention_layernorm"), hs, eps, dtype)?,
                mlp: Mlp {
                    gate: linear(&mb, cfg.intermediate_size, hs, "gate_proj")?,
                    up: linear(&mb, cfg.intermediate_size, hs, "up_proj")?,
                    down: linear(&mb, hs, cfg.intermediate_size, "down_proj")?,
                },
            });
        }
        let rd = cfg.rotary_dim();
        let theta = cfg.rope_parameters.rope_theta;
        let inv_freq = (0..rd / 2)
            .map(|i| (1.0 / theta.powf((2 * i) as f64 / rd as f64)) as f32)
            .collect();
        Ok(Self {
            embed,
            layers,
            norm: RmsNorm::load(&vb.pp("norm"), hs, eps, dtype)?,
            inv_freq,
            attention: options.attention,
            dtype,
            device: device.clone(),
        })
    }

    /// Final-norm hidden states of every prompt position, FP32, shape (L, hidden).
    pub fn hidden_states(&self, ids: &[u32]) -> Result<Tensor> {
        self.norm
            .forward(&self.layers_forward(ids)?)?
            .to_dtype(DType::F32)
    }

    /// Final-norm hidden state at the last prompt position, FP32, shape (hidden,). Normalizes that row only.
    pub fn last_hidden(&self, ids: &[u32]) -> Result<Tensor> {
        let x = self.layers_forward(ids)?;
        let last = x.narrow(0, ids.len() - 1, 1)?;
        self.norm.forward(&last)?.squeeze(0)?.to_dtype(DType::F32)
    }

    /// The decoder layers over one prompt, before the final norm: (L, hidden) in the compute dtype.
    fn layers_forward(&self, ids: &[u32]) -> Result<Tensor> {
        let len = ids.len();
        if len == 0 {
            bail!("empty prompt");
        }
        let idx = Tensor::new(ids, &Device::Cpu)?;
        let mut x = self
            .embed
            .index_select(&idx, 0)?
            .to_dtype(self.dtype)?
            .to_device(&self.device)?;
        // RoPE tables in FP32, cast to the compute dtype like the reference.
        let freqs: Vec<f32> = (0..len)
            .flat_map(|p| self.inv_freq.iter().map(move |f| p as f32 * f))
            .collect();
        let freqs = Tensor::from_vec(freqs, (len, self.inv_freq.len()), &Device::Cpu)?;
        let cos = freqs.cos()?.to_dtype(self.dtype)?.to_device(&self.device)?;
        let sin = freqs.sin()?.to_dtype(self.dtype)?.to_device(&self.device)?;
        let mask = match self.attention {
            Attention::Bidirectional => None,
            Attention::Causal => {
                let values: Vec<f32> = (0..len)
                    .flat_map(|i| {
                        (0..len).map(move |j| if j > i { f32::NEG_INFINITY } else { 0.0 })
                    })
                    .collect();
                let mask = Tensor::from_vec(values, (len, len), &Device::Cpu)?;
                Some(mask.to_dtype(self.dtype)?.to_device(&self.device)?)
            }
        };
        for layer in &self.layers {
            let h = layer.input_norm.forward(&x)?;
            let h = match &layer.mixer {
                Mixer::Full(a) => a.forward(&h, &cos, &sin, mask.as_ref())?,
                Mixer::Linear(g) => g.forward(&h)?,
            };
            x = (x + h)?;
            let h = layer.mlp.forward(&layer.post_norm.forward(&x)?)?;
            x = (x + h)?;
        }
        Ok(x)
    }
}
