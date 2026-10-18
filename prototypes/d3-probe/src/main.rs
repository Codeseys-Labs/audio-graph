//! d3-probe: Phase A parity and latency probe for the Decision 3.0 native runtime (audio-graph-60c7).
//!
//!   d3-probe parity <snapshot> <fixture.json> [--device cpu|cuda] [--dtype f32|bf16] [--tol 1e-3]
//!   d3-probe bench  <snapshot> <fixture.json> [--device cpu|cuda] [--dtype f32|bf16] [--iters 30]

mod config;
mod format;
mod gdn;
mod model;
mod runtime;

use anyhow::{bail, Context, Result};
use candle_core::{DType, Device};
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Deserialize)]
struct Fixture {
    model: String,
    codes: Vec<String>,
    token_ids: Vec<u32>,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    request: Request,
    questions: Vec<FixtureQuestion>,
}

#[derive(Deserialize)]
struct Request {
    state: Value,
    questions: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct FixtureQuestion {
    id: String,
    prompt: String,
    input_ids: Vec<u32>,
    official: Vec<f64>,
    official_b1: Vec<f64>,
    fp32: Vec<f64>,
    fp32_causal: Vec<f64>,
}

struct Opts {
    snapshot: PathBuf,
    fixture: PathBuf,
    device: Device,
    dtype: DType,
    tol: f64,
    iters: usize,
}

fn parse(args: &[String]) -> Result<Opts> {
    if args.len() < 2 {
        bail!("usage: d3-probe <parity|bench> <snapshot> <fixture.json> [--device cpu|cuda] [--dtype f32|bf16]");
    }
    let mut opts = Opts {
        snapshot: PathBuf::from(&args[0]),
        fixture: PathBuf::from(&args[1]),
        device: Device::Cpu,
        dtype: DType::F32,
        tol: 1e-3,
        iters: 30,
    };
    let mut rest = args[2..].iter();
    while let Some(flag) = rest.next() {
        let value = rest.next().with_context(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--device" => {
                opts.device = match value.as_str() {
                    "cpu" => Device::Cpu,
                    "cuda" => {
                        let dev = Device::new_cuda(0)?;
                        // One stream, no cross-stream sharing: per-tensor CUDA event tracking is pure overhead.
                        #[cfg(feature = "cuda")]
                        if std::env::var_os("D3_CUDA_EVENTS").is_none() {
                            unsafe { dev.as_cuda_device()?.disable_event_tracking() };
                        }
                        dev
                    }
                    other => bail!("unknown device {other}"),
                }
            }
            "--dtype" => {
                opts.dtype = match value.as_str() {
                    "f32" => DType::F32,
                    "bf16" => DType::BF16,
                    other => bail!("unknown dtype {other}"),
                }
            }
            "--tol" => opts.tol = value.parse()?,
            "--iters" => opts.iters = value.parse()?,
            other => bail!("unknown flag {other}"),
        }
    }
    Ok(opts)
}

fn max_abs(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f64::max)
}

fn load(opts: &Opts) -> Result<(runtime::D3, Fixture)> {
    let fixture: Fixture = serde_json::from_slice(&std::fs::read(&opts.fixture)?)?;
    let started = Instant::now();
    let d3 = runtime::D3::load(&opts.snapshot, opts.dtype, &opts.device)?;
    eprintln!(
        "loaded {} ({} layers, hidden {}) on {:?} as {:?} in {:.2}s",
        fixture.model,
        d3.text.num_hidden_layers,
        d3.text.hidden_size,
        opts.device,
        opts.dtype,
        started.elapsed().as_secs_f64()
    );
    if d3.decision.codes != fixture.codes || d3.decision.token_ids != fixture.token_ids {
        bail!("fixture answer codes differ from the checkpoint's decision_config.json");
    }
    Ok((d3, fixture))
}

fn parity(opts: &Opts) -> Result<()> {
    let (d3, fixture) = load(opts)?;
    let (mut worst_fp32, mut worst_official, mut worst_b1, mut worst_causal) = (0f64, 0f64, 0f64, 0f64);
    let (mut questions, mut pick_mismatch, mut causal_flips) = (0usize, 0usize, 0usize);
    let mut failures = Vec::new();
    for case in &fixture.cases {
        for fq in &case.questions {
            questions += 1;
            let question = format::normalize_question(&case.request.questions[&fq.id])?;
            let prompt = d3.prompt(&case.request.state, &question)?;
            if prompt != fq.prompt {
                failures.push(format!("{}/{}: rendered prompt differs from d3_format.render", case.id, fq.id));
                continue;
            }
            let ids = d3.tokenize(&prompt)?;
            if ids != fq.input_ids {
                failures.push(format!("{}/{}: token ids differ ({} vs {} tokens)", case.id, fq.id, ids.len(), fq.input_ids.len()));
                continue;
            }
            let probs = format::normalized(&d3.probabilities(&ids, question.keys.len())?)?;
            let (dfp32, doff, db1) = (max_abs(&probs, &fq.fp32), max_abs(&probs, &fq.official), max_abs(&probs, &fq.official_b1));
            let dcausal = max_abs(&fq.fp32, &fq.fp32_causal);
            worst_fp32 = worst_fp32.max(dfp32);
            worst_official = worst_official.max(doff);
            worst_b1 = worst_b1.max(db1);
            worst_causal = worst_causal.max(dcausal);
            let pick = format::argmax(&probs);
            if pick != format::argmax(&fq.official) || pick != format::argmax(&fq.fp32) {
                pick_mismatch += 1;
                failures.push(format!("{}/{}: pick {} differs from the reference", case.id, fq.id, question.keys[pick]));
            }
            if format::argmax(&fq.fp32_causal) != format::argmax(&fq.fp32) {
                causal_flips += 1;
            }
            if opts.dtype == DType::F32 && dfp32 > opts.tol {
                failures.push(format!("{}/{}: |p - p_fp32| = {dfp32:.2e} > {:.0e}", case.id, fq.id, opts.tol));
            }
            println!(
                "{:>22}/{:<13} {:>4} tok  pick {:<10} p={:.6}  d_fp32={dfp32:.2e} d_official={doff:.2e} d_official_b1={db1:.2e}",
                case.id,
                fq.id,
                ids.len(),
                question.keys[pick],
                probs[pick]
            );
        }
    }
    println!(
        "\n{} questions: max|d| vs fp32 reference {worst_fp32:.3e}, vs official bf16 {worst_official:.3e}, vs official bf16 batch1 {worst_b1:.3e}; pick mismatches {pick_mismatch}",
        questions
    );
    println!("reference causal-vs-bidirectional (fp32): max|d| {worst_causal:.3e}, picks flipped {causal_flips}/{questions}");
    if failures.is_empty() {
        println!("PARITY OK");
        Ok(())
    } else {
        for f in &failures {
            eprintln!("FAIL {f}");
        }
        bail!("{} parity failures", failures.len())
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[rank]
}

/// Device memory in use (total - free), MiB; None on CPU.
fn device_used_mib(device: &Device) -> Option<f64> {
    #[cfg(feature = "cuda")]
    if device.is_cuda() {
        let (free, total) = candle_core::cuda::cudarc::driver::result::mem_get_info().ok()?;
        return Some((total - free) as f64 / 1048576.0);
    }
    let _ = device;
    None
}

fn bench(opts: &Opts) -> Result<()> {
    let before = device_used_mib(&opts.device);
    let (d3, fixture) = load(opts)?;
    let loaded = device_used_mib(&opts.device);
    // One short and one long question from the fixture.
    let pick = |case: &str, q: &str| -> Result<(Vec<u32>, usize)> {
        let c = fixture.cases.iter().find(|c| c.id == case).context("case")?;
        let fq = c.questions.iter().find(|x| x.id == q).context("question")?;
        Ok((fq.input_ids.clone(), format::normalize_question(&c.request.questions[q])?.keys.len()))
    };
    for (label, (ids, count)) in [
        ("text 1 question (card-support/route)", pick("card-support", "route")?),
        ("long text (long-meeting/topic)", pick("long-meeting", "topic")?),
    ] {
        for _ in 0..3 {
            d3.probabilities(&ids, count)?;
        }
        let mut times = Vec::with_capacity(opts.iters);
        for _ in 0..opts.iters {
            let t = Instant::now();
            d3.probabilities(&ids, count)?; // copies the probabilities to the host: synchronizes the device
            times.push(t.elapsed().as_secs_f64() * 1e3);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "{label}: {} tokens, {:?} {:?}: p50 {:.2} ms, p95 {:.2} ms (n={})",
            ids.len(),
            d3.device(),
            opts.dtype,
            percentile(&times, 0.5),
            percentile(&times, 0.95),
            times.len()
        );
    }
    if let (Some(b), Some(l), Some(after)) = (before, loaded, device_used_mib(&opts.device)) {
        println!("device memory: weights {:.0} MiB, after long forward {:.0} MiB (device-wide delta)", l - b, after - b);
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        bail!("usage: d3-probe <parity|bench> ...");
    };
    let opts = parse(&args[1..])?;
    match cmd.as_str() {
        "parity" => parity(&opts),
        "bench" => bench(&opts),
        other => bail!("unknown command {other}"),
    }
}
