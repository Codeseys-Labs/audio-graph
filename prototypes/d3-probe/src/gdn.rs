//! Gated delta rule (the Gated DeltaNet token mixer of Qwen3.5) over one prompt, FP32.
//!
//! Inputs are already L2-normalized (q, k), q is scaled by `1/sqrt(dk)`, and k-heads are repeated to
//! the v-head count. Per head the recurrence is the reference `torch_recurrent_gated_delta_rule`:
//!
//! ```text
//! S = S * exp(g_t);  kv = S^T k_t;  delta = (v_t - kv) * beta_t;  S += k_t delta^T;  o_t = S^T q_t
//! ```
//!
//! The prompt is read once, causally, from a zero state: the d3 runtime never caches across requests.

use anyhow::Result;
use candle_core::{DType, Device, Tensor};
use rayon::prelude::*;

/// `q`, `k`: (L, H, DK); `v`: (L, H, DV); `g`, `beta`: (L, H). All FP32. Returns (L, H, DV) FP32.
pub fn gated_delta_rule(q: &Tensor, k: &Tensor, v: &Tensor, g: &Tensor, beta: &Tensor) -> Result<Tensor> {
    match q.device() {
        Device::Cpu => cpu(q, k, v, g, beta),
        #[cfg(feature = "cuda")]
        Device::Cuda(_) => cuda::run(q, k, v, g, beta),
        other => anyhow::bail!("no gated delta rule kernel for {other:?}"),
    }
}

fn cpu(q: &Tensor, k: &Tensor, v: &Tensor, g: &Tensor, beta: &Tensor) -> Result<Tensor> {
    let (len, heads, dk) = q.dims3()?;
    let dv = v.dim(2)?;
    let flat = |t: &Tensor| -> Result<Vec<f32>> { Ok(t.to_dtype(DType::F32)?.flatten_all()?.to_vec1::<f32>()?) };
    let (q, k, v, g, beta) = (flat(q)?, flat(k)?, flat(v)?, flat(g)?, flat(beta)?);
    // One task per head; each owns its (DK, DV) state and writes a (L, DV) slab.
    let per_head: Vec<Vec<f32>> = (0..heads)
        .into_par_iter()
        .map(|h| {
            let mut state = vec![0f32; dk * dv];
            let mut kv = vec![0f32; dv];
            let mut out = vec![0f32; len * dv];
            for t in 0..len {
                let qt = &q[(t * heads + h) * dk..][..dk];
                let kt = &k[(t * heads + h) * dk..][..dk];
                let vt = &v[(t * heads + h) * dv..][..dv];
                let decay = g[t * heads + h].exp();
                let b = beta[t * heads + h];
                kv.fill(0.0);
                for i in 0..dk {
                    let row = &mut state[i * dv..][..dv];
                    let ki = kt[i];
                    for (s, acc) in row.iter_mut().zip(kv.iter_mut()) {
                        *s *= decay;
                        *acc += *s * ki;
                    }
                }
                for (acc, vj) in kv.iter_mut().zip(vt) {
                    *acc = (vj - *acc) * b; // kv now holds delta
                }
                let ot = &mut out[t * dv..][..dv];
                for i in 0..dk {
                    let row = &mut state[i * dv..][..dv];
                    let (ki, qi) = (kt[i], qt[i]);
                    for ((s, d), o) in row.iter_mut().zip(&kv).zip(ot.iter_mut()) {
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

#[cfg(feature = "cuda")]
mod cuda {
    use anyhow::Result;
    use candle_core::cuda::cudarc::driver::{CudaView, LaunchConfig, PushKernelArg};
    use candle_core::cuda::cudarc::nvrtc::{compile_ptx_with_opts, CompileOptions};
    use candle_core::cuda::WrapErr;
    use candle_core::{CudaStorage, CustomOp3, Layout, Shape, Tensor};
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    /// One block per (v-tile, head). Each value column is owned by `G` adjacent lanes that split the DK
    /// state rows between them (rows `r*G + lane`, so the lanes hit different shared-memory banks) and
    /// combine their partial dot products with warp shuffles. The next token's q/k row is prefetched into
    /// the other half of a double buffer while the current token computes. Tokens run in order inside the
    /// kernel, so a prompt is one launch per layer.
    const SOURCE: &str = r#"
#define G 4
#define R (DK / G)
extern "C" __global__ void gdn_recurrence(
    const float* __restrict__ qk,   // (L, H, 2*DK): q then k
    const float* __restrict__ v,    // (L, H, DV)
    const float* __restrict__ gb,   // (L, H, 2): g then beta
    float* __restrict__ out,        // (L, H, DV)
    int len, int heads, int dv)
{
    const int h = blockIdx.y;
    const int lane = threadIdx.x % G;
    const int j = blockIdx.x * (blockDim.x / G) + threadIdx.x / G;
    __shared__ float buf[2][2 * DK];   // [parity][q | k]
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

    /// Threads per block: 32 value columns x G=4 lanes.
    const BLOCK: u32 = 128;
    const COLUMNS_PER_BLOCK: u32 = BLOCK / 4;

    fn ptx(dk: usize) -> Result<String> {
        static CACHE: LazyLock<Mutex<HashMap<usize, String>>> = LazyLock::new(Default::default);
        let mut cache = CACHE.lock().expect("ptx cache");
        if let Some(p) = cache.get(&dk) {
            return Ok(p.clone());
        }
        let opts = CompileOptions { options: vec![format!("-DDK={dk}")], ..Default::default() };
        let ptx = compile_ptx_with_opts(SOURCE, opts)?.to_src();
        cache.insert(dk, ptx.clone());
        Ok(ptx)
    }

    struct Gdn {
        len: usize,
        heads: usize,
        dk: usize,
        dv: usize,
    }

    impl CustomOp3 for Gdn {
        fn name(&self) -> &'static str {
            "d3-gdn-recurrence"
        }

        fn cpu_fwd(
            &self,
            _: &candle_core::CpuStorage,
            _: &Layout,
            _: &candle_core::CpuStorage,
            _: &Layout,
            _: &candle_core::CpuStorage,
            _: &Layout,
        ) -> candle_core::Result<(candle_core::CpuStorage, Shape)> {
            candle_core::bail!("d3-gdn-recurrence cuda op called on cpu")
        }

        fn cuda_fwd(
            &self,
            qk: &CudaStorage,
            qk_l: &Layout,
            v: &CudaStorage,
            v_l: &Layout,
            gb: &CudaStorage,
            gb_l: &Layout,
        ) -> candle_core::Result<(CudaStorage, Shape)> {
            let dev = qk.device.clone();
            fn view<'a>(s: &'a CudaStorage, l: &Layout) -> candle_core::Result<CudaView<'a, f32>> {
                let (a, b) = l
                    .contiguous_offsets()
                    .ok_or_else(|| candle_core::Error::Msg("gdn input not contiguous".into()))?;
                Ok(s.as_cuda_slice::<f32>()?.slice(a..b))
            }
            let (qk, v, gb) = (view(qk, qk_l)?, view(v, v_l)?, view(gb, gb_l)?);
            let ptx = ptx(self.dk).map_err(|e| candle_core::Error::Msg(e.to_string()))?;
            let func = dev.get_or_load_custom_func("gdn_recurrence", &format!("d3_gdn_dk{}", self.dk), &ptx)?;
            let n = self.len * self.heads * self.dv;
            let out = unsafe { dev.alloc::<f32>(n)? };
            let cfg = LaunchConfig {
                grid_dim: ((self.dv as u32).div_ceil(COLUMNS_PER_BLOCK), self.heads as u32, 1),
                block_dim: (BLOCK, 1, 1),
                shared_mem_bytes: 0,
            };
            let (len, heads, dv) = (self.len as i32, self.heads as i32, self.dv as i32);
            let mut b = func.builder();
            b.arg(&qk).arg(&v).arg(&gb).arg(&out).arg(&len).arg(&heads).arg(&dv);
            unsafe { b.launch(cfg) }.w()?;
            Ok((CudaStorage::wrap_cuda_slice(out, dev), Shape::from((self.len, self.heads, self.dv))))
        }
    }

    pub fn run(q: &Tensor, k: &Tensor, v: &Tensor, g: &Tensor, beta: &Tensor) -> Result<Tensor> {
        let (len, heads, dk) = q.dims3()?;
        let dv = v.dim(2)?;
        let qk = Tensor::cat(&[q, k], 2)?.contiguous()?;
        let gb = Tensor::stack(&[g, beta], 2)?.contiguous()?;
        let v = v.contiguous()?;
        Ok(qk.apply_op3_no_bwd(&v, &gb, &Gdn { len, heads, dk, dv })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CPU kernel against a direct transcription of the reference recurrence on random inputs.
    #[test]
    fn cpu_matches_reference_recurrence() -> Result<()> {
        let (len, heads, dk, dv) = (9, 3, 8, 5);
        let dev = Device::Cpu;
        let q = Tensor::randn(0f32, 1., (len, heads, dk), &dev)?;
        let k = Tensor::randn(0f32, 1., (len, heads, dk), &dev)?;
        let v = Tensor::randn(0f32, 1., (len, heads, dv), &dev)?;
        let g = (Tensor::rand(0f32, 1., (len, heads), &dev)? * -1.0)?;
        let beta = Tensor::rand(0f32, 1., (len, heads), &dev)?;
        let got = cpu(&q, &k, &v, &g, &beta)?.to_vec3::<f32>()?;
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
}
