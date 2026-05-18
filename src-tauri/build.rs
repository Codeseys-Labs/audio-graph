use std::{env, path::Path};

const WINDOWS_TEST_MANIFEST: &str = "windows-app-manifest.xml";

fn main() {
    embed_windows_test_manifest();
    tauri_build::build();
}

fn embed_windows_test_manifest() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").ok();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").ok();
    if target_os.as_deref() != Some("windows") || target_env.as_deref() != Some("msvc") {
        return;
    }

    let manifest = env::current_dir()
        .expect("current dir should be available to the build script")
        .join(Path::new(WINDOWS_TEST_MANIFEST));

    println!("cargo::rerun-if-changed={}", manifest.display());
    println!("cargo::rustc-link-arg-tests=/MANIFEST:EMBED");
    println!(
        "cargo::rustc-link-arg-tests=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
