//! cargo run --locked --no-default-features --features cloud --example system_one_eval -- --endpoint jev --allow-cloud
use audio_graph::decision::{CloudflareModel, EndpointKind, SystemOneHttpEngine, eval};
use std::path::Path;

fn value(args: &[String], flag: &str) -> Result<Option<String>, String> {
    match args.iter().position(|s| s == flag) {
        Some(index) => args
            .get(index + 1)
            .filter(|s| !s.starts_with("--"))
            .cloned()
            .map(Some)
            .ok_or_else(|| format!("Missing value for {flag}")),
        None => Ok(None),
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute().await {
        eprintln!("system_one_eval: {error}");
        std::process::exit(1);
    }
}

async fn execute() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help") {
        println!(
            "system_one_eval --endpoint jev|cf-jev|cf-clef|ollama|vllm [--allow-cloud] [--allow-private-session] [--fixture PATH | --session ID] [--model NAME] [--url LOOPBACK_SYSTEMONE_URL] [--account-id ID] [--limit N] [--input-price USD_PER_MTOK]\nDefault input: fixtures/projection_eval/two_span_repair.json. Credentials: saved keychain slot; harness-only TYPESAFE_API_KEY/CLOUDFLARE_API_TOKEN overrides. Only aggregate metrics are printed. --session requires --allow-private-session after the user's explicit approval, even for local replay. Cloud requests additionally require --allow-cloud. No private session is loaded without its consent flag."
        );
        return Ok(());
    }
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--allow-cloud" | "--allow-private-session" => index += 1,
            "--endpoint" | "--fixture" | "--session" | "--model" | "--url" | "--account-id"
            | "--limit" | "--input-price" => {
                value(&args, &args[index])?;
                index += 2;
            }
            _ => return Err("Unknown argument; see --help".to_owned()),
        }
    }
    let endpoint = value(&args, "--endpoint")?.unwrap_or_else(|| "jev".to_owned());
    let kind = match endpoint.as_str() {
        "jev" => EndpointKind::TypeSafe,
        "cf-jev" | "cf-clef" => EndpointKind::Cloudflare {
            account_id: value(&args, "--account-id")?
                .or_else(|| std::env::var("CLOUDFLARE_ACCOUNT_ID").ok())
                .ok_or("Missing --account-id or CLOUDFLARE_ACCOUNT_ID")?,
            model: if endpoint == "cf-jev" {
                CloudflareModel::Jev
            } else {
                CloudflareModel::ClefFlash
            },
        },
        "ollama" => EndpointKind::Ollama {
            url: value(&args, "--url")?
                .unwrap_or_else(|| "http://127.0.0.1:11434/v1/systemone".to_owned()),
            model: value(&args, "--model")?.unwrap_or_else(|| "nimble".to_owned()),
        },
        "vllm" => EndpointKind::Vllm {
            url: value(&args, "--url")?
                .unwrap_or_else(|| "http://127.0.0.1:8000/v1/systemone".to_owned()),
            model: value(&args, "--model")?.ok_or("vLLM requires --model")?,
        },
        _ => return Err("Unknown endpoint; see --help".to_owned()),
    };
    if kind.credential_key().is_some() && !args.iter().any(|s| s == "--allow-cloud") {
        return Err("Cloud evaluation requires --allow-cloud; no content was sent".to_owned());
    }
    let session = value(&args, "--session")?;
    let fixture = value(&args, "--fixture")?;
    if session.is_some() && fixture.is_some() {
        return Err("Choose --session or --fixture, not both".to_owned());
    }
    if session.is_some() && !args.iter().any(|s| s == "--allow-private-session") {
        return Err(
            "Private session replay requires explicit user approval and --allow-private-session; no session content was loaded or sent"
                .to_owned(),
        );
    }
    let mut corpus = match session {
        Some(id) => eval::load_session(&id)?,
        None => eval::load_fixture(Path::new(fixture.as_deref().unwrap_or(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/fixtures/projection_eval/two_span_repair.json"
        ))))?,
    };
    if let Some(limit) = value(&args, "--limit")? {
        let limit: usize = limit.parse().map_err(|_| "Invalid --limit")?;
        if limit == 0 {
            return Err("--limit must be positive".to_owned());
        }
        corpus.spans.truncate(limit);
    }
    let price = match value(&args, "--input-price")? {
        Some(p) => p.parse().map_err(|_| "Invalid --input-price")?,
        None => eval::input_price(&kind),
    };
    let engine = SystemOneHttpEngine::for_eval(kind).map_err(|e| e.to_string())?;
    let report = eval::run(&engine, &corpus, price).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|_| "Report encoding failed")?
    );
    Ok(())
}
