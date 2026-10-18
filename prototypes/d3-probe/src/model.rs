//! Qwen3.5 text backbone as a d3 prompt encoder: one forward pass over one prompt, no cache, no LM head.
//!
//! Matches `d3_runtime.PromptEncoder` with `attention_mode = noncausal_full_attention`: the softmax
//! (full) attention layers read every prompt position (no causal mask, batch of one so no padding mask);
//! the Gated DeltaNet layers stay causal. RoPE positions are `0..L-1` on all three mRoPE sections, which
//! for text reduces to standard rotate-half RoPE on the first `head_dim * partial_rotary_factor` dims.

use crate::config::TextConfig;
use crate::gdn;
use anyhow::{Context, Result};
use candle_core::{DType, Device, Module, Tensor, D};
use candle_nn::{Linear, VarBuilder};
use std::path::{Path, PathBuf};

fn linear(vb: &VarBuilder, out_dim: usize, in_dim: usize, name: &str) -> Result<Linear> {
    Ok(Linear::new(vb.get((out_dim, in_dim), &format!("{name}.weight"))?, None))
}

/// Zero-centered RMSNorm (`x * rsqrt(mean(x^2) + eps) * (1 + w)`), as `Qwen3_5RMSNorm`.
struct RmsNorm {
    alpha: Tensor,
    eps: f32,
}

impl RmsNorm {
    fn load(vb: &VarBuilder, size: usize, eps: f64) -> Result<Self> {
        let w = vb.get(size, "weight")?;
        let alpha = (w.to_dtype(DType::F32)? + 1.0)?.to_dtype(w.dtype())?;
        Ok(Self { alpha, eps: eps as f32 })
    }

    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        Ok(candle_nn::ops::rms_norm(&x.contiguous()?, &self.alpha, self.eps)?)
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
        Ok(self.down.forward(&h)?)
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
        Ok(Tensor::cat(&[rot, pass], D::Minus1)?)
    }

    fn forward(&self, x: &Tensor, cos: &Tensor, sin: &Tensor) -> Result<Tensor> {
        let len = x.dim(0)?;
        let (h, kvh, hd) = (self.heads, self.kv_heads, self.head_dim);
        let qg = self.q_proj.forward(x)?.reshape((len, h, 2 * hd))?;
        let q = self.q_norm.forward(&qg.narrow(2, 0, hd)?)?;
        let gate = qg.narrow(2, hd, hd)?.reshape((len, h * hd))?;
        let k = self.k_norm.forward(&self.k_proj.forward(x)?.reshape((len, kvh, hd))?)?;
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
        // Bidirectional: no mask at all (one prompt, no padding).
        let probs = candle_nn::ops::softmax_last_dim(&scores)?;
        let out = probs.matmul(&v)?; // (kvh, rep*L, hd)
        let out = out.reshape((h, len, hd))?.transpose(0, 1)?.reshape((len, h * hd))?;
        let out = (out * candle_nn::ops::sigmoid(&gate)?)?;
        Ok(self.o_proj.forward(&out)?)
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
        let dtype = x.dtype();
        let (nk, nv, dk, dv) = (self.k_heads, self.v_heads, self.dk, self.dv);
        let key_dim = nk * dk;
        let qkv = self.in_proj_qkv.forward(x)?; // (L, conv_dim)
        // Depthwise causal conv1d (kernel K, left zero padding) followed by SiLU.
        let kernel = self.conv_weight.dim(0)?;
        let conv_dim = qkv.dim(1)?;
        let padded = Tensor::cat(&[Tensor::zeros((kernel - 1, conv_dim), dtype, x.device())?, qkv], 0)?;
        let mut mixed = padded.narrow(0, 0, len)?.broadcast_mul(&self.conv_weight.get(0)?)?;
        for j in 1..kernel {
            mixed = (mixed + padded.narrow(0, j, len)?.broadcast_mul(&self.conv_weight.get(j)?)?)?;
        }
        let mixed = mixed.silu()?;
        let q = mixed.narrow(1, 0, key_dim)?.reshape((len, nk, dk))?;
        let k = mixed.narrow(1, key_dim, key_dim)?.reshape((len, nk, dk))?;
        let v = mixed.narrow(1, 2 * key_dim, nv * dv)?.reshape((len, nv, dv))?;
        let z = self.in_proj_z.forward(x)?.reshape((len, nv, dv))?;
        let beta = candle_nn::ops::sigmoid(&self.in_proj_b.forward(x)?)?.to_dtype(DType::F32)?;
        let a = self.in_proj_a.forward(x)?.to_dtype(DType::F32)?.broadcast_add(&self.dt_bias)?;
        let g = softplus(&a)?.broadcast_mul(&self.neg_a)?;
        let (q, k) = if nv > nk {
            let rep = nv / nk;
            let expand = |t: Tensor| -> Result<Tensor> {
                Ok(t.unsqueeze(2)?.broadcast_as((len, nk, rep, dk))?.reshape((len, nv, dk))?)
            };
            (expand(q)?, expand(k)?)
        } else {
            (q, k)
        };
        let q = (l2norm(&q.to_dtype(DType::F32)?)? * (dk as f64).powf(-0.5))?;
        let k = l2norm(&k.to_dtype(DType::F32)?)?;
        let v = v.to_dtype(DType::F32)?;
        let core = gdn::gated_delta_rule(&q.contiguous()?, &k.contiguous()?, &v.contiguous()?, &g.contiguous()?, &beta.contiguous()?)?;
        let core = core.to_dtype(dtype)?;
        // Gated RMSNorm over each head's value dim: weight * norm(x) * silu(z) (weight not zero-centered).
        let core32 = core.to_dtype(DType::F32)?;
        let var = core32.sqr()?.mean_keepdim(D::Minus1)?;
        let normed = core32.broadcast_div(&(var + self.eps)?.sqrt()?)?.to_dtype(dtype)?;
        let normed = normed.broadcast_mul(&self.norm_weight)?.to_dtype(DType::F32)?;
        let gated = (normed * z.to_dtype(DType::F32)?.silu()?)?.to_dtype(dtype)?;
        Ok(self.out_proj.forward(&gated.reshape((len, nv * dv))?)?)
    }
}

fn softplus(x: &Tensor) -> Result<Tensor> {
    // max(x, 0) + log(1 + exp(-|x|)): no overflow for large |x|.
    Ok((x.relu()? + ((x.abs()?.neg()?.exp()? + 1.0)?.log()?))?)
}

fn l2norm(x: &Tensor) -> Result<Tensor> {
    let inv = (x.sqr()?.sum_keepdim(D::Minus1)? + 1e-6)?.sqrt()?.recip()?;
    Ok(x.broadcast_mul(&inv)?)
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
    dtype: DType,
    device: Device,
}

pub fn weight_files(root: &Path) -> Result<Vec<PathBuf>> {
    let index = root.join("model.safetensors.index.json");
    if index.is_file() {
        let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&index)?)?;
        let mut files: Vec<String> = doc["weight_map"]
            .as_object()
            .context("weight_map")?
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
    pub fn load(root: &Path, cfg: &TextConfig, dtype: DType, device: &Device) -> Result<Self> {
        let files = weight_files(root)?;
        // SAFETY: the checkpoint files are not modified while mapped.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&files, dtype, device)? }.pp("language_model");
        let cpu = unsafe { VarBuilder::from_mmaped_safetensors(&files, DType::BF16, &Device::Cpu)? };
        let embed = cpu.get((cfg.vocab_size, cfg.hidden_size), "language_model.embed_tokens.weight")?;
        let (hs, eps) = (cfg.hidden_size, cfg.rms_norm_eps);
        let mut layers = Vec::with_capacity(cfg.num_hidden_layers);
        for i in 0..cfg.num_hidden_layers {
            let lb = vb.pp(format!("layers.{i}"));
            let mixer = match cfg.layer_types[i].as_str() {
                "full_attention" => {
                    let ab = lb.pp("self_attn");
                    let (h, kvh, hd) = (cfg.num_attention_heads, cfg.num_key_value_heads, cfg.head_dim);
                    Mixer::Full(FullAttention {
                        q_proj: linear(&ab, h * hd * 2, hs, "q_proj")?,
                        k_proj: linear(&ab, kvh * hd, hs, "k_proj")?,
                        v_proj: linear(&ab, kvh * hd, hs, "v_proj")?,
                        o_proj: linear(&ab, hs, h * hd, "o_proj")?,
                        q_norm: RmsNorm::load(&ab.pp("q_norm"), hd, eps)?,
                        k_norm: RmsNorm::load(&ab.pp("k_norm"), hd, eps)?,
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
                    let a_log = gb.get_with_hints_dtype(nv, "A_log", Default::default(), DType::F32)?;
                    Mixer::Linear(GatedDeltaNet {
                        in_proj_qkv: linear(&gb, conv_dim, hs, "in_proj_qkv")?,
                        in_proj_z: linear(&gb, nv * dv, hs, "in_proj_z")?,
                        in_proj_b: linear(&gb, nv, hs, "in_proj_b")?,
                        in_proj_a: linear(&gb, nv, hs, "in_proj_a")?,
                        conv_weight: conv.squeeze(1)?.t()?.contiguous()?,
                        neg_a: a_log.exp()?.neg()?,
                        dt_bias: gb.get_with_hints_dtype(nv, "dt_bias", Default::default(), DType::F32)?,
                        norm_weight: gb.get(dv, "norm.weight")?,
                        out_proj: linear(&gb, hs, nv * dv, "out_proj")?,
                        k_heads: nk,
                        v_heads: nv,
                        dk,
                        dv,
                        eps,
                    })
                }
                other => anyhow::bail!("layer {i}: unsupported layer type {other:?}"),
            };
            let mb = lb.pp("mlp");
            layers.push(Layer {
                input_norm: RmsNorm::load(&lb.pp("input_layernorm"), hs, eps)?,
                mixer,
                post_norm: RmsNorm::load(&lb.pp("post_attention_layernorm"), hs, eps)?,
                mlp: Mlp {
                    gate: linear(&mb, cfg.intermediate_size, hs, "gate_proj")?,
                    up: linear(&mb, cfg.intermediate_size, hs, "up_proj")?,
                    down: linear(&mb, hs, cfg.intermediate_size, "down_proj")?,
                },
            });
        }
        let rd = cfg.rotary_dim();
        let theta = cfg.rope_parameters.rope_theta;
        let inv_freq = (0..rd / 2).map(|i| (1.0 / theta.powf((2 * i) as f64 / rd as f64)) as f32).collect();
        Ok(Self {
            embed,
            layers,
            norm: RmsNorm::load(&vb.pp("norm"), hs, eps)?,
            inv_freq,
            dtype,
            device: device.clone(),
        })
    }

    /// Final-norm hidden state at the last prompt position, FP32, shape (hidden,).
    pub fn last_hidden(&self, ids: &[u32]) -> Result<Tensor> {
        let len = ids.len();
        let idx = Tensor::new(ids, &Device::Cpu)?;
        let mut x = self.embed.index_select(&idx, 0)?.to_dtype(self.dtype)?.to_device(&self.device)?;
        // RoPE tables in FP32, cast to the compute dtype like the reference.
        let freqs: Vec<f32> = (0..len)
            .flat_map(|p| self.inv_freq.iter().map(move |f| p as f32 * f))
            .collect();
        let freqs = Tensor::from_vec(freqs, (len, self.inv_freq.len()), &Device::Cpu)?;
        let cos = freqs.cos()?.to_dtype(self.dtype)?.to_device(&self.device)?;
        let sin = freqs.sin()?.to_dtype(self.dtype)?.to_device(&self.device)?;
        let profile = std::env::var_os("D3_PROFILE").is_some();
        let mut mark = std::time::Instant::now();
        if profile {
            self.device.synchronize()?;
            eprintln!("  embed+rope {:.3} ms", mark.elapsed().as_secs_f64() * 1e3);
            mark = std::time::Instant::now();
        }
        for (i, layer) in self.layers.iter().enumerate() {
            let h = layer.input_norm.forward(&x)?;
            let h = match &layer.mixer {
                Mixer::Full(a) => a.forward(&h, &cos, &sin)?,
                Mixer::Linear(g) => g.forward(&h)?,
            };
            x = (x + h)?;
            if profile {
                self.device.synchronize()?;
                eprintln!("  layer {i:>2} mixer {:.3} ms", mark.elapsed().as_secs_f64() * 1e3);
                mark = std::time::Instant::now();
            }
            let h = layer.mlp.forward(&layer.post_norm.forward(&x)?)?;
            x = (x + h)?;
            if profile {
                self.device.synchronize()?;
                eprintln!("  layer {i:>2} mlp   {:.3} ms", mark.elapsed().as_secs_f64() * 1e3);
                mark = std::time::Instant::now();
            }
        }
        // Only the last position is read: normalize that row alone.
        let last = x.narrow(0, len - 1, 1)?;
        Ok(self.norm.forward(&last)?.squeeze(0)?.to_dtype(DType::F32)?)
    }
}
