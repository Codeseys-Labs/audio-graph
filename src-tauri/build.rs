use std::{env, path::Path};

const EMBED_WINDOWS_TEST_MANIFEST: &str = "AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST";
const WINDOWS_TEST_MANIFEST: &str = "windows-app-manifest.xml";

fn main() {
    println!("cargo::rerun-if-env-changed={EMBED_WINDOWS_TEST_MANIFEST}");
    embed_windows_test_manifest();
    tauri_build::build();
}

fn is_truthy(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
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
