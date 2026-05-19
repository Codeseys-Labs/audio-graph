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

    let manifest = env::current_dir()
        .expect("current dir should be available to the build script")
        .join(Path::new(WINDOWS_TEST_MANIFEST));

    println!("cargo::rerun-if-changed={}", manifest.display());
    // Use the test-scoped link arg so the manifest is only embedded into
    // test binaries — never into the production app exe (which already has
    // its own manifest from tauri-build / winres).
    println!("cargo::rustc-link-arg-tests=/MANIFEST:EMBED");
    println!(
        "cargo::rustc-link-arg-tests=/MANIFESTINPUT:{}",
        manifest.display()
    );
    println!(
        "cargo::warning=AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST is set; embedding test manifest from {}",
        manifest.display()
    );
}
