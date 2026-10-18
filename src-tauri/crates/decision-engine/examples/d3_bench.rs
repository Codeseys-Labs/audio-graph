//! Warm latency of one d3 checkpoint through `DecisionEngine::system_one`.
//!
//!   cargo run --release --features decision-d3[,cuda] --example d3_bench -- \
//!       MODELS_DIR d3-edge|d3-lite|d3-nano|d3-mini cpu|cuda|metal [f32|bf16] [ROUNDS]
//!
//! Requests are fixture questions: a 128-token support-ticket choice and a 751-token meeting choice.

#[cfg(feature = "decision-d3")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use audio_graph_decision_engine::{
        D3_CHECKPOINTS, DecisionEngine, DecisionRequest, EngineConfig, ExecutionProvider,
        d3::{D3Engine, Precision},
    };
    use std::{path::PathBuf, time::Instant};

    const USAGE: &str = "usage: d3_bench MODELS_DIR ID cpu|cuda|metal [f32|bf16] [ROUNDS]";
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().ok_or(USAGE)?);
    let id = args.next().ok_or(USAGE)?;
    let checkpoint = D3_CHECKPOINTS
        .iter()
        .find(|c| c.id == id)
        .ok_or("unknown d3 checkpoint")?;
    let provider = match args.next().as_deref() {
        Some("cpu") => ExecutionProvider::Cpu,
        Some("cuda") => ExecutionProvider::Cuda,
        Some("metal") => ExecutionProvider::CoreMl,
        _ => return Err(USAGE.into()),
    };
    let precision = match args.next().as_deref() {
        Some("f32") => Some(Precision::F32),
        Some("bf16") => Some(Precision::Bf16),
        Some("default") | None => None,
        Some(_) => return Err(USAGE.into()),
    };
    let rounds: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(30);
    if rounds == 0 {
        return Err("rounds must be positive".into());
    }
    let mut config = EngineConfig::new(root);
    config.execution_provider = provider;
    let mut engine = D3Engine::load(&config, checkpoint)?;
    if let Some(precision) = precision {
        engine = engine.with_precision(precision);
    }
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/d3-edge.json"))?;
    let request =
        |case: &str, question: &str| -> Result<DecisionRequest, Box<dyn std::error::Error>> {
            let case = fixture["cases"]
                .as_array()
                .ok_or("cases")?
                .iter()
                .find(|c| c["id"] == case)
                .ok_or("case")?;
            let mut request = case["request"].clone();
            let only = request["questions"][question].clone();
            request["questions"] = serde_json::json!({ question: only });
            Ok(serde_json::from_value(request)?)
        };
    let started = Instant::now();
    engine
        .system_one(&request("card-support", "route")?)
        .await?;
    println!(
        "model={} provider={provider:?} precision={:?} load+first_ms={:.0}",
        checkpoint.repo,
        engine.precision(),
        started.elapsed().as_secs_f64() * 1e3
    );
    for (label, req) in [
        ("short", request("card-support", "route")?),
        ("long", request("long-meeting", "topic")?),
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
            "{label}: tokens={tokens} rounds={rounds} p50_ms={:.2} p95_ms={:.2}",
            percentile(0.5),
            percentile(0.95)
        );
    }
    Ok(())
}

#[cfg(not(feature = "decision-d3"))]
fn main() {
    eprintln!("enable decision-d3 to run native inference");
    std::process::exit(2);
}
