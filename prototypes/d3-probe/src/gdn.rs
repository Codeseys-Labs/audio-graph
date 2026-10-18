//! Gated DeltaNet token mixer pieces that run outside plain matmuls, for one prompt.
//!
//! `prepare` turns the `in_proj_qkv` / `in_proj_a` / `in_proj_b` outputs into the FP32 inputs of the
//! delta rule: depthwise causal conv1d + SiLU (rounded to the compute dtype, as the reference does),
//! q/k L2 normalization, the `1/sqrt(dk)` query scale, k-head repeat-interleave to the v-head count,
//! `beta = sigmoid(b)` and `g = -exp(A_log) * softplus(a + dt_bias)`. `recurrence` is the reference
//! `torch_recurrent_gated_delta_rule` from a zero state:
//!
//! ```text
//! S = S * exp(g_t);  kv = S^T k_t;  delta = (v_t - kv) * beta_t;  S += k_t delta^T;  o_t = S^T q_t
//! ```
//!
//! `gated_norm` is `Qwen3_5RMSNormGated`: `w * norm(x) * silu(z)` per head.
//!
//! CPU and CUDA use fused kernels (one pass, one launch each); other devices use `generic`, the same
//! math in candle ops, which is also the oracle in the tests.

use anyhow::Result;
use candle_core::{DType, Device, Tensor, D};
use rayon::prelude::*;

#[derive(Debug, Clone, Copy)]
pub struct Dims {
    pub k_heads: usize,
    pub v_heads: usize,
    pub dk: usize,
    pub dv: usize,
}

impl Dims {
    fn key_dim(&self) -> usize {
        self.k_heads * self.dk
    }
    fn conv_dim(&self) -> usize {
        2 * self.key_dim() + self.v_heads * self.dv
    }
}

/// Delta-rule inputs, FP32: `qk` (L, H, 2*DK) with q (normalized, scaled) then k (normalized);
/// `v` (L, H, DV); `gb` (L, H, 2) with g then beta. H is the v-head count.
pub struct Prepared {
    pub qk: Tensor,
    pub v: Tensor,
    pub gb: Tensor,
}

/// Weights `prepare` needs: `conv` (K, conv_dim) in the compute dtype; `dt_bias`, `neg_a` (H,) FP32.
pub struct PrepWeights<'a> {
    pub conv: &'a Tensor,
    pub dt_bias: &'a Tensor,
    pub neg_a: &'a Tensor,
}

pub fn prepare(qkv: &Tensor, a: &Tensor, b: &Tensor, w: &PrepWeights, dims: Dims) -> Result<Prepared> {
    match qkv.device() {
        Device::Cpu => cpu::prepare(qkv, a, b, w, dims),
        #[cfg(feature = "cuda")]
        Device::Cuda(_) => cuda::prepare(qkv, a, b, w, dims),
        #[allow(unreachable_patterns)]
        _ => generic::prepare(qkv, a, b, w, dims),
    }
}

/// (L, H, DV) FP32.
pub fn recurrence(p: &Prepared) -> Result<Tensor> {
    match p.qk.device() {
        Device::Cpu => cpu::recurrence(p),
        #[cfg(feature = "cuda")]
        Device::Cuda(_) => cuda::recurrence(p),
        other => anyhow::bail!("no gated delta rule kernel for {other:?}"),
    }
}

/// `core` (L, H, DV) FP32, `z` (L, H*DV) and `weight` (DV) in the compute dtype; returns (L, H*DV) in
/// the compute dtype. The reference rounds `core` to the compute dtype before normalizing.
pub fn gated_norm(core: &Tensor, z: &Tensor, weight: &Tensor, eps: f64) -> Result<Tensor> {
    match core.device() {
        Device::Cpu => cpu::gated_norm(core, z, weight, eps),
        #[cfg(feature = "cuda")]
        Device::Cuda(_) => cuda::gated_norm(core, z, weight, eps),
        #[allow(unreachable_patterns)]
        _ => generic::gated_norm(core, z, weight, eps),
    }
}

/// Round an FP32 value through the compute dtype (identity for FP32).
fn round_to(dtype: DType, x: f32) -> f32 {
    match dtype {
        DType::BF16 => half::bf16::from_f32(x).to_f32(),
        DType::F16 => half::f16::from_f32(x).to_f32(),
        _ => x,
    }
}

fn softplus(x: f32) -> f32 {
    // torch.nn.functional.softplus (beta 1, threshold 20).
    if x > 20.0 { x } else { x.exp().ln_1p() }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn silu(x: f32) -> f32 {
    x * sigmoid(x)
}

pub mod generic {
    use super::*;

    pub fn prepare(qkv: &Tensor, a: &Tensor, b: &Tensor, w: &PrepWeights, dims: Dims) -> Result<Prepared> {
        let len = qkv.dim(0)?;
        let dtype = qkv.dtype();
        let Dims { k_heads: nk, v_heads: nv, dk, dv } = dims;
        let kernel = w.conv.dim(0)?;
        let padded = Tensor::cat(&[Tensor::zeros((kernel - 1, dims.conv_dim()), dtype, qkv.device())?, qkv.clone()], 0)?;
        let mut mixed = padded.narrow(0, 0, len)?.broadcast_mul(&w.conv.get(0)?)?;
        for j in 1..kernel {
            mixed = (mixed + padded.narrow(0, j, len)?.broadcast_mul(&w.conv.get(j)?)?)?;
        }
        let mixed = mixed.silu()?;
        let kd = dims.key_dim();
        let q = mixed.narrow(1, 0, kd)?.reshape((len, nk, dk))?;
        let k = mixed.narrow(1, kd, kd)?.reshape((len, nk, dk))?;
        let v = mixed.narrow(1, 2 * kd, nv * dv)?.reshape((len, nv, dv))?;
        let beta = candle_nn::ops::sigmoid(b)?.to_dtype(DType::F32)?;
        let a = a.to_dtype(DType::F32)?.broadcast_add(w.dt_bias)?;
        let sp = (a.relu()? + ((a.abs()?.neg()?.exp()? + 1.0)?.log()?))?;
        let g = sp.broadcast_mul(w.neg_a)?;
        let expand = |t: Tensor| -> Result<Tensor> {
            if nv == nk {
                return Ok(t);
            }
            Ok(t.unsqueeze(2)?.broadcast_as((len, nk, nv / nk, dk))?.reshape((len, nv, dk))?)
        };
        let l2 = |x: Tensor| -> Result<Tensor> {
            let inv = (x.sqr()?.sum_keepdim(D::Minus1)? + 1e-6)?.sqrt()?.recip()?;
            Ok(x.broadcast_mul(&inv)?)
        };
        let q = (l2(expand(q)?.to_dtype(DType::F32)?)? * (dk as f64).powf(-0.5))?;
        let k = l2(expand(k)?.to_dtype(DType::F32)?)?;
        Ok(Prepared {
            qk: Tensor::cat(&[q, k], 2)?.contiguous()?,
            v: v.to_dtype(DType::F32)?.contiguous()?,
            gb: Tensor::stack(&[g, beta], 2)?.contiguous()?,
        })
    }

    pub fn gated_norm(core: &Tensor, z: &Tensor, weight: &Tensor, eps: f64) -> Result<Tensor> {
        let (len, heads, dv) = core.dims3()?;
        let dtype = z.dtype();
        let x = core.to_dtype(dtype)?.to_dtype(DType::F32)?;
        let var = x.sqr()?.mean_keepdim(D::Minus1)?;
        let normed = x.broadcast_div(&(var + eps)?.sqrt()?)?.to_dtype(dtype)?;
        let normed = normed.broadcast_mul(weight)?.to_dtype(DType::F32)?;
        let z = z.reshape((len, heads, dv))?.to_dtype(DType::F32)?;
        Ok((normed * z.silu()?)?.to_dtype(dtype)?.reshape((len, heads * dv))?)
    }
}

mod cpu {
    use super::*;

    fn host(t: &Tensor) -> Result<Vec<f32>> {
        Ok(t.to_dtype(DType::F32)?.flatten_all()?.to_vec1::<f32>()?)
    }

    pub fn prepare(qkv: &Tensor, a: &Tensor, b: &Tensor, w: &PrepWeights, dims: Dims) -> Result<Prepared> {
        let len = qkv.dim(0)?;
        let dtype = qkv.dtype();
        let Dims { k_heads: nk, v_heads: nv, dk, dv } = dims;
        let (cd, kd) = (dims.conv_dim(), dims.key_dim());
        let kernel = w.conv.dim(0)?;
        let (x, conv) = (host(qkv)?, host(w.conv)?);
        let (a, b, dt_bias, neg_a) = (host(a)?, host(b)?, host(w.dt_bias)?, host(w.neg_a)?);
        let rep = nv / nk;
        let scale = (dk as f32).powf(-0.5);
        let mut qk = vec![0f32; len * nv * 2 * dk];
        let mut v = vec![0f32; len * nv * dv];
        let mut gb = vec![0f32; len * nv * 2];
        qk.par_chunks_mut(nv * 2 * dk)
            .zip(v.par_chunks_mut(nv * dv))
            .zip(gb.par_chunks_mut(nv * 2))
            .enumerate()
            .for_each(|(t, ((qk_t, v_t), gb_t))| {
                // Causal depthwise conv over the last `kernel` rows, then SiLU, rounded like the reference.
                let mut mixed = vec![0f32; cd];
                for (c, m) in mixed.iter_mut().enumerate() {
                    let mut acc = 0f32;
                    for j in 0..kernel {
                        let src = t as isize + j as isize - (kernel as isize - 1);
                        if src >= 0 {
                            acc += x[src as usize * cd + c] * conv[j * cd + c];
                        }
                    }
                    *m = round_to(dtype, silu(round_to(dtype, acc)));
                }
                for h in 0..nv {
                    let kh = h / rep;
                    for (offset, out_offset, factor) in [(0, 0, scale), (kd, dk, 1.0)] {
                        let src = &mixed[offset + kh * dk..][..dk];
                        let inv = 1.0 / (src.iter().map(|x| x * x).sum::<f32>() + 1e-6).sqrt();
                        let dst = &mut qk_t[h * 2 * dk + out_offset..][..dk];
                        for (d, s) in dst.iter_mut().zip(src) {
                            *d = s * inv * factor;
                        }
                    }
                    v_t[h * dv..][..dv].copy_from_slice(&mixed[2 * kd + h * dv..][..dv]);
                    let av = a[t * nv + h] + dt_bias[h];
                    gb_t[2 * h] = neg_a[h] * softplus(av);
                    gb_t[2 * h + 1] = round_to(dtype, sigmoid(b[t * nv + h]));
                }
            });
        let dev = &Device::Cpu;
        Ok(Prepared {
            qk: Tensor::from_vec(qk, (len, nv, 2 * dk), dev)?,
            v: Tensor::from_vec(v, (len, nv, dv), dev)?,
            gb: Tensor::from_vec(gb, (len, nv, 2), dev)?,
        })
    }

    pub fn recurrence(p: &Prepared) -> Result<Tensor> {
        let (len, heads, dk2) = p.qk.dims3()?;
        let dk = dk2 / 2;
        let dv = p.v.dim(2)?;
        let (qk, v, gb) = (host(&p.qk)?, host(&p.v)?, host(&p.gb)?);
        // One task per head; each owns its (DK, DV) state and writes a (L, DV) slab.
        let per_head: Vec<Vec<f32>> = (0..heads)
            .into_par_iter()
            .map(|h| {
                let mut state = vec![0f32; dk * dv];
                let mut kv = vec![0f32; dv];
                let mut out = vec![0f32; len * dv];
                for t in 0..len {
                    let row = &qk[(t * heads + h) * 2 * dk..][..2 * dk];
                    let (qt, kt) = row.split_at(dk);
                    let vt = &v[(t * heads + h) * dv..][..dv];
                    let decay = gb[(t * heads + h) * 2].exp();
                    let beta = gb[(t * heads + h) * 2 + 1];
                    kv.fill(0.0);
                    for i in 0..dk {
                        let srow = &mut state[i * dv..][..dv];
                        let ki = kt[i];
                        for (s, acc) in srow.iter_mut().zip(kv.iter_mut()) {
                            *s *= decay;
                            *acc += *s * ki;
                        }
                    }
                    for (acc, vj) in kv.iter_mut().zip(vt) {
                        *acc = (vj - *acc) * beta; // kv now holds delta
                    }
                    let ot = &mut out[t * dv..][..dv];
                    for i in 0..dk {
                        let srow = &mut state[i * dv..][..dv];
                        let (ki, qi) = (kt[i], qt[i]);
                        for ((s, d), o) in srow.iter_mut().zip(&kv).zip(ot.iter_mut()) {
                            *s += ki * d;
                            *o += *s * qi;
                        }
                    }
                }
                out
            })
            .collect();
        let mut out = vec![0f32; len * heads * dv];
        for (h, slab) in per_head.iter().enumerate() {
            for t in 0..len {
                out[(t * heads + h) * dv..][..dv].copy_from_slice(&slab[t * dv..][..dv]);
            }
        }
        Ok(Tensor::from_vec(out, (len, heads, dv), &Device::Cpu)?)
    }

    pub fn gated_norm(core: &Tensor, z: &Tensor, weight: &Tensor, eps: f64) -> Result<Tensor> {
        let (len, heads, dv) = core.dims3()?;
        let dtype = z.dtype();
        let (x, zv, w) = (host(core)?, host(z)?, host(weight)?);
        let mut out = vec![0f32; len * heads * dv];
        out.par_chunks_mut(dv).enumerate().for_each(|(row, o)| {
            let xs: Vec<f32> = x[row * dv..][..dv].iter().map(|v| round_to(dtype, *v)).collect();
            let var = xs.iter().map(|v| v * v).sum::<f32>() / dv as f32;
            let inv = 1.0 / (var + eps as f32).sqrt();
            for j in 0..dv {
                let n = round_to(dtype, round_to(dtype, xs[j] * inv) * w[j]);
                o[j] = round_to(dtype, n * silu(zv[row * dv + j]));
            }
        });
        Ok(Tensor::from_vec(out, (len, heads * dv), &Device::Cpu)?.to_dtype(dtype)?)
    }
}

#[cfg(feature = "cuda")]
mod cuda {
    use super::{Dims, PrepWeights, Prepared};
    use anyhow::Result;
    use candle_core::cuda::cudarc::driver::{CudaSlice, CudaView, LaunchConfig, PushKernelArg};
    use candle_core::cuda::cudarc::nvrtc::{compile_ptx_with_opts, CompileOptions};
    use candle_core::cuda::{CudaDevice, WrapErr};
    use candle_core::{CudaStorage, DType, Layout, Shape, Storage, Tensor};
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    /// `T` is the compute dtype: `float`, or `unsigned short` holding BF16 bits (no cuda_bf16.h, so no
    /// CUDA include path is needed at run time).
    const SOURCE: &str = r#"
#if IS_BF16
typedef unsigned short T;
__device__ __forceinline__ float ld(T x) { return __uint_as_float(((unsigned)x) << 16); }
__device__ __forceinline__ T st(float f) {
    unsigned u = __float_as_uint(f);
    if ((u & 0x7f800000u) == 0x7f800000u && (u & 0x7fffffu)) return (T)((u >> 16) | 0x40);
    u += 0x7fffu + ((u >> 16) & 1u);
    return (T)(u >> 16);
}
#else
typedef float T;
__device__ __forceinline__ float ld(T x) { return x; }
__device__ __forceinline__ T st(float f) { return f; }
#endif
__device__ __forceinline__ float rnd(float f) { return ld(st(f)); }
__device__ __forceinline__ float sigm(float x) { return 1.f / (1.f + expf(-x)); }

__device__ float block_sum(float v, float* scratch) {
    for (int m = 16; m > 0; m >>= 1) v += __shfl_xor_sync(0xffffffff, v, m);
    const int warp = threadIdx.x / 32, lane = threadIdx.x % 32;
    __syncthreads();
    if (lane == 0) scratch[warp] = v;
    __syncthreads();
    float total = 0.f;
    for (int w = 0; w < (int)(blockDim.x / 32); ++w) total += scratch[w];
    return total;
}

// Causal depthwise conv1d + SiLU over (L, C), rounded through T like the reference. One thread per element.
extern "C" __global__ void gdn_conv_silu(const T* __restrict__ x, const T* __restrict__ w, T* __restrict__ out,
                                         int len, int channels, int kernel) {
    const long i = (long)blockIdx.x * blockDim.x + threadIdx.x;
    if (i >= (long)len * channels) return;
    const int t = (int)(i / channels), c = (int)(i % channels);
    float acc = 0.f;
    for (int j = 0; j < kernel; ++j) {
        const int src = t + j - (kernel - 1);
        if (src >= 0) acc += ld(x[(long)src * channels + c]) * ld(w[(long)j * channels + c]);
    }
    const float a = rnd(acc);
    out[i] = st(a * sigm(a));
}

// One block (DK threads) per (token, v-head): normalized/scaled q, normalized k, v, g, beta.
extern "C" __global__ void gdn_qkv_gates(const T* __restrict__ mixed, const T* __restrict__ a, const T* __restrict__ b,
                                         const float* __restrict__ dt_bias, const float* __restrict__ neg_a,
                                         float* __restrict__ qk, float* __restrict__ v, float* __restrict__ gb,
                                         int nk, int nv, int dk, int dv) {
    __shared__ float scratch[32];
    const int t = blockIdx.x, h = blockIdx.y, i = threadIdx.x;
    const int kh = h / (nv / nk), kd = nk * dk, cd = 2 * kd + nv * dv;
    const T* row = mixed + (long)t * cd;
    const float q = ld(row[kh * dk + i]);
    const float k = ld(row[kd + kh * dk + i]);
    const float qn = block_sum(q * q, scratch);
    const float kn = block_sum(k * k, scratch);
    const long th = (long)t * nv + h;
    qk[th * 2 * dk + i] = q * rsqrtf(qn + 1e-6f) * rsqrtf((float)dk);
    qk[th * 2 * dk + dk + i] = k * rsqrtf(kn + 1e-6f);
    for (int j = i; j < dv; j += blockDim.x) v[th * dv + j] = ld(row[2 * kd + h * dv + j]);
    if (i == 0) {
        const float x = ld(a[th]) + dt_bias[h];
        const float sp = x > 20.f ? x : log1pf(expf(x));
        gb[th * 2] = neg_a[h] * sp;
        gb[th * 2 + 1] = rnd(sigm(ld(b[th])));
    }
}

// Gated RMSNorm, one block (DV threads) per (token, head): w * norm(rnd(core)) * silu(z).
extern "C" __global__ void gdn_gated_norm(const float* __restrict__ core, const T* __restrict__ z, const T* __restrict__ w,
                                          T* __restrict__ out, int dv, float eps) {
    __shared__ float scratch[32];
    const long row = blockIdx.x;
    const int j = threadIdx.x;
    const float x = rnd(core[row * dv + j]);
    const float var = block_sum(x * x, scratch) / (float)dv;
    const float n = rnd(rnd(x * rsqrtf(var + eps)) * ld(w[j]));
    const float zz = ld(z[row * dv + j]);
    out[row * dv + j] = st(n * zz * sigm(zz));
}

#define G 4
#define R (DK / G)
// Delta rule. One block per (32 value columns, head); G lanes per column split the DK state rows
// (rows r*G + lane: distinct shared-memory banks) and combine dot products with warp shuffles. The next
// token's q/k row is prefetched into the other half of a double buffer.
extern "C" __global__ void gdn_recurrence(const float* __restrict__ qk, const float* __restrict__ v,
                                          const float* __restrict__ gb, float* __restrict__ out,
                                          int len, int heads, int dv) {
    const int h = blockIdx.y;
    const int lane = threadIdx.x % G;
    const int j = blockIdx.x * (blockDim.x / G) + threadIdx.x / G;
    __shared__ float buf[2][2 * DK];
    float s[R];
    #pragma unroll
    for (int r = 0; r < R; ++r) s[r] = 0.f;
    for (int i = threadIdx.x; i < 2 * DK; i += blockDim.x) buf[0][i] = qk[(long)h * 2 * DK + i];
    __syncthreads();
    for (int t = 0; t < len; ++t) {
        const int p = t & 1;
        if (t + 1 < len) {
            const float* next = qk + ((long)(t + 1) * heads + h) * 2 * DK;
            for (int i = threadIdx.x; i < 2 * DK; i += blockDim.x) buf[p ^ 1][i] = next[i];
        }
        const float* qs = buf[p];
        const float* ks = buf[p] + DK;
        const long th = (long)t * heads + h;
        const float decay = expf(gb[th * 2]);
        const float beta = gb[th * 2 + 1];
        const float vj = j < dv ? v[th * dv + j] : 0.f;
        float kv = 0.f;
        #pragma unroll
        for (int r = 0; r < R; ++r) { s[r] *= decay; kv = fmaf(s[r], ks[r * G + lane], kv); }
        #pragma unroll
        for (int m = 1; m < G; m <<= 1) kv += __shfl_xor_sync(0xffffffff, kv, m);
        const float delta = (vj - kv) * beta;
        float o = 0.f;
        #pragma unroll
        for (int r = 0; r < R; ++r) { s[r] = fmaf(ks[r * G + lane], delta, s[r]); o = fmaf(s[r], qs[r * G + lane], o); }
        #pragma unroll
        for (int m = 1; m < G; m <<= 1) o += __shfl_xor_sync(0xffffffff, o, m);
        if (lane == 0 && j < dv) out[th * dv + j] = o;
        __syncthreads();
    }
}
"#;

    const RECURRENCE_BLOCK: u32 = 128;
    const COLUMNS_PER_BLOCK: u32 = RECURRENCE_BLOCK / 4;

    /// PTX and module name for a (DK, compute dtype) variant, compiled once per process with NVRTC.
    fn ptx_for(dk: usize, dtype: DType) -> Result<(String, String)> {
        static PTX: LazyLock<Mutex<HashMap<(usize, bool), String>>> = LazyLock::new(Default::default);
        let bf16 = match dtype {
            DType::F32 => false,
            DType::BF16 => true,
            other => anyhow::bail!("gated deltanet kernels support f32 and bf16, not {other:?}"),
        };
        let mut cache = PTX.lock().expect("ptx cache");
        let ptx = match cache.get(&(dk, bf16)) {
            Some(p) => p.clone(),
            None => {
                let opts = CompileOptions {
                    options: vec![format!("-DDK={dk}"), format!("-DIS_BF16={}", u8::from(bf16))],
                    ..Default::default()
                };
                let ptx = compile_ptx_with_opts(SOURCE, opts)?.to_src();
                cache.insert((dk, bf16), ptx.clone());
                ptx
            }
        };
        Ok((ptx, format!("d3_gdn_dk{dk}_{}", if bf16 { "bf16" } else { "f32" })))
    }

    /// candle's `CudaFunc` is not nameable outside candle, so the lookup is a macro.
    macro_rules! module {
        ($dev:expr, $dk:expr, $dtype:expr, $name:expr) => {{
            let (ptx, module_name) = ptx_for($dk, $dtype)?;
            $dev.get_or_load_custom_func($name, &module_name, &ptx)?
        }};
    }

    fn cuda_dev(t: &Tensor) -> Result<CudaDevice> {
        Ok(t.device().as_cuda_device()?.clone())
    }

    /// Raw device view of a contiguous tensor.
    fn with_view<T: candle_core::cuda_backend::CudaDType, R>(
        t: &Tensor,
        f: impl FnOnce(CudaView<'_, T>) -> Result<R>,
    ) -> Result<R> {
        let (storage, layout): (std::sync::RwLockReadGuard<'_, Storage>, &Layout) = t.storage_and_layout();
        let Storage::Cuda(cs) = &*storage else { anyhow::bail!("expected a cuda tensor") };
        let (start, end) = layout.contiguous_offsets().ok_or_else(|| anyhow::anyhow!("tensor not contiguous"))?;
        f(cs.as_cuda_slice::<T>()?.slice(start..end))
    }

    fn wrap<T: candle_core::cuda_backend::CudaDType>(slice: CudaSlice<T>, dev: &CudaDevice, shape: impl Into<Shape>) -> Tensor {
        let storage = Storage::Cuda(CudaStorage::wrap_cuda_slice(slice, dev.clone()));
        Tensor::from_storage(storage, shape, candle_core::op::BackpropOp::none(), false)
    }

    fn launch_1d(n: usize, block: u32) -> LaunchConfig {
        LaunchConfig { grid_dim: ((n as u32).div_ceil(block), 1, 1), block_dim: (block, 1, 1), shared_mem_bytes: 0 }
    }

    pub fn prepare(qkv: &Tensor, a: &Tensor, b: &Tensor, w: &PrepWeights, dims: Dims) -> Result<Prepared> {
        let dev = cuda_dev(qkv)?;
        let dtype = qkv.dtype();
        let len = qkv.dim(0)?;
        let Dims { k_heads: nk, v_heads: nv, dk, dv } = dims;
        let cd = dims.conv_dim();
        let kernel = w.conv.dim(0)?;
        let (qkv, conv, a, b) = (qkv.contiguous()?, w.conv.contiguous()?, a.contiguous()?, b.contiguous()?);
        let (dt_bias, neg_a) = (w.dt_bias.contiguous()?, w.neg_a.contiguous()?);
        macro_rules! run {
            ($t:ty) => {{
                let mixed = unsafe { dev.alloc::<$t>(len * cd)? };
                let conv_fn = module!(dev, dk, dtype, "gdn_conv_silu");
                with_view::<$t, _>(&qkv, |x| {
                    with_view::<$t, _>(&conv, |cw| {
                        let (l, c, k) = (len as i32, cd as i32, kernel as i32);
                        let mut bld = conv_fn.builder();
                        bld.arg(&x).arg(&cw).arg(&mixed).arg(&l).arg(&c).arg(&k);
                        unsafe { bld.launch(launch_1d(len * cd, 256)) }.w()?;
                        Ok(())
                    })
                })?;
                let qk = unsafe { dev.alloc::<f32>(len * nv * 2 * dk)? };
                let v = unsafe { dev.alloc::<f32>(len * nv * dv)? };
                let gb = unsafe { dev.alloc::<f32>(len * nv * 2)? };
                let gate_fn = module!(dev, dk, dtype, "gdn_qkv_gates");
                with_view::<$t, _>(&a, |av| {
                    with_view::<$t, _>(&b, |bv| {
                        with_view::<f32, _>(&dt_bias, |dtb| {
                            with_view::<f32, _>(&neg_a, |na| {
                                let (nk_, nv_, dk_, dv_) = (nk as i32, nv as i32, dk as i32, dv as i32);
                                let mut bld = gate_fn.builder();
                                bld.arg(&mixed).arg(&av).arg(&bv).arg(&dtb).arg(&na);
                                bld.arg(&qk).arg(&v).arg(&gb).arg(&nk_).arg(&nv_).arg(&dk_).arg(&dv_);
                                let cfg = LaunchConfig {
                                    grid_dim: (len as u32, nv as u32, 1),
                                    block_dim: (dk as u32, 1, 1),
                                    shared_mem_bytes: 0,
                                };
                                unsafe { bld.launch(cfg) }.w()?;
                                Ok(())
                            })
                        })
                    })
                })?;
                Prepared {
                    qk: wrap(qk, &dev, (len, nv, 2 * dk)),
                    v: wrap(v, &dev, (len, nv, dv)),
                    gb: wrap(gb, &dev, (len, nv, 2)),
                }
            }};
        }
        Ok(match dtype {
            DType::F32 => run!(f32),
            DType::BF16 => run!(half::bf16),
            other => anyhow::bail!("unsupported compute dtype {other:?}"),
        })
    }

    pub fn recurrence(p: &Prepared) -> Result<Tensor> {
        let dev = cuda_dev(&p.qk)?;
        let (len, heads, dk2) = p.qk.dims3()?;
        let dk = dk2 / 2;
        let dv = p.v.dim(2)?;
        let func = module!(dev, dk, DType::F32, "gdn_recurrence");
        let out = unsafe { dev.alloc::<f32>(len * heads * dv)? };
        with_view::<f32, _>(&p.qk, |qk| {
            with_view::<f32, _>(&p.v, |v| {
                with_view::<f32, _>(&p.gb, |gb| {
                    let (l, h, d) = (len as i32, heads as i32, dv as i32);
                    let mut bld = func.builder();
                    bld.arg(&qk).arg(&v).arg(&gb).arg(&out).arg(&l).arg(&h).arg(&d);
                    let cfg = LaunchConfig {
                        grid_dim: ((dv as u32).div_ceil(COLUMNS_PER_BLOCK), heads as u32, 1),
                        block_dim: (RECURRENCE_BLOCK, 1, 1),
                        shared_mem_bytes: 0,
                    };
                    unsafe { bld.launch(cfg) }.w()?;
                    Ok(())
                })
            })
        })?;
        Ok(wrap(out, &dev, (len, heads, dv)))
    }

    pub fn gated_norm(core: &Tensor, z: &Tensor, weight: &Tensor, eps: f64) -> Result<Tensor> {
        let dev = cuda_dev(core)?;
        let (len, heads, dv) = core.dims3()?;
        let dtype = z.dtype();
        let (core, z, weight) = (core.contiguous()?, z.contiguous()?, weight.contiguous()?);
        let func = module!(dev, 128, dtype, "gdn_gated_norm");
        macro_rules! run {
            ($t:ty) => {{
                let out = unsafe { dev.alloc::<$t>(len * heads * dv)? };
                with_view::<f32, _>(&core, |c| {
                    with_view::<$t, _>(&z, |zz| {
                        with_view::<$t, _>(&weight, |w| {
                            let (d, e) = (dv as i32, eps as f32);
                            let mut bld = func.builder();
                            bld.arg(&c).arg(&zz).arg(&w).arg(&out).arg(&d).arg(&e);
                            let cfg = LaunchConfig {
                                grid_dim: ((len * heads) as u32, 1, 1),
                                block_dim: (dv as u32, 1, 1),
                                shared_mem_bytes: 0,
                            };
                            unsafe { bld.launch(cfg) }.w()?;
                            Ok(())
                        })
                    })
                })?;
                wrap(out, &dev, (len, heads * dv))
            }};
        }
        Ok(match dtype {
            DType::F32 => run!(f32),
            DType::BF16 => run!(half::bf16),
            other => anyhow::bail!("unsupported compute dtype {other:?}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn max_diff(a: &Tensor, b: &Tensor) -> Result<f32> {
        Ok((a.to_dtype(DType::F32)? - b.to_dtype(DType::F32)?)?.abs()?.flatten_all()?.max(0)?.to_scalar::<f32>()?)
    }

    fn random_prep(dev: &Device, dtype: DType, dims: Dims, len: usize) -> Result<(Tensor, Tensor, Tensor, Tensor, Tensor, Tensor)> {
        let qkv = Tensor::randn(0f32, 1., (len, dims.conv_dim()), dev)?.to_dtype(dtype)?;
        let a = Tensor::randn(0f32, 1., (len, dims.v_heads), dev)?.to_dtype(dtype)?;
        let b = Tensor::randn(0f32, 1., (len, dims.v_heads), dev)?.to_dtype(dtype)?;
        let conv = Tensor::randn(0f32, 0.5, (4, dims.conv_dim()), dev)?.to_dtype(dtype)?;
        let dt_bias = Tensor::randn(0f32, 1., dims.v_heads, dev)?;
        let neg_a = (Tensor::rand(0.01f32, 4., dims.v_heads, dev)? * -1.0)?;
        Ok((qkv, a, b, conv, dt_bias, neg_a))
    }

    /// The fused CPU kernels against the candle-op oracle, with k heads repeated (mini's 16/32 layout).
    #[test]
    fn cpu_fused_matches_generic() -> Result<()> {
        let dims = Dims { k_heads: 2, v_heads: 4, dk: 16, dv: 8 };
        let dev = Device::Cpu;
        for dtype in [DType::F32, DType::BF16] {
            let (qkv, a, b, conv, dt_bias, neg_a) = random_prep(&dev, dtype, dims, 11)?;
            let w = PrepWeights { conv: &conv, dt_bias: &dt_bias, neg_a: &neg_a };
            let fused = cpu::prepare(&qkv, &a, &b, &w, dims)?;
            let oracle = generic::prepare(&qkv, &a, &b, &w, dims)?;
            // The generic path rounds every BF16 conv product and sum; the fused path accumulates the conv
            // in FP32 and rounds once, like the reference's conv1d. Only FP32 is expected to agree tightly.
            let tol = if dtype == DType::F32 { 1e-5 } else { 5e-2 };
            assert!(max_diff(&fused.qk, &oracle.qk)? < tol, "{dtype:?} qk");
            assert!(max_diff(&fused.v, &oracle.v)? < tol, "{dtype:?} v");
            assert!(max_diff(&fused.gb, &oracle.gb)? < tol, "{dtype:?} gb");
            let core = Tensor::randn(0f32, 1., (11, 4, 8), &dev)?;
            let z = Tensor::randn(0f32, 1., (11, 32), &dev)?.to_dtype(dtype)?;
            let weight = Tensor::randn(0f32, 1., 8, &dev)?.to_dtype(dtype)?;
            let tol = if dtype == DType::F32 { 1e-5 } else { 2e-2 };
            assert!(max_diff(&cpu::gated_norm(&core, &z, &weight, 1e-6)?, &generic::gated_norm(&core, &z, &weight, 1e-6)?)? < tol);
        }
        Ok(())
    }

    /// The CPU recurrence against a direct transcription of the reference loop.
    #[test]
    fn cpu_recurrence_matches_reference_loop() -> Result<()> {
        let (len, heads, dk, dv) = (9, 3, 8, 5);
        let dev = Device::Cpu;
        let q = Tensor::randn(0f32, 1., (len, heads, dk), &dev)?;
        let k = Tensor::randn(0f32, 1., (len, heads, dk), &dev)?;
        let v = Tensor::randn(0f32, 1., (len, heads, dv), &dev)?;
        let g = (Tensor::rand(0f32, 1., (len, heads), &dev)? * -1.0)?;
        let beta = Tensor::rand(0f32, 1., (len, heads), &dev)?;
        let p = Prepared { qk: Tensor::cat(&[&q, &k], 2)?, v: v.clone(), gb: Tensor::stack(&[&g, &beta], 2)? };
        let got = cpu::recurrence(&p)?.to_vec3::<f32>()?;
        let (qv, kv, vv) = (q.to_vec3::<f32>()?, k.to_vec3::<f32>()?, v.to_vec3::<f32>()?);
        let (gv, bv) = (g.to_vec2::<f32>()?, beta.to_vec2::<f32>()?);
        for h in 0..heads {
            let mut s = vec![vec![0f32; dv]; dk];
            for t in 0..len {
                for row in s.iter_mut() {
                    for x in row.iter_mut() {
                        *x *= gv[t][h].exp();
                    }
                }
                let mem: Vec<f32> = (0..dv).map(|j| (0..dk).map(|i| s[i][j] * kv[t][h][i]).sum()).collect();
                let delta: Vec<f32> = (0..dv).map(|j| (vv[t][h][j] - mem[j]) * bv[t][h]).collect();
                for i in 0..dk {
                    for j in 0..dv {
                        s[i][j] += kv[t][h][i] * delta[j];
                    }
                }
                for j in 0..dv {
                    let o: f32 = (0..dk).map(|i| s[i][j] * qv[t][h][i]).sum();
                    assert!((o - got[t][h][j]).abs() < 1e-4, "t{t} h{h} j{j}: {o} vs {}", got[t][h][j]);
                }
            }
        }
        Ok(())
    }

    /// The CUDA kernels against the CPU kernels on the same inputs (dk 128 as in every d3 size).
    #[cfg(feature = "cuda")]
    #[test]
    fn cuda_kernels_match_cpu() -> Result<()> {
        let dims = Dims { k_heads: 2, v_heads: 4, dk: 128, dv: 128 };
        let gpu = Device::new_cuda(0)?;
        for dtype in [DType::F32, DType::BF16] {
            let (qkv, a, b, conv, dt_bias, neg_a) = random_prep(&Device::Cpu, dtype, dims, 70)?;
            let w = PrepWeights { conv: &conv, dt_bias: &dt_bias, neg_a: &neg_a };
            let host = cpu::prepare(&qkv, &a, &b, &w, dims)?;
            let (cg, dg, ng) = (conv.to_device(&gpu)?, dt_bias.to_device(&gpu)?, neg_a.to_device(&gpu)?);
            let wg = PrepWeights { conv: &cg, dt_bias: &dg, neg_a: &ng };
            let dev_p = cuda::prepare(&qkv.to_device(&gpu)?, &a.to_device(&gpu)?, &b.to_device(&gpu)?, &wg, dims)?;
            assert!(max_diff(&host.qk, &dev_p.qk.to_device(&Device::Cpu)?)? < 1e-5, "{dtype:?} qk");
            assert!(max_diff(&host.gb, &dev_p.gb.to_device(&Device::Cpu)?)? < 1e-5, "{dtype:?} gb");
            let core_h = cpu::recurrence(&host)?;
            let core_d = cuda::recurrence(&dev_p)?;
            assert!(max_diff(&core_h, &core_d.to_device(&Device::Cpu)?)? < 1e-3, "{dtype:?} recurrence");
            let z = Tensor::randn(0f32, 1., (70, 4 * 128), &Device::Cpu)?.to_dtype(dtype)?;
            let weight = Tensor::randn(0f32, 1., 128, &Device::Cpu)?.to_dtype(dtype)?;
            let nh = cpu::gated_norm(&core_h, &z, &weight, 1e-6)?;
            let nd = cuda::gated_norm(&core_h.to_device(&gpu)?, &z.to_device(&gpu)?, &weight.to_device(&gpu)?, 1e-6)?;
            let tol = if dtype == DType::F32 { 1e-4 } else { 2e-2 };
            assert!(max_diff(&nh, &nd.to_device(&Device::Cpu)?)? < tol, "{dtype:?} gated norm");
        }
        Ok(())
    }
}
