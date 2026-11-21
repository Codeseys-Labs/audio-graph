//! Warm latency of Clef-flash through `DecisionEngine::system_one`.
//!
//!   cargo run --release --features decision-clef[,cuda] --example clef_bench -- \
//!       MODELS_DIR cpu|cuda|metal [bf16|f32] [ROUNDS]
//!
//! Requests are fixture records: a 3-question support ticket (335 tokens) and a 4-question meeting
//! transcript (1,930 tokens). One forward pass answers every question of a request.

#[cfg(feature = "decision-clef")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use audio_graph_decision_engine::clef::{ClefEngine, Precision};
    use audio_graph_decision_engine::{
        DecisionEngine, DecisionRequest, EngineConfig, ExecutionProvider,
    };
    use std::{path::PathBuf, time::Instant};

    const USAGE: &str = "usage: clef_bench MODELS_DIR cpu|cuda|metal [bf16|f32] [ROUNDS]";
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().ok_or(USAGE)?);
    let provider = match args.next().as_deref() {
        Some("cpu") => ExecutionProvider::Cpu,
        Some("cuda") => ExecutionProvider::Cuda,
        Some("metal") => ExecutionProvider::CoreMl,
        _ => return Err(USAGE.into()),
    };
    let precision = match args.next().as_deref() {
        Some("bf16") => Some(Precision::Bf16),
        Some("f32") => Some(Precision::F32),
        Some("default") | None => None,
        Some(_) => return Err(USAGE.into()),
    };
    let rounds: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(30);
    if rounds == 0 {
        return Err("rounds must be positive".into());
    }
    // Device memory in use (MiB), read through a context of our own; None off CUDA.
    let used_mib = || -> Option<f64> {
        #[cfg(feature = "cuda")]
        if provider == ExecutionProvider::Cuda {
            let device = candle_core::Device::new_cuda(0).ok()?;
            let (free, total) = device
                .as_cuda_device()
                .ok()?
                .cuda_stream()
                .context()
                .mem_get_info()
                .ok()?;
            return Some((total - free) as f64 / 1048576.0);
        }
        None
    };
    let before = used_mib();
    let mut config = EngineConfig::new(root);
    config.execution_provider = provider;
    let mut engine = ClefEngine::load(&config)?;
    if let Some(precision) = precision {
        engine = engine.with_precision(precision);
    }
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/clef-flash.json"))?;
    let request = |id: &str| -> Result<DecisionRequest, Box<dyn std::error::Error>> {
        let case = fixture["cases"]
            .as_array()
            .ok_or("cases")?
            .iter()
            .find(|c| c["id"] == id)
            .ok_or("case")?;
        Ok(serde_json::from_value(case["request"].clone())?)
    };
    let started = Instant::now();
    engine.system_one(&request("support-ticket")?).await?;
    println!(
        "provider={provider:?} precision={:?} load+first_ms={:.0}",
        engine.precision(),
        started.elapsed().as_secs_f64() * 1e3
    );
    for (label, req) in [
        ("short", request("support-ticket")?),
        ("long", request("long-meeting")?),
    ] {
        for _ in 0..3 {
            engine.system_one(&req).await?;
        }
        let mut latency = Vec::with_capacity(rounds);
        let mut tokens = 0;
        for _ in 0..rounds {
            let start = Instant::now();
            let response = std::hint::black_box(engine.system_one(&req).await?);
            latency.push(start.elapsed().as_secs_f64() * 1e3);
            tokens = response.usage.input_tokens;
        }
        latency.sort_by(f64::total_cmp);
        let percentile = |p: f64| {
            latency[((rounds as f64 * p).ceil() as usize)
                .saturating_sub(1)
                .min(rounds - 1)]
        };
        println!(
            "{label}: questions={} tokens={tokens} rounds={rounds} p50_ms={:.2} p95_ms={:.2}",
            req.questions.len(),
            percentile(0.5),
            percentile(0.95)
        );
    }
    if let (Some(before), Some(after)) = (before, used_mib()) {
        println!(
            "device_mib_delta={:.0} (device-wide: keep other GPU work idle)",
            after - before
        );
    }
    Ok(())
}

#[cfg(not(feature = "decision-clef"))]
fn main() {
    eprintln!("enable decision-clef to run native inference");
    std::process::exit(2);
}
