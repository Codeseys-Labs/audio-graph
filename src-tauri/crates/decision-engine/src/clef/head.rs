//! `JointSchemaHead` of `joint_schema_model.py` on candle: two evidence-routing layers that let every
//! option attend to the whole prompt, four transformer decoder layers over the fields (self-attention
//! across questions, cross-attention to the prompt), and a prior + gated joint scorer per option.

use super::format::Encoded;
use candle_core::{D, DType, Device, Module, Result, Tensor, bail};
use candle_nn::{LayerNorm, Linear};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// `joint_head_config.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct HeadConfig {
    pub hidden_size: usize,
    pub width: usize,
    pub routing_layers: usize,
    pub layers: usize,
    pub heads: usize,
    pub feedforward: usize,
}

struct Weights {
    map: HashMap<String, Tensor>,
    dtype: DType,
    device: Device,
}

impl Weights {
    fn get(&self, name: &str) -> Result<Tensor> {
        let Some(t) = self.map.get(name) else {
            bail!("joint_head.safetensors has no {name}");
        };
        t.to_dtype(self.dtype)?.to_device(&self.device)
    }

    fn linear(&self, name: &str, bias: bool) -> Result<Linear> {
        let w = self.get(&format!("{name}.weight"))?;
        let b = if bias {
            Some(self.get(&format!("{name}.bias"))?)
        } else {
            None
        };
        Ok(Linear::new(w, b))
    }

    fn layer_norm(&self, name: &str) -> Result<LayerNorm> {
        // torch.nn.LayerNorm default eps.
        Ok(LayerNorm::new(
            self.get(&format!("{name}.weight"))?,
            self.get(&format!("{name}.bias"))?,
            1e-5,
        ))
    }

    fn mha(&self, name: &str, heads: usize) -> Result<MultiheadAttention> {
        let w = self.get(&format!("{name}.in_proj_weight"))?;
        let b = self.get(&format!("{name}.in_proj_bias"))?;
        let width = w.dim(1)?;
        let part = |i: usize| -> Result<Linear> {
            Ok(Linear::new(
                w.narrow(0, i * width, width)?,
                Some(b.narrow(0, i * width, width)?),
            ))
        };
        Ok(MultiheadAttention {
            q: part(0)?,
            k: part(1)?,
            v: part(2)?,
            out: self.linear(&format!("{name}.out_proj"), true)?,
            heads,
        })
    }

    fn scalar(&self, name: &str) -> Result<Tensor> {
        self.get(name)?.reshape(1)
    }
}

/// `torch.nn.MultiheadAttention` (batch_first, no masks, no dropout) for one sequence.
struct MultiheadAttention {
    q: Linear,
    k: Linear,
    v: Linear,
    out: Linear,
    heads: usize,
}

impl MultiheadAttention {
    /// `query` (Lq, W), `key_value` (Lk, W) -> (Lq, W).
    fn forward(&self, query: &Tensor, key_value: &Tensor) -> Result<Tensor> {
        let (lq, width) = query.dims2()?;
        let lk = key_value.dim(0)?;
        let hd = width / self.heads;
        let split = |t: Tensor, len: usize| -> Result<Tensor> {
            t.reshape((len, self.heads, hd))?
                .transpose(0, 1)?
                .contiguous()
        };
        let q = split(self.q.forward(query)?, lq)?;
        let k = split(self.k.forward(key_value)?, lk)?;
        let v = split(self.v.forward(key_value)?, lk)?;
        let scores = (q.matmul(&k.t()?)? * (hd as f64).powf(-0.5))?;
        let probs = candle_nn::ops::softmax_last_dim(&scores)?;
        let out = probs.matmul(&v)?.transpose(0, 1)?.reshape((lq, width))?;
        self.out.forward(&out)
    }
}

struct FeedForward {
    up: Linear,
    down: Linear,
}

impl FeedForward {
    fn forward(&self, x: &Tensor) -> Result<Tensor> {
        self.down.forward(&self.up.forward(x)?.gelu_erf()?)
    }
}

struct EvidenceRoutingLayer {
    query_norm: LayerNorm,
    memory_norm: LayerNorm,
    attention: MultiheadAttention,
    feedforward_norm: LayerNorm,
    feedforward: FeedForward,
}

impl EvidenceRoutingLayer {
    fn forward(&self, queries: &Tensor, memory: &Tensor) -> Result<Tensor> {
        let routed = self.attention.forward(
            &self.query_norm.forward(queries)?,
            &self.memory_norm.forward(memory)?,
        )?;
        let queries = (queries + routed)?;
        &queries
            + self
                .feedforward
                .forward(&self.feedforward_norm.forward(&queries)?)?
    }
}

/// `torch.nn.TransformerDecoderLayer(norm_first=True, activation="gelu")`.
struct DecoderLayer {
    norm1: LayerNorm,
    self_attn: MultiheadAttention,
    norm2: LayerNorm,
    cross_attn: MultiheadAttention,
    norm3: LayerNorm,
    feedforward: FeedForward,
}

impl DecoderLayer {
    fn forward(&self, x: &Tensor, memory: &Tensor) -> Result<Tensor> {
        let h = self.norm1.forward(x)?;
        let x = (x + self.self_attn.forward(&h, &h)?)?;
        let x = (&x + self.cross_attn.forward(&self.norm2.forward(&x)?, memory)?)?;
        &x + self.feedforward.forward(&self.norm3.forward(&x)?)?
    }
}

pub struct JointSchemaHead {
    hidden_norm: LayerNorm,
    memory_projection: Linear,
    question_projection: Linear,
    option_question_projection: Linear,
    global_projection: Linear,
    option_context_projection: Linear,
    option_lexical_projection: Linear,
    type_embedding: Tensor,
    evidence_layers: Vec<EvidenceRoutingLayer>,
    option_summary_norm: LayerNorm,
    layers: Vec<DecoderLayer>,
    field_norm: LayerNorm,
    option_norm: LayerNorm,
    residual_scorer: (Linear, Linear),
    prior_scale: Tensor,
    joint_scale: Tensor,
    residual_gate: Tensor,
    dtype: DType,
}

/// `values[start..end].mean(0)` accumulated in FP32, like torch's mean of BF16, back in `dtype`.
fn mean_span(values: &Tensor, (start, end): (usize, usize), dtype: DType) -> Result<Tensor> {
    if end <= start {
        bail!("empty span {start}..{end}");
    }
    values
        .narrow(0, start, end - start)?
        .to_dtype(DType::F32)?
        .mean(0)?
        .to_dtype(dtype)
}

/// `torch.nn.functional.normalize` over the last dim (eps 1e-12).
fn normalize(x: &Tensor) -> Result<Tensor> {
    let norm = x
        .to_dtype(DType::F32)?
        .sqr()?
        .sum_keepdim(D::Minus1)?
        .sqrt()?
        .maximum(1e-12)?;
    x.broadcast_div(&norm.to_dtype(x.dtype())?)
}

/// `exp(clamp(scale, max=ln 100))` of a one-element parameter.
fn logit_scale(scale: &Tensor) -> Result<Tensor> {
    scale.minimum(100f64.ln())?.exp()
}

impl JointSchemaHead {
    pub fn load(root: &Path, dtype: DType, device: &Device) -> Result<Self> {
        let config: HeadConfig =
            crate::qwen35::config::read_json(&root.join("joint_head_config.json"))?;
        let weights = Weights {
            map: candle_core::safetensors::load(root.join("joint_head.safetensors"), &Device::Cpu)?,
            dtype,
            device: device.clone(),
        };
        let ff = |prefix: &str, up: &str, down: &str| -> Result<FeedForward> {
            Ok(FeedForward {
                up: weights.linear(&format!("{prefix}.{up}"), true)?,
                down: weights.linear(&format!("{prefix}.{down}"), true)?,
            })
        };
        let evidence_layers = (0..config.routing_layers)
            .map(|i| {
                let p = format!("evidence_layers.{i}");
                Ok(EvidenceRoutingLayer {
                    query_norm: weights.layer_norm(&format!("{p}.query_norm"))?,
                    memory_norm: weights.layer_norm(&format!("{p}.memory_norm"))?,
                    attention: weights.mha(&format!("{p}.attention"), config.heads)?,
                    feedforward_norm: weights.layer_norm(&format!("{p}.feedforward_norm"))?,
                    feedforward: ff(&p, "feedforward.0", "feedforward.3")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let layers = (0..config.layers)
            .map(|i| {
                let p = format!("layers.{i}");
                Ok(DecoderLayer {
                    norm1: weights.layer_norm(&format!("{p}.norm1"))?,
                    self_attn: weights.mha(&format!("{p}.self_attn"), config.heads)?,
                    norm2: weights.layer_norm(&format!("{p}.norm2"))?,
                    cross_attn: weights.mha(&format!("{p}.multihead_attn"), config.heads)?,
                    norm3: weights.layer_norm(&format!("{p}.norm3"))?,
                    feedforward: ff(&p, "linear1", "linear2")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            hidden_norm: weights.layer_norm("hidden_norm")?,
            memory_projection: weights.linear("memory_projection", false)?,
            question_projection: weights.linear("question_projection", false)?,
            option_question_projection: weights.linear("option_question_projection", false)?,
            global_projection: weights.linear("global_projection", false)?,
            option_context_projection: weights.linear("option_context_projection", false)?,
            option_lexical_projection: weights.linear("option_lexical_projection", false)?,
            type_embedding: weights.get("type_embedding.weight")?,
            evidence_layers,
            option_summary_norm: weights.layer_norm("option_summary_norm")?,
            layers,
            field_norm: weights.layer_norm("field_norm")?,
            option_norm: weights.layer_norm("option_norm")?,
            residual_scorer: (
                weights.linear("residual_scorer.0", true)?,
                weights.linear("residual_scorer.3", true)?,
            ),
            prior_scale: weights.scalar("prior_logit_scale")?,
            joint_scale: weights.scalar("joint_logit_scale")?,
            residual_gate: weights.scalar("residual_gate")?,
            dtype,
        })
    }

    /// Option logits per question (prompt order), FP32 on the host.
    ///
    /// `hidden`: final-norm backbone states (L, hidden) on the head's device; `lexical`: per question, the
    /// mean output-embedding row of each option's tokens, (options, hidden) on the head's device.
    pub fn logits(
        &self,
        hidden: &Tensor,
        encoded: &Encoded,
        lexical: &[Tensor],
    ) -> Result<Vec<Vec<f32>>> {
        let dtype = self.dtype;
        let hidden = self.hidden_norm.forward(&hidden.to_dtype(dtype)?)?;
        let len = hidden.dim(0)?;
        let memory = self.memory_projection.forward(&hidden)?;
        let global = hidden.narrow(0, len - 1, 1)?; // (1, H)
        let question_vectors = Tensor::cat(
            &encoded
                .questions
                .iter()
                .map(|q| mean_span(&hidden, q.question_span, dtype)?.unsqueeze(0))
                .collect::<Result<Vec<_>>>()?,
            0,
        )?; // (Q, H)
        let type_ids: Vec<u32> = encoded.questions.iter().map(|q| q.kind.type_id()).collect();
        let types = self
            .type_embedding
            .index_select(&Tensor::new(type_ids.as_slice(), hidden.device())?, 0)?;
        // Lexical option vectors arrive in FP32 (mean of stored rows); the head runs in its own dtype.
        let lexical = lexical
            .iter()
            .map(|l| l.to_dtype(dtype))
            .collect::<Result<Vec<_>>>()?;

        let mut queries = Vec::with_capacity(encoded.questions.len());
        let mut counts = Vec::with_capacity(encoded.questions.len());
        for (i, (question, lexical)) in encoded.questions.iter().zip(&lexical).enumerate() {
            let contexts = Tensor::cat(
                &question
                    .option_spans
                    .iter()
                    .map(|span| mean_span(&hidden, *span, dtype)?.unsqueeze(0))
                    .collect::<Result<Vec<_>>>()?,
                0,
            )?;
            let qv = question_vectors.narrow(0, i, 1)?;
            let query = (self.option_context_projection.forward(&contexts)?
                + self.option_lexical_projection.forward(lexical)?)?
            .broadcast_add(&self.option_question_projection.forward(&qv)?)?;
            counts.push(question.option_spans.len());
            queries.push(query);
        }
        let mut routed = Tensor::cat(&queries, 0)?;
        for layer in &self.evidence_layers {
            routed = layer.forward(&routed, &memory)?;
        }

        let base_fields = self.question_projection.forward(&question_vectors)?;
        let width = base_fields.dim(1)?;
        let mut split = Vec::with_capacity(counts.len());
        let mut summaries = Vec::with_capacity(counts.len());
        let mut offset = 0;
        for (i, count) in counts.iter().enumerate() {
            let options = routed.narrow(0, offset, *count)?;
            offset += count;
            let field = base_fields.narrow(0, i, 1)?; // (1, W)
            let scores = (options.matmul(&field.t()?)? / (width as f64).sqrt())?.squeeze(1)?; // (n,)
            let weights = candle_nn::ops::softmax(&scores, 0)?;
            summaries.push(weights.unsqueeze(0)?.matmul(&options)?); // (1, W)
            split.push(options);
        }
        let mut fields = ((base_fields
            + self
                .option_summary_norm
                .forward(&Tensor::cat(&summaries, 0)?)?)?
        .broadcast_add(&self.global_projection.forward(&global)?)?
            + types)?;
        for layer in &self.layers {
            fields = layer.forward(&fields, &memory)?;
        }
        let fields = self.field_norm.forward(&fields)?;

        let prior_scale = logit_scale(&self.prior_scale)?;
        let joint_scale = logit_scale(&self.joint_scale)?;
        let gate = candle_nn::ops::sigmoid(&self.residual_gate)?;
        let mut out = Vec::with_capacity(counts.len());
        for (i, (routed, lexical)) in split.iter().zip(&lexical).enumerate() {
            let anchor = normalize(&(question_vectors.narrow(0, i, 1)? + &global)?)?; // (1, H)
            let prior = normalize(lexical)?
                .matmul(&anchor.t()?)?
                .squeeze(1)?
                .broadcast_mul(&prior_scale)?;
            let options = self.option_norm.forward(routed)?; // (n, W)
            let field = fields
                .narrow(0, i, 1)?
                .broadcast_as(options.shape())?
                .contiguous()?;
            let cosine = cosine_similarity(&field, &options)?;
            let features = Tensor::cat(
                &[
                    &field,
                    &options,
                    &(&field * &options)?,
                    &(&field - &options)?.abs()?,
                ],
                1,
            )?;
            let (up, down) = &self.residual_scorer;
            let residual = down
                .forward(&up.forward(&features)?.gelu_erf()?)?
                .squeeze(1)?;
            let joint = (cosine.broadcast_mul(&joint_scale)? + residual)?;
            let logits = (prior + joint.broadcast_mul(&gate)?)?;
            out.push(
                logits
                    .to_dtype(DType::F32)?
                    .to_device(&Device::Cpu)?
                    .to_vec1::<f32>()?,
            );
        }
        Ok(out)
    }
}

/// `torch.nn.functional.cosine_similarity(a, b, dim=-1)` (eps 1e-8) row by row.
fn cosine_similarity(a: &Tensor, b: &Tensor) -> Result<Tensor> {
    let dot = (a * b)?.sum(D::Minus1)?;
    let na = a.to_dtype(DType::F32)?.sqr()?.sum(D::Minus1)?;
    let nb = b.to_dtype(DType::F32)?.sqr()?.sum(D::Minus1)?;
    let denom = (na * nb)?.sqrt()?.maximum(1e-8)?;
    (dot.to_dtype(DType::F32)? / denom)?.to_dtype(a.dtype())
}

/// Softmax of one question's logits (FP32, like `.float().softmax(-1)`).
pub fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|l| (l - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.into_iter().map(|e| e / sum).collect()
}
