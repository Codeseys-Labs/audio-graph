mod authority_journal;
mod keyring_entry;
#[cfg(any(target_os = "linux", test))]
mod linux_secret_service_wire;
// Capability-blind raw cores. Their policy adapters live under
// `native_interaction`, which forbids `unsafe`.
mod macos_acl_ffi;
mod macos_keychain_ffi;
mod native_interaction;
pub(crate) mod native_keyring;
mod windows_credential_manager_ffi;

// Re-sealed for the credential-v2 native assembly (#158): lexical platform
// children, the platform factory seam, recovery catches that dispose of panic
// payloads inside the redaction scope, and the `PanicWith` test step.
#[cfg(test)]
const NATIVE_INTERACTION_REVIEWED_SHA256: [u8; 32] = [
    0x72, 0xd5, 0x07, 0xb8, 0x36, 0x3b, 0xd9, 0x37, 0x94, 0x8c, 0x98, 0xe8, 0xdb, 0x29, 0xd7, 0x28,
    0x3d, 0x0a, 0xca, 0xf0, 0x34, 0x3d, 0x16, 0xac, 0x95, 0x94, 0x0d, 0xb9, 0x70, 0x03, 0x92, 0x6c,
];

/// Child-process crash-hook canaries shared by the native boundary tests.
#[cfg(test)]
mod panic_hook_canary {
    use std::path::Path;

    const CANARY_ENV: &str = "AUDIOGRAPH_TEST_NATIVE_PANIC_CANARY";
    const REPORT_PREFIX: &str = "AudioGraph crash report\n\nPayload:\n[redacted panic payload: ";
    const REPORT_SUFFIX: &str = "]\n";

    /// The private canary inside a child run; `None` in an ordinary test run.
    pub(super) fn child_canary() -> Option<String> {
        std::env::var(CANARY_ENV).ok()
    }

    fn required_canary() -> String {
        child_canary().expect("native panic canary environment")
    }

    /// Panics with the private canary as an ordinary string payload.
    pub(super) fn panic_with_canary() -> ! {
        panic!("{}", required_canary())
    }

    struct DestructorPanic(String);

    impl Drop for DestructorPanic {
        fn drop(&mut self) {
            panic!("{}", self.0)
        }
    }

    /// Panics with an opaque payload whose destructor panics with the canary.
    pub(super) fn panic_with_destructor_canary() -> ! {
        std::panic::panic_any(DestructorPanic(required_canary()))
    }

    /// Runs one ignored child test under the real crash hook with a fresh
    /// private canary and returns the sorted codes of its crash reports. A
    /// report that is not the closed redacted form counts as `unredacted`;
    /// report and stream contents are never formatted.
    pub(super) fn redacted_report_codes(test: &str) -> Vec<String> {
        let nonce = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("audiograph-native-panic-{nonce}"));
        let canary = format!("private native payload {nonce}");
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", test, "--ignored", "--nocapture"])
            .env(crate::user_data::DATA_DIR_ENV, &root)
            .env(CANARY_ENV, &canary)
            .env("RUST_BACKTRACE", "0")
            .output()
            .expect("run native panic child");
        let reports = read_reports(&root.join("crashes"));
        let _ = std::fs::remove_dir_all(&root);

        assert!(output.status.success(), "child status {}", output.status);
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains(&canary),
            "canary reached child stdout"
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).contains(&canary),
            "canary reached child stderr"
        );
        let mut codes: Vec<String> = reports
            .expect("child crash reports")
            .iter()
            .map(|report| {
                report
                    .strip_prefix(REPORT_PREFIX)
                    .and_then(|rest| rest.strip_suffix(REPORT_SUFFIX))
                    .filter(|code| {
                        code.bytes()
                            .all(|byte| byte.is_ascii_lowercase() || matches!(byte, b'.' | b'_'))
                    })
                    .unwrap_or("unredacted")
                    .to_owned()
            })
            .collect();
        codes.sort();
        codes
    }

    fn read_reports(directory: &Path) -> std::io::Result<Vec<String>> {
        std::fs::read_dir(directory)?
            .map(|entry| std::fs::read_to_string(entry?.path()))
            .collect()
    }
}
