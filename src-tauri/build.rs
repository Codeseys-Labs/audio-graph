use std::{env, path::Path};

const EMBED_WINDOWS_TEST_MANIFEST: &str = "AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST";
const WINDOWS_TEST_MANIFEST: &str = "windows-app-manifest.xml";

/// The cmake/cc env var that forces the C++ ML deps to the debug dynamic CRT.
/// See [`warn_windows_debug_crt_skew`] and docs/ops/windows-debug-crt-fix.md.
const CMAKE_MSVC_RUNTIME_LIBRARY: &str = "CMAKE_MSVC_RUNTIME_LIBRARY";

fn main() {
    println!("cargo::rerun-if-env-changed={EMBED_WINDOWS_TEST_MANIFEST}");
    println!("cargo::rerun-if-env-changed={CMAKE_MSVC_RUNTIME_LIBRARY}");
    embed_windows_test_manifest();
    warn_windows_debug_crt_skew();
    tauri_build::build();
}

fn is_truthy(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Emit a self-explaining `cargo::warning` when a Windows-MSVC **debug** build
/// with local ML is happening WITHOUT the CRT-runtime override — the exact
/// condition that produces the `is_block_type_valid` debug-heap abort at runtime
/// (seed audio-graph-d47b).
///
/// This is a HINT only: a build script CANNOT set env for the already-spawned
/// `-sys` build scripts (build-graph ordering), so it cannot itself fix the
/// skew. The fix is to run with the override env (see
/// `scripts/run-windows-debug.ps1`). The warning turns a cryptic runtime dialog
/// into an actionable build-time message.
fn warn_windows_debug_crt_skew() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").ok();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").ok();
    if target_os.as_deref() != Some("windows") || target_env.as_deref() != Some("msvc") {
        return;
    }
    if env::var("PROFILE").unwrap_or_default() != "debug" {
        return;
    }
    // Only relevant when a native-ML feature that pulls a C++ CRT is enabled.
    // Cargo exposes enabled features as CARGO_FEATURE_<NAME> (uppercased, `-`→`_`).
    let local_ml = env::var("CARGO_FEATURE_ASR_WHISPER").is_ok()
        || env::var("CARGO_FEATURE_LLM_LLAMA").is_ok();
    if !local_ml {
        return;
    }
    // Already overridden? Then the build is correct — say nothing.
    if env::var(CMAKE_MSVC_RUNTIME_LIBRARY).is_ok() {
        return;
    }
    println!(
        "cargo::warning=Windows --debug local-ML build without {CMAKE_MSVC_RUNTIME_LIBRARY}=MultiThreadedDebugDLL: \
         the native ML libs (whisper.cpp/llama.cpp) will compile against the RELEASE CRT (/MD) while this debug \
         build links /MDd, so the app will abort at runtime with a debug-heap assertion (is_block_type_valid, \
         seed audio-graph-d47b). Run via scripts/run-windows-debug.ps1, or use a release build, or build cloud-only \
         (--no-default-features --features cloud). See docs/ops/windows-debug-crt-fix.md."
    );
}

fn embed_windows_test_manifest() {
    match env::var(EMBED_WINDOWS_TEST_MANIFEST) {
        Ok(v) if is_truthy(&v) => {}
        _ => return,
    }

    let target_os = env::var("CARGO_CFG_TARGET_OS").ok();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").ok();
    if target_os.as_deref() != Some("windows") || target_env.as_deref() != Some("msvc") {
        return;
    }

    // Defense in depth: the audio-graph crate has no explicit [[test]]
    // target — tests live in `#[cfg(test)] mod` blocks inside the lib —
    // so `rustc-link-arg-tests=` is invalid here. We use the unscoped
    // `rustc-link-arg=` form, which applies to ALL outputs of this crate.
    //
    // To prevent accidental contamination of production binaries, refuse to
    // proceed unless the build profile is `debug`. Release builds (which is
    // what `tauri build` uses) will see a hard error if the env var is set,
    // so it is impossible to silently embed the test manifest into a
    // shipping exe.
    let profile = env::var("PROFILE").unwrap_or_default();
    if profile != "debug" {
        panic!(
            "{EMBED_WINDOWS_TEST_MANIFEST} is set during a {profile:?} build.              This env var is for `cargo test` (debug profile) only — release              builds must not embed the test manifest. Unset the env var              before running tauri build."
        );
    }

    let manifest = env::current_dir()
        .expect("current dir should be available to the build script")
        .join(Path::new(WINDOWS_TEST_MANIFEST));

    println!("cargo::rerun-if-changed={}", manifest.display());
    println!("cargo::rustc-link-arg=/MANIFEST:EMBED");
    println!(
        "cargo::rustc-link-arg=/MANIFESTINPUT:{}",
        manifest.display()
    );
    println!(
        "cargo::warning=AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST is set; embedding test manifest from {}",
        manifest.display()
    );
}
