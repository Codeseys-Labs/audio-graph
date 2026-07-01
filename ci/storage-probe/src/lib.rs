//! storage-probe — seed 2b2c storage-engine evidence (THROWAWAY).
//!
//! This library names `surrealdb::engine::local` and (when an engine feature is
//! enabled) constructs the file-backed engine so the engine code is actually
//! *linked*, not merely compiled. That is the load-bearing evidence ADR-0021
//! needs: "do `kv-surrealkv` / `kv-rocksdb` build AND link on this OS?".
//!
//! It is feature-gated so a bare `cargo check` (no engine feature) still
//! compiles and validates the manifest.

// Always reference the local-engine module so a no-feature build still touches
// the relevant API surface. `Db` is the connection type for every embedded
// file-backed engine.
#[allow(unused_imports)]
use surrealdb::engine::local::Db;

/// Returns a static label for the engine compiled into this build. Used by the
/// durability binary's evidence output. Distinct arms per feature so the linked
/// engine type is named in each configuration.
pub fn engine_label() -> &'static str {
    #[cfg(feature = "surrealkv")]
    {
        // Name the SurrealKv engine type so it links.
        let _ = std::any::type_name::<surrealdb::engine::local::SurrealKv>();
        "surrealkv"
    }
    #[cfg(all(feature = "rocksdb", not(feature = "surrealkv")))]
    {
        let _ = std::any::type_name::<surrealdb::engine::local::RocksDb>();
        "rocksdb"
    }
    #[cfg(not(any(feature = "surrealkv", feature = "rocksdb")))]
    {
        "none"
    }
}

/// Best-effort durability round-trip for the file-backed engine compiled in.
///
/// Opens a file-backed store at `path`, runs `op`, and returns the number of
/// rows currently in the `probe` table. The caller drives the kill/reopen across
/// two *process* invocations (a same-process reopen is invalid because the
/// engines hold an exclusive on-disk LOCK).
///
/// Compiled only when an engine feature is on; with no engine the durability
/// binary prints "engine=none" and exits 0 (nothing to probe).
#[cfg(any(feature = "surrealkv", feature = "rocksdb"))]
pub async fn round_trip(
    path: &str,
    op: DurabilityOp,
) -> Result<usize, Box<dyn std::error::Error>> {
    use surrealdb::Surreal;

    // Connect to the engine selected at compile time. SurrealKV and RocksDB take
    // a filesystem path; both are addressed through `engine::local`.
    #[cfg(feature = "surrealkv")]
    let db = Surreal::new::<surrealdb::engine::local::SurrealKv>(path).await?;
    #[cfg(all(feature = "rocksdb", not(feature = "surrealkv")))]
    let db = Surreal::new::<surrealdb::engine::local::RocksDb>(path).await?;

    db.use_ns("probe_ns").use_db("probe_db").await?;

    match op {
        DurabilityOp::Write { rows } => {
            for i in 0..rows {
                let _: Option<serde_json::Value> = db
                    .create(("probe", i.to_string()))
                    .content(serde_json::json!({ "seq": i, "payload": "2b2c-evidence" }))
                    .await?;
            }
        }
        DurabilityOp::Read => {}
    }

    let existing: Vec<serde_json::Value> = db.select("probe").await?;
    Ok(existing.len())
}

/// Which half of the cross-process durability probe to run.
#[cfg(any(feature = "surrealkv", feature = "rocksdb"))]
#[derive(Clone, Copy, Debug)]
pub enum DurabilityOp {
    /// Write `rows` rows, then count.
    Write { rows: usize },
    /// Reopen and count only (the "after restart" half).
    Read,
}
