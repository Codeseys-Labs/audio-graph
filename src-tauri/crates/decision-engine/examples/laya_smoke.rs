#[cfg(feature = "decision-laya")]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use audio_graph_decision_engine::{
        Answer, DecisionRequest, EngineConfig, EngineRegistry, ExecutionProvider,
    };
    use std::{path::PathBuf, time::Instant};
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(
        args.next()
            .ok_or("usage: laya_smoke MODELS_DIR cpu|directml|cuda|coreml FIXTURE [ROUNDS]")?,
    );
    let provider = match args.next().as_deref() {
        Some("cpu") => ExecutionProvider::Cpu,
        Some("directml") => ExecutionProvider::DirectMl,
        Some("cuda") => ExecutionProvider::Cuda,
        Some("coreml") => ExecutionProvider::CoreMl,
        _ => return Err("invalid execution provider".into()),
    };
    let fixture: serde_json::Value =
        serde_json::from_slice(&std::fs::read(args.next().ok_or("fixture path required")?)?)?;
    let rounds: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(30);
    if rounds == 0 {
        return Err("rounds must be positive".into());
    }
    let mut config = EngineConfig::new(root);
    config.execution_provider = provider;
    let engine = EngineRegistry::default().create("laya", &config)?;
    let mut max_error = 0_f64;
    for case in fixture["cases"].as_array().ok_or("cases array required")? {
        let request: DecisionRequest = serde_json::from_value(case["request"].clone())?;
        let response = engine.system_one(&request).await?;
        for (id, answer) in &response.answers {
            let expected = &case["reference"]["answers"][id];
            match answer {
                Answer::Choice {
                    choice,
                    probabilities,
                    ..
                } => {
                    if expected["choice"].as_str() != Some(choice) {
                        return Err(format!("choice mismatch for {id}").into());
                    }
                    for (key, p) in probabilities {
                        max_error = max_error.max(
                            (p - expected["probabilities"][key]
                                .as_f64()
                                .ok_or("expected choice probability")?)
                            .abs(),
                        );
                    }
                }
                Answer::Score {
                    score,
                    probabilities,
                    ..
                } => {
                    max_error = max_error
                        .max((score - expected["score"].as_f64().ok_or("expected score")?).abs());
                    for (i, p) in probabilities.iter().enumerate() {
                        max_error = max_error.max(
                            (p - expected["probabilities"][i.to_string()]
                                .as_f64()
                                .ok_or("expected score probability")?)
                            .abs(),
                        );
                    }
                }
                Answer::Noul { noul, .. } => {
                    max_error = max_error
                        .max((noul - expected["noul"].as_f64().ok_or("expected noul")?).abs());
                }
            }
        }
        if max_error > 0.001 {
            return Err(format!("parity failed, max error {max_error}").into());
        }
        for _ in 0..3 {
            engine.system_one(&request).await?;
        }
        let mut latency = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            let start = Instant::now();
            std::hint::black_box(engine.system_one(&request).await?);
            latency.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        latency.sort_by(f64::total_cmp);
        let percentile = |p: f64| {
            latency[((rounds as f64 * p).ceil() as usize)
                .saturating_sub(1)
                .min(rounds - 1)]
        };
        let route_start = Instant::now();
        for _ in 0..10_000 {
            std::hint::black_box(audio_graph_decision_engine::laya::checkpoint_for_request(
                std::hint::black_box(&request),
            ));
        }
        let router_us = route_start.elapsed().as_secs_f64() * 1e6 / 10_000.0;
        let router_percent = router_us / (percentile(0.5) * 1000.0) * 100.0;
        println!(
            "provider={provider:?} model={} questions={} tokens={} rounds={rounds} p50_ms={:.3} p95_ms={:.3} router_us={router_us:.3} router_percent={router_percent:.5} max_error={max_error:.8}",
            response.model,
            request.questions.len(),
            response.usage.input_tokens,
            percentile(0.5),
            percentile(0.95)
        );
    }
    println!("parity_pass max_error={max_error:.8}");
    Ok(())
}

#[cfg(not(feature = "decision-laya"))]
fn main() {
    eprintln!("enable decision-laya to run native inference");
    std::process::exit(2);
}
