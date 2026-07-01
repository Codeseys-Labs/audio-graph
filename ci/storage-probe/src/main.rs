//! durability-probe — seed 2b2c cross-process durability evidence (THROWAWAY).
//!
//! Usage (driven by the CI job, best-effort):
//!   durability-probe write <path> <rows>   # writer process: write N rows, exit
//!   durability-probe read  <path> <rows>   # reader process ("restart"): assert N rows survive
//!
//! The two halves run as SEPARATE process invocations — that is the kill/restart
//! proxy. The file-backed engines hold an exclusive on-disk LOCK, so a
//! same-process reopen is not a valid durability test (it would just fail to
//! acquire the lock, which itself proves data hit disk). Driving it across two
//! processes is the honest test the Linux leg already passed for SurrealKV.
//!
//! This binary is best-effort: the CI step that runs it does NOT fail the job on
//! a non-zero exit (durability is evidence, not a gate). Output is plain text so
//! the evidence artifact captures engine + row counts per OS.

#[cfg(any(feature = "surrealkv", feature = "rocksdb"))]
use storage_probe::{DurabilityOp, round_trip};

#[tokio::main]
async fn main() {
    let engine = storage_probe::engine_label();
    println!("engine={engine}");

    // No engine compiled in: nothing to probe. Exit 0 so a feature-less build
    // (the manifest-validation `cargo check`) still runs the binary cleanly.
    #[cfg(not(any(feature = "surrealkv", feature = "rocksdb")))]
    {
        println!("durability=skipped (no engine feature)");
        return;
    }

    #[cfg(any(feature = "surrealkv", feature = "rocksdb"))]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.len() < 4 {
            eprintln!("usage: durability-probe <write|read> <path> <rows>");
            std::process::exit(2);
        }
        let phase = args[1].as_str();
        let path = args[2].clone();
        let rows: usize = args[3].parse().unwrap_or(0);

        let op = match phase {
            "write" => DurabilityOp::Write { rows },
            "read" => DurabilityOp::Read,
            other => {
                eprintln!("unknown phase: {other}");
                std::process::exit(2);
            }
        };

        match round_trip(&path, op).await {
            Ok(count) => {
                println!("phase={phase} engine={engine} rows_present={count} expected={rows}");
                if phase == "read" && count < rows {
                    eprintln!(
                        "DURABILITY MISS (best-effort): expected {rows} rows after restart, found {count}"
                    );
                    std::process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("durability probe error (best-effort, non-fatal to the job): {e}");
                std::process::exit(1);
            }
        }
    }
}
