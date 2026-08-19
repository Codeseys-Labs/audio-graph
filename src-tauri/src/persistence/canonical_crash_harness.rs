//! Native subprocess evidence for dormant canonical durability and recovery.
//!
//! This module is compiled only into the library test binary. Handshakes are
//! versioned stage tokens and never include managed paths or fixture bytes.
//! Linux and qualified macOS/APFS fixtures exercise the accepted namespace
//! barriers. Windows/NTFS exercises the typed pre-mutation refusal boundary.
//!
//! Fixture filesystem evidence comes from a platform probe, and one of those
//! probes needs a privilege the test process may not hold: Windows
//! `fsutil fsinfo volumeinfo` requires an elevated process and answers
//! "Access is denied." otherwise. Absent evidence is therefore reported as its
//! own outcome — `ProbedFilesystem::Unavailable` with a reason token — and is
//! never folded into "observed a filesystem that is not the required one".
//! Where the proof under test does not depend on the filesystem name, the test
//! continues and downgrades only its evidence claim; where it does, the caller
//! reports the unavailable outcome instead of asserting.

use std::io::Write;
use std::time::{Duration, Instant};

const CHILD_ACTION_ENV: &str = "AUDIO_GRAPH_B77B_CHILD_ACTION";
const CHECKPOINT_ENV: &str = "AUDIO_GRAPH_B77B_CHECKPOINT";
const HANDSHAKE_PREFIX: &str = "AUDIO_GRAPH_B77B_HANDSHAKE_V1";

fn emit_handshake(token: &str) {
    println!("{HANDSHAKE_PREFIX}:{token}");
    std::io::stdout()
        .flush()
        .expect("flush content-free subprocess handshake");
}

/// Stop a test child at an ordered persistence boundary until its parent
/// kills it. A bounded self-timeout prevents an orphan if the parent itself
/// disappears before its RAII cleanup runs.
pub(crate) fn checkpoint(stage: &str) {
    if std::env::var_os(CHILD_ACTION_ENV).is_none()
        || std::env::var(CHECKPOINT_ENV).as_deref() != Ok(stage)
    {
        return;
    }
    emit_handshake(stage);
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("subprocess checkpoint exceeded its bounded parent-kill window");
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod tests {
    use super::*;
    use crate::persistence::canonical_durability::{
        CanonicalCoordinationError, CanonicalDurability, CanonicalDurabilityOutcome,
        CanonicalDurabilityRejection, CanonicalFilesystemQualification, CanonicalRecoveryKey,
    };
    use crate::persistence::canonical_log::{
        CanonicalLogSnapshot, CanonicalRecoveryDescriptor, CanonicalRecoveryOutcome,
        CanonicalRecoveryTransaction, CanonicalTailRecovery, load_canonical_stream,
    };
    use crate::persistence::session_artifact_manifest::{
        ArtifactAvailability, ArtifactContentIdentity, ArtifactPrivacyClass,
        ArtifactUnavailableReason, ManagedArtifactIdentity, ManifestCasOutcome,
        ManifestLoadOutcome, ManifestTransition, ManifestTransitionState, SessionArtifactEntry,
        SessionArtifactKind, SessionArtifactManifestStore, SessionArtifactManifestV1, Sha256Digest,
    };
    use serde::{Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use std::fs::{self, File};
    use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError};

    const ROOT_ENV: &str = "AUDIO_GRAPH_B77B_ROOT";
    const CHILD_TEST: &str =
        "persistence::canonical_crash_harness::tests::subprocess_child_entrypoint";
    const CHILD_TIMEOUT: Duration = Duration::from_secs(10);
    const DROP_REAP_TIMEOUT: Duration = Duration::from_secs(2);
    const SESSION: &str = "session-1";
    const STREAM: &str = "transcript";
    const SCHEMA: u32 = 1;
    const SOURCE_IDENTITY: &str = "streams/events.jsonl";
    const TEMP_IDENTITY: &str = "recovery/events.recovery.tmp";
    const QUARANTINE_IDENTITY: &str = "recovery/events.recovery.bin";
    const SOURCE_PREFIX: &[u8] = b"{\"value\":1}\n";
    const SOURCE_TAIL: &[u8] = b"private incomplete tail";
    const SOURCE_FULL: &[u8] = b"{\"value\":1}\nprivate incomplete tail";
    const FIRST_CREATE_BYTES: &[u8] = b"first-create-proof";
    const CONTINUE_FILE: &str = ".audio-graph-b77b-continue";

    #[cfg(target_os = "windows")]
    use crate::persistence::canonical_durability::{
        CanonicalNamespaceOperation, CanonicalPlatform, CanonicalSnapshotExpectation,
    };

    /// Reason tokens for a probe that produced no filesystem evidence. They are
    /// content-free: no command line, no volume, no path.
    const WINDOWS_PROBE_REQUIRES_ELEVATION: &str = "fsutil-access-denied-requires-elevation";
    const WINDOWS_PROBE_EXIT_REFUSED: &str = "fsutil-exit-refused";
    const WINDOWS_PROBE_UNPARSABLE: &str = "fsutil-named-no-filesystem";

    /// What one fixture filesystem probe produced.
    ///
    /// The two variants are not interchangeable. `Observed` is a measurement of
    /// the live fixture volume; `Unavailable` says the probe never got to
    /// measure. Collapsing the second into a sentinel observation is what made
    /// a non-elevated Windows run look like a fixture on the wrong filesystem.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum ProbedFilesystem {
        Observed(String),
        Unavailable(&'static str),
    }

    #[derive(Debug)]
    struct FixtureFilesystemEvidence {
        queried_path: PathBuf,
        platform: &'static str,
        expected_filesystem: &'static str,
        observed: ProbedFilesystem,
        detail: String,
    }

    impl FixtureFilesystemEvidence {
        fn is_qualified(&self) -> bool {
            match &self.observed {
                ProbedFilesystem::Observed(filesystem) => {
                    filesystem.eq_ignore_ascii_case(self.expected_filesystem)
                }
                ProbedFilesystem::Unavailable(_) => false,
            }
        }

        fn unavailable_reason(&self) -> Option<&'static str> {
            match self.observed {
                ProbedFilesystem::Observed(_) => None,
                ProbedFilesystem::Unavailable(reason) => Some(reason),
            }
        }

        /// The `observed=` field of the evidence line: the measured filesystem,
        /// or the literal `unavailable` when there was nothing to measure.
        fn observed_report(&self) -> &str {
            match &self.observed {
                ProbedFilesystem::Observed(filesystem) => filesystem,
                ProbedFilesystem::Unavailable(_) => "unavailable",
            }
        }
    }

    /// What the fixture filesystem contract concluded for one fixture root.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FixtureContract {
        /// The probe ran and named the required filesystem.
        Qualified,
        /// The probe ran and named a filesystem the contract does not accept,
        /// on a platform whose `ALLOW_EXPLICIT_FIXTURE_REFUSAL` permits saying so.
        RefusedFilesystem,
        /// The probe produced no evidence, on a platform where that is a
        /// possible outcome of a correctly configured host — Windows `fsutil`
        /// without elevation. The reason token says which case it was.
        EvidenceUnavailable(&'static str),
    }

    /// Classify one completed `fsutil fsinfo volumeinfo` run.
    ///
    /// Kept out of the Windows-only `platform` module and free of process
    /// spawning so every host compiles and tests it. A non-elevated `fsutil`
    /// answers "Access is denied." and names no filesystem, which is absent
    /// evidence and is classified as such before the exit status is consulted,
    /// because that is the case whose reason a reader needs to see.
    fn classify_windows_filesystem_probe(succeeded: bool, output: &str) -> ProbedFilesystem {
        if output.to_ascii_lowercase().contains("access is denied") {
            return ProbedFilesystem::Unavailable(WINDOWS_PROBE_REQUIRES_ELEVATION);
        }
        if !succeeded {
            return ProbedFilesystem::Unavailable(WINDOWS_PROBE_EXIT_REFUSED);
        }
        match parse_windows_filesystem_output(output) {
            Some(filesystem) => ProbedFilesystem::Observed(filesystem),
            None => ProbedFilesystem::Unavailable(WINDOWS_PROBE_UNPARSABLE),
        }
    }

    fn parse_macos_filesystem_plist(output: &str) -> Option<String> {
        let key = "<key>FilesystemType</key>";
        let after_key = output.split_once(key)?.1;
        let after_open = after_key.split_once("<string>")?.1;
        let value = after_open.split_once("</string>")?.0.trim();
        (!value.is_empty()).then(|| value.to_owned())
    }

    fn parse_windows_filesystem_output(output: &str) -> Option<String> {
        output.split_whitespace().find_map(|token| {
            let normalized =
                token.trim_matches(|character: char| !character.is_ascii_alphanumeric());
            ["NTFS", "ReFS", "exFAT", "FAT32", "FAT"]
                .into_iter()
                .find(|candidate| normalized.eq_ignore_ascii_case(candidate))
                .map(str::to_owned)
        })
    }

    #[cfg(target_os = "linux")]
    mod platform {
        use super::*;
        use std::os::unix::process::ExitStatusExt;

        pub(super) const ALLOW_EXPLICIT_FIXTURE_REFUSAL: bool = false;
        /// `findmnt` and `stat -f` are unprivileged, so absent evidence here is
        /// a broken host rather than an expected outcome — no reason qualifies.
        pub(super) fn unavailable_reason_is_expected(_reason: &str) -> bool {
            false
        }
        const LINUX_PROBE_REFUSED: &str = "findmnt-refused";

        pub(super) fn assert_forced_termination(status: ExitStatus, context: &str) {
            assert_eq!(status.signal(), Some(9), "{context}");
        }

        pub(super) fn fixture_filesystem_evidence(path: &Path) -> FixtureFilesystemEvidence {
            let findmnt = Command::new("findmnt")
                .arg("-T")
                .arg(path)
                .args(["-o", "FSTYPE", "-n"])
                .output()
                .expect("run findmnt for live fixture parent");
            let statfs = Command::new("stat")
                .args([
                    "-f",
                    "-c",
                    "filesystem_type=%T block_size=%S blocks=%b free_blocks=%f name_max=%l",
                ])
                .arg(path)
                .output()
                .expect("run stat -f for live fixture parent");
            let observed = if findmnt.status.success() {
                ProbedFilesystem::Observed(
                    String::from_utf8(findmnt.stdout)
                        .expect("findmnt evidence is UTF-8")
                        .trim()
                        .to_owned(),
                )
            } else {
                ProbedFilesystem::Unavailable(LINUX_PROBE_REFUSED)
            };
            // `stat -f` is unprivileged here, so its failure is a broken host,
            // not an outcome. Reporting it as a detail sentinel would let half
            // this evidence go missing under `outcome=qualified`.
            assert!(
                statfs.status.success(),
                "stat -f fixture query failed on a platform where it needs no privilege"
            );
            FixtureFilesystemEvidence {
                queried_path: path.to_path_buf(),
                platform: "linux",
                expected_filesystem: "ext4",
                observed,
                detail: String::from_utf8(statfs.stdout)
                    .expect("stat -f evidence is UTF-8")
                    .trim()
                    .to_owned(),
            }
        }
    }

    #[cfg(target_os = "macos")]
    mod platform {
        use super::*;
        use std::os::unix::process::ExitStatusExt;

        pub(super) const ALLOW_EXPLICIT_FIXTURE_REFUSAL: bool = true;
        /// `df` and `diskutil info` are unprivileged; this platform already
        /// tolerates an explicit refusal, so it needs no exemption of its own.
        pub(super) fn unavailable_reason_is_expected(_reason: &str) -> bool {
            false
        }
        const MACOS_PROBE_REFUSED: &str = "filesystem-probe-refused";

        pub(super) fn assert_forced_termination(status: ExitStatus, context: &str) {
            assert_eq!(status.signal(), Some(9), "{context}");
        }

        pub(super) fn fixture_filesystem_evidence(path: &Path) -> FixtureFilesystemEvidence {
            let df = Command::new("/bin/df")
                .arg("-P")
                .arg(path)
                .output()
                .expect("run df for live fixture parent");
            let df_output = String::from_utf8_lossy(&df.stdout);
            let mount_point = df_output
                .lines()
                .last()
                .and_then(|line| line.split_whitespace().last());
            let diskutil = mount_point.and_then(|mount_point| {
                let output = Command::new("/usr/sbin/diskutil")
                    .args(["info", "-plist", mount_point])
                    .output()
                    .expect("run diskutil for live fixture mount");
                output.status.success().then_some(output)
            });
            let observed = diskutil
                .as_ref()
                .and_then(|output| {
                    parse_macos_filesystem_plist(&String::from_utf8_lossy(&output.stdout))
                })
                .map_or(
                    ProbedFilesystem::Unavailable(MACOS_PROBE_REFUSED),
                    ProbedFilesystem::Observed,
                );
            let probed = observed != ProbedFilesystem::Unavailable(MACOS_PROBE_REFUSED);
            FixtureFilesystemEvidence {
                queried_path: path.to_path_buf(),
                platform: "macos",
                expected_filesystem: "apfs",
                observed,
                detail: if df.status.success() && probed {
                    "df-and-diskutil-ok".to_owned()
                } else {
                    MACOS_PROBE_REFUSED.to_owned()
                },
            }
        }
    }

    #[cfg(target_os = "windows")]
    mod platform {
        use super::*;
        use std::path::{Component, Prefix};

        pub(super) const ALLOW_EXPLICIT_FIXTURE_REFUSAL: bool = false;
        /// `fsutil fsinfo volumeinfo` requires an elevated process. A developer
        /// running the suite from an ordinary shell gets "Access is denied." and
        /// no filesystem name, so THAT absence is an expected outcome here and
        /// must not be reported as a failed filesystem contract. CI runners are
        /// administrative, which is why this only ever bit local runs.
        ///
        /// Only the elevation reason is exempt. A missing `fsutil`, a fixture
        /// path with no volume prefix, or output this harness cannot parse are
        /// all broken-probe conditions that no privilege explains, so they still
        /// fail rather than degrading into silent unavailable evidence.
        pub(super) fn unavailable_reason_is_expected(reason: &str) -> bool {
            reason == WINDOWS_PROBE_REQUIRES_ELEVATION
        }
        const WINDOWS_PROBE_NO_VOLUME: &str = "fixture-path-has-no-volume-prefix";
        const WINDOWS_PROBE_SPAWN_REFUSED: &str = "fsutil-spawn-refused";

        pub(super) fn assert_forced_termination(status: ExitStatus, context: &str) {
            assert!(
                !status.success() && status.code().is_some(),
                "{context}: TerminateProcess must yield a reaped unsuccessful child, got {status:?}"
            );
        }

        fn volume_argument(path: &Path) -> Option<String> {
            let canonical = fs::canonicalize(path).ok()?;
            let Component::Prefix(prefix) = canonical.components().next()? else {
                return None;
            };
            match prefix.kind() {
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                    Some(format!("{}:", char::from(letter)))
                }
                _ => None,
            }
        }

        pub(super) fn fixture_filesystem_evidence(path: &Path) -> FixtureFilesystemEvidence {
            let observed = match volume_argument(path) {
                None => ProbedFilesystem::Unavailable(WINDOWS_PROBE_NO_VOLUME),
                Some(volume) => match Command::new("fsutil")
                    .args(["fsinfo", "volumeinfo", &volume])
                    .output()
                {
                    // "Access is denied." reaches stdout on some builds and
                    // stderr on others, so both streams are classified.
                    Ok(output) => classify_windows_filesystem_probe(
                        output.status.success(),
                        &[
                            String::from_utf8_lossy(&output.stdout),
                            String::from_utf8_lossy(&output.stderr),
                        ]
                        .join("\n"),
                    ),
                    Err(_) => ProbedFilesystem::Unavailable(WINDOWS_PROBE_SPAWN_REFUSED),
                },
            };
            let detail: &'static str = match &observed {
                ProbedFilesystem::Observed(_) => "fsutil-ok",
                ProbedFilesystem::Unavailable(reason) => *reason,
            };
            let detail = detail.to_owned();
            FixtureFilesystemEvidence {
                queried_path: path.to_path_buf(),
                platform: "windows",
                expected_filesystem: "ntfs",
                observed,
                detail,
            }
        }
    }

    /// Probe the fixture parent and report the contract outcome.
    ///
    /// Three outcomes, kept distinct on purpose. A probe that ran and named the
    /// wrong filesystem still fails hard wherever the platform does not permit
    /// an explicit refusal — that is a fixture on an unsupported volume. A probe
    /// that produced no evidence fails hard unless that specific reason is one a
    /// correctly configured host can produce (`unavailable_reason_is_expected`,
    /// which on Windows admits only the missing-elevation reason); such a reason
    /// is reported as `unavailable` and handed back to the caller to interpret,
    /// while a broken probe on any platform still fails.
    fn fixture_contract_is_qualified(path: &Path, context: &str) -> FixtureContract {
        let evidence = platform::fixture_filesystem_evidence(path);
        assert_eq!(evidence.queried_path, path);
        let qualified = evidence.is_qualified();
        let unavailable = evidence.unavailable_reason();
        let outcome = match (qualified, unavailable) {
            (true, _) => "qualified",
            (false, None) => "refused",
            (false, Some(_)) => "unavailable",
        };
        println!(
            "AUDIO_GRAPH_67D3_FIXTURE_FS_V1 platform={} expected={} observed={} outcome={} detail={} context={context}",
            evidence.platform,
            evidence.expected_filesystem,
            evidence.observed_report(),
            outcome,
            evidence.detail,
        );
        if qualified {
            return FixtureContract::Qualified;
        }
        if let Some(reason) = unavailable {
            // Keyed on the specific reason, not on a per-platform boolean: only
            // the reasons a correctly configured host can produce are tolerated,
            // so a broken probe (missing binary, unexpected path shape) still
            // fails even on the platform that has one expected absence.
            if !platform::unavailable_reason_is_expected(reason)
                && !platform::ALLOW_EXPLICIT_FIXTURE_REFUSAL
            {
                panic!(
                    "{context}: the fixture filesystem probe produced no evidence for a reason no \
                     correctly configured host explains, so this host is misconfigured: \
                     platform={}, expected={}, detail={reason}",
                    evidence.platform, evidence.expected_filesystem,
                );
            }
            return FixtureContract::EvidenceUnavailable(reason);
        }
        if !platform::ALLOW_EXPLICIT_FIXTURE_REFUSAL {
            panic!(
                "{context}: required filesystem contract refused: platform={}, expected={}, \
                 observed={}",
                evidence.platform,
                evidence.expected_filesystem,
                evidence.observed_report(),
            );
        }
        FixtureContract::RefusedFilesystem
    }

    #[cfg(target_os = "macos")]
    fn macos_fixture_is_qualified(path: &Path, context: &str) -> bool {
        fixture_contract_is_qualified(path, context) == FixtureContract::Qualified
    }

    #[cfg(not(target_os = "macos"))]
    fn macos_fixture_is_qualified(_path: &Path, _context: &str) -> bool {
        true
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ObservedManifestPhase {
        SeedCompleted,
        RecoveryPrepared,
        RecoveryCompleted,
    }

    #[derive(Clone, Copy)]
    struct ExpectedResidual {
        phase: ObservedManifestPhase,
        generation: u64,
        source: &'static [u8],
        temporary: Option<&'static [u8]>,
        quarantine: Option<&'static [u8]>,
    }

    #[derive(Clone, Copy)]
    struct RecoveryCase {
        checkpoint: &'static str,
        residual: ExpectedResidual,
    }

    const RECOVERY_CASES: &[RecoveryCase] = &[
        RecoveryCase {
            checkpoint: "quarantine_create_before",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::SeedCompleted,
                generation: 1,
                source: SOURCE_FULL,
                temporary: None,
                quarantine: None,
            },
        },
        RecoveryCase {
            checkpoint: "quarantine_create_after",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::SeedCompleted,
                generation: 1,
                source: SOURCE_FULL,
                temporary: Some(b""),
                quarantine: None,
            },
        },
        RecoveryCase {
            checkpoint: "quarantine_write_before",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::SeedCompleted,
                generation: 1,
                source: SOURCE_FULL,
                temporary: Some(b""),
                quarantine: None,
            },
        },
        RecoveryCase {
            checkpoint: "quarantine_write_after",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::SeedCompleted,
                generation: 1,
                source: SOURCE_FULL,
                temporary: Some(SOURCE_TAIL),
                quarantine: None,
            },
        },
        RecoveryCase {
            checkpoint: "quarantine_flush_before",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::SeedCompleted,
                generation: 1,
                source: SOURCE_FULL,
                temporary: Some(SOURCE_TAIL),
                quarantine: None,
            },
        },
        RecoveryCase {
            checkpoint: "quarantine_flush_after",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::SeedCompleted,
                generation: 1,
                source: SOURCE_FULL,
                temporary: Some(SOURCE_TAIL),
                quarantine: None,
            },
        },
        RecoveryCase {
            checkpoint: "quarantine_file_sync_before",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::SeedCompleted,
                generation: 1,
                source: SOURCE_FULL,
                temporary: Some(SOURCE_TAIL),
                quarantine: None,
            },
        },
        RecoveryCase {
            checkpoint: "quarantine_file_sync_after",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::SeedCompleted,
                generation: 1,
                source: SOURCE_FULL,
                temporary: Some(SOURCE_TAIL),
                quarantine: None,
            },
        },
        RecoveryCase {
            checkpoint: "quarantine_rename_before",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::SeedCompleted,
                generation: 1,
                source: SOURCE_FULL,
                temporary: Some(SOURCE_TAIL),
                quarantine: None,
            },
        },
        RecoveryCase {
            checkpoint: "quarantine_rename_after",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::SeedCompleted,
                generation: 1,
                source: SOURCE_FULL,
                temporary: None,
                quarantine: Some(SOURCE_TAIL),
            },
        },
        RecoveryCase {
            checkpoint: "quarantine_parent_sync_before",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::SeedCompleted,
                generation: 1,
                source: SOURCE_FULL,
                temporary: None,
                quarantine: Some(SOURCE_TAIL),
            },
        },
        RecoveryCase {
            checkpoint: "quarantine_parent_sync_after",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::SeedCompleted,
                generation: 1,
                source: SOURCE_FULL,
                temporary: None,
                quarantine: Some(SOURCE_TAIL),
            },
        },
        RecoveryCase {
            checkpoint: "manifest_prepared_before",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::SeedCompleted,
                generation: 1,
                source: SOURCE_FULL,
                temporary: None,
                quarantine: Some(SOURCE_TAIL),
            },
        },
        RecoveryCase {
            checkpoint: "manifest_prepared_after",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::RecoveryPrepared,
                generation: 2,
                source: SOURCE_FULL,
                temporary: None,
                quarantine: Some(SOURCE_TAIL),
            },
        },
        RecoveryCase {
            checkpoint: "source_truncate_before",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::RecoveryPrepared,
                generation: 2,
                source: SOURCE_FULL,
                temporary: None,
                quarantine: Some(SOURCE_TAIL),
            },
        },
        RecoveryCase {
            checkpoint: "source_truncate_after",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::RecoveryPrepared,
                generation: 2,
                source: SOURCE_PREFIX,
                temporary: None,
                quarantine: Some(SOURCE_TAIL),
            },
        },
        RecoveryCase {
            checkpoint: "source_sync_before",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::RecoveryPrepared,
                generation: 2,
                source: SOURCE_PREFIX,
                temporary: None,
                quarantine: Some(SOURCE_TAIL),
            },
        },
        RecoveryCase {
            checkpoint: "source_sync_after",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::RecoveryPrepared,
                generation: 2,
                source: SOURCE_PREFIX,
                temporary: None,
                quarantine: Some(SOURCE_TAIL),
            },
        },
        RecoveryCase {
            checkpoint: "manifest_completed_before",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::RecoveryPrepared,
                generation: 2,
                source: SOURCE_PREFIX,
                temporary: None,
                quarantine: Some(SOURCE_TAIL),
            },
        },
        RecoveryCase {
            checkpoint: "manifest_completed_after",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::RecoveryCompleted,
                generation: 3,
                source: SOURCE_PREFIX,
                temporary: None,
                quarantine: Some(SOURCE_TAIL),
            },
        },
        RecoveryCase {
            checkpoint: "acknowledgement_before",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::RecoveryCompleted,
                generation: 3,
                source: SOURCE_PREFIX,
                temporary: None,
                quarantine: Some(SOURCE_TAIL),
            },
        },
        RecoveryCase {
            checkpoint: "acknowledgement_after",
            residual: ExpectedResidual {
                phase: ObservedManifestPhase::RecoveryCompleted,
                generation: 3,
                source: SOURCE_PREFIX,
                temporary: None,
                quarantine: Some(SOURCE_TAIL),
            },
        },
    ];

    const FIRST_CREATE_CUTS: &[&str] = &[
        "first_create_file_sync_before",
        "first_create_file_sync_after",
        "first_create_parent_sync_before",
        "first_create_parent_sync_after",
    ];

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TestPayload {
        value: u64,
    }

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            static NONCE: AtomicU64 = AtomicU64::new(0);
            let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "audio-graph-b77b-{label}-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create subprocess fixture root");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct ManagedChild {
        child: Child,
        lines: Receiver<String>,
        transcript: Vec<String>,
    }

    impl ManagedChild {
        fn wait_for(&mut self, expected: &str) {
            let deadline = Instant::now() + CHILD_TIMEOUT;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                match self.lines.recv_timeout(remaining) {
                    Ok(line) if line.ends_with(expected) => {
                        self.transcript.push(line);
                        return;
                    }
                    Ok(line) => self.transcript.push(line),
                    Err(RecvTimeoutError::Disconnected | RecvTimeoutError::Timeout) => {
                        let status = self.kill_and_wait();
                        panic!(
                            "bounded child handshake missing: expected={expected:?}, status={status:?}, transcript={:?}",
                            self.transcript
                        );
                    }
                }
            }
        }

        fn wait(&mut self) -> ExitStatus {
            let deadline = Instant::now() + CHILD_TIMEOUT;
            loop {
                match self.child.try_wait() {
                    Ok(Some(status)) => return status,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Ok(None) => {
                        let status = self.kill_and_wait();
                        panic!(
                            "bounded child exit timed out: status={status:?}, transcript={:?}",
                            self.transcript
                        );
                    }
                    Err(error) => {
                        let cleanup = self.kill_and_reap(CHILD_TIMEOUT);
                        panic!("poll bounded child failed: {error}; cleanup={cleanup:?}");
                    }
                }
            }
        }

        fn kill_and_wait(&mut self) -> ExitStatus {
            self.kill_and_reap(CHILD_TIMEOUT)
                .expect("checked kill and bounded subprocess reap")
        }

        fn kill_and_reap(&mut self, timeout: Duration) -> Result<ExitStatus, String> {
            match self.child.try_wait() {
                Ok(Some(status)) => return Ok(status),
                Ok(None) => {}
                Err(error) => {
                    self.child.kill().map_err(|kill_error| {
                        format!(
                            "initial child poll failed ({error}); checked kill failed ({kill_error})"
                        )
                    })?;
                    return self.reap_until(Instant::now() + timeout);
                }
            }
            if let Err(kill_error) = self.child.kill() {
                return match self.child.try_wait() {
                    Ok(Some(status)) => Ok(status),
                    Ok(None) => Err(format!("checked child kill failed ({kill_error})")),
                    Err(poll_error) => Err(format!(
                        "checked child kill failed ({kill_error}); follow-up poll failed ({poll_error})"
                    )),
                };
            }
            self.reap_until(Instant::now() + timeout)
        }

        fn reap_until(&mut self, deadline: Instant) -> Result<ExitStatus, String> {
            loop {
                match self.child.try_wait() {
                    Ok(Some(status)) => return Ok(status),
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Ok(None) => return Err("bounded child reap deadline expired".to_owned()),
                    Err(error) => return Err(format!("bounded child reap poll failed ({error})")),
                }
            }
        }
    }

    impl Drop for ManagedChild {
        fn drop(&mut self) {
            if let Err(error) = self.kill_and_reap(DROP_REAP_TIMEOUT) {
                eprintln!("AUDIO_GRAPH_67D3_CHILD_CLEANUP_V1 outcome=failed detail={error}");
            }
        }
    }

    fn spawn_child(action: &str) -> ManagedChild {
        spawn_child_with(action, None, None)
    }

    fn spawn_root_child(action: &str, root: &Path, checkpoint: Option<&str>) -> ManagedChild {
        spawn_child_with(action, Some(root), checkpoint)
    }

    fn spawn_child_with(
        action: &str,
        root: Option<&Path>,
        checkpoint: Option<&str>,
    ) -> ManagedChild {
        let mut command = Command::new(std::env::current_exe().expect("resolve lib test binary"));
        command
            .arg("--exact")
            .arg(CHILD_TEST)
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(CHILD_ACTION_ENV, action)
            .env_remove(CHECKPOINT_ENV)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(root) = root {
            command.env(ROOT_ENV, root);
        }
        if let Some(checkpoint) = checkpoint {
            command.env(CHECKPOINT_ENV, checkpoint);
        }
        let mut child = command.spawn().expect("spawn exact lib test child");
        let stdout = child.stdout.take().expect("capture child stdout");
        let (sender, lines) = mpsc::channel();
        drop(std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if sender.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        }));
        ManagedChild {
            child,
            lines,
            transcript: Vec::new(),
        }
    }

    fn handshake(token: &str) -> String {
        format!("{HANDSHAKE_PREFIX}:{token}")
    }

    fn child_root() -> PathBuf {
        std::env::var_os(ROOT_ENV)
            .map(PathBuf::from)
            .expect("subprocess action requires an opaque fixture root")
    }

    fn await_parent_signal(root: &Path, token: &str) {
        emit_handshake(token);
        let signal = root.join(CONTINUE_FILE);
        let deadline = Instant::now() + CHILD_TIMEOUT;
        while Instant::now() < deadline {
            if signal.is_file() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("bounded subprocess continuation signal was not received");
    }

    fn digest(bytes: &[u8]) -> Sha256Digest {
        Sha256Digest::new(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
            .expect("construct synthetic fixture digest")
    }

    fn content_identity(bytes: &[u8]) -> ArtifactContentIdentity {
        ArtifactContentIdentity {
            sha256: digest(bytes),
            byte_length: u64::try_from(bytes.len()).expect("fixture length fits u64"),
        }
    }

    fn source_path(root: &Path) -> PathBuf {
        root.join(SOURCE_IDENTITY)
    }

    fn temporary_path(root: &Path) -> PathBuf {
        root.join(TEMP_IDENTITY)
    }

    fn quarantine_path(root: &Path) -> PathBuf {
        root.join(QUARANTINE_IDENTITY)
    }

    fn recovery_descriptor() -> CanonicalRecoveryDescriptor {
        CanonicalRecoveryDescriptor::new(
            SESSION,
            STREAM,
            SCHEMA,
            "attempt-1",
            digest(b"attempt-1"),
            ManagedArtifactIdentity::new(SOURCE_IDENTITY).expect("source identity"),
            ManagedArtifactIdentity::new(TEMP_IDENTITY).expect("temporary identity"),
            ManagedArtifactIdentity::new(QUARANTINE_IDENTITY).expect("quarantine identity"),
        )
        .expect("construct recovery descriptor")
    }

    fn setup_recovery_fixture(root: &Path) {
        fs::create_dir_all(root.join("streams")).expect("create stream fixture directory");
        fs::create_dir_all(root.join("recovery")).expect("create recovery fixture directory");
        fs::write(source_path(root), SOURCE_FULL).expect("write damaged synthetic stream");

        let store =
            SessionArtifactManifestStore::qualified_for_test(root).expect("qualify fixture root");
        let candidate = SessionArtifactManifestV1::candidate(
            SESSION,
            ManifestTransition {
                idempotency_id: "seed-manifest".to_owned(),
                fingerprint: digest(b"seed-manifest"),
                state: ManifestTransitionState::Completed,
            },
            vec![
                SessionArtifactEntry {
                    kind: SessionArtifactKind::OriginalSessionAudio,
                    privacy_class: ArtifactPrivacyClass::OriginalEvidence,
                    managed_identity: ManagedArtifactIdentity::new("audio/original.wav")
                        .expect("audio identity"),
                    availability: ArtifactAvailability::Unavailable {
                        reason: ArtifactUnavailableReason::NeverCaptured,
                    },
                },
                SessionArtifactEntry {
                    kind: SessionArtifactKind::TranscriptRevisions,
                    privacy_class: ArtifactPrivacyClass::CanonicalSessionMemory,
                    managed_identity: ManagedArtifactIdentity::new(SOURCE_IDENTITY)
                        .expect("source identity"),
                    availability: ArtifactAvailability::Present {
                        content: content_identity(SOURCE_FULL),
                    },
                },
            ],
            None,
        )
        .expect("construct seed manifest");
        let mut write = store.begin_write().expect("begin seed manifest write");
        assert!(matches!(
            write.compare_and_swap(0, candidate),
            ManifestCasOutcome::Accepted { .. }
        ));
    }

    fn execute_recovery(root: &Path) -> CanonicalRecoveryOutcome {
        let store =
            SessionArtifactManifestStore::qualified_for_test(root).expect("reopen qualified store");
        let mut recovery =
            CanonicalRecoveryTransaction::begin::<TestPayload>(&store, recovery_descriptor())
                .expect("begin subprocess recovery");
        recovery.execute()
    }

    fn converge_recovery(root: &Path) {
        match execute_recovery(root) {
            CanonicalRecoveryOutcome::Accepted(_) => emit_handshake("retry_accepted"),
            CanonicalRecoveryOutcome::AlreadyCompleted(_) => {
                emit_handshake("retry_already_completed");
            }
            outcome => panic!("fresh convergence returned {outcome:?}"),
        }
    }

    fn exact_completed_retry(root: &Path) {
        let receipt = match execute_recovery(root) {
            CanonicalRecoveryOutcome::AlreadyCompleted(receipt) => receipt,
            outcome => panic!("second fresh retry must be AlreadyCompleted, got {outcome:?}"),
        };
        assert_eq!(receipt.manifest_generation, 3);
        assert_eq!(receipt.retained_bytes, SOURCE_PREFIX.len() as u64);
        assert_eq!(receipt.quarantined_bytes, SOURCE_TAIL.len() as u64);
        assert_final_recovery(root);
        emit_handshake("second_retry_already_completed");
    }

    #[derive(Debug)]
    struct ObservedFile {
        length: u64,
        sha256: String,
    }

    #[derive(Debug)]
    struct RecoveryObservation {
        phase: ObservedManifestPhase,
        generation: u64,
        source: ObservedFile,
        temporary: Option<ObservedFile>,
        quarantine: Option<ObservedFile>,
    }

    fn observe_file(path: &Path) -> Option<ObservedFile> {
        match fs::read(path) {
            Ok(bytes) => Some(ObservedFile {
                length: u64::try_from(bytes.len()).expect("fixture length fits u64"),
                sha256: digest(&bytes).as_str().to_owned(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => panic!("observe synthetic fixture file: {error}"),
        }
    }

    fn observe_recovery(root: &Path) -> RecoveryObservation {
        let store = SessionArtifactManifestStore::new(root);
        let ManifestLoadOutcome::Present(manifest) = store.load().expect("load manifest evidence")
        else {
            panic!("manifest evidence must be present");
        };
        let phase = match manifest.quarantine_transaction.as_ref() {
            None => {
                assert_eq!(
                    manifest.transition.state,
                    ManifestTransitionState::Completed
                );
                ObservedManifestPhase::SeedCompleted
            }
            Some(transaction) => {
                assert_eq!(manifest.transition.state, transaction.state);
                match transaction.state {
                    ManifestTransitionState::Prepared => ObservedManifestPhase::RecoveryPrepared,
                    ManifestTransitionState::Completed => ObservedManifestPhase::RecoveryCompleted,
                }
            }
        };
        RecoveryObservation {
            phase,
            generation: manifest.generation,
            source: observe_file(&source_path(root)).expect("source evidence must be present"),
            temporary: observe_file(&temporary_path(root)),
            quarantine: observe_file(&quarantine_path(root)),
        }
    }

    fn assert_file_residual(
        cut: &str,
        label: &str,
        observed: Option<&ObservedFile>,
        expected: Option<&[u8]>,
    ) {
        match (observed, expected) {
            (None, None) => {}
            (Some(observed), Some(expected)) => {
                assert_eq!(
                    observed.length,
                    expected.len() as u64,
                    "{cut}: {label} length"
                );
                assert_eq!(
                    observed.sha256,
                    digest(expected).as_str(),
                    "{cut}: {label} hash"
                );
            }
            (Some(_), None) => panic!("{cut}: unexpected {label} entry"),
            (None, Some(_)) => panic!("{cut}: missing {label} entry"),
        }
    }

    fn assert_expected_residual(case: RecoveryCase, observed: &RecoveryObservation) {
        assert_eq!(
            observed.phase, case.residual.phase,
            "{}: manifest phase",
            case.checkpoint
        );
        assert_eq!(
            observed.generation, case.residual.generation,
            "{}: manifest generation",
            case.checkpoint
        );
        assert_file_residual(
            case.checkpoint,
            "source",
            Some(&observed.source),
            Some(case.residual.source),
        );
        assert_file_residual(
            case.checkpoint,
            "temporary quarantine",
            observed.temporary.as_ref(),
            case.residual.temporary,
        );
        assert_file_residual(
            case.checkpoint,
            "final quarantine",
            observed.quarantine.as_ref(),
            case.residual.quarantine,
        );
    }

    fn optional_file_summary(file: Option<&ObservedFile>) -> String {
        file.map_or_else(
            || "absent".to_owned(),
            |file| format!("length={},sha256={}", file.length, file.sha256),
        )
    }

    fn assert_final_recovery(root: &Path) {
        let durability = CanonicalDurability::new();
        let _reader = durability
            .try_lock_shared(root)
            .expect("strict oracle participates in the shared coordination lock");
        let store = SessionArtifactManifestStore::new(root);
        let ManifestLoadOutcome::Present(manifest) = store.load().expect("fresh manifest reopen")
        else {
            panic!("completed manifest must be present");
        };
        assert_eq!(manifest.generation, 3);
        assert_eq!(
            manifest.transition.state,
            ManifestTransitionState::Completed
        );
        let quarantine = manifest
            .quarantine_transaction
            .as_ref()
            .expect("completed quarantine transaction");
        assert_eq!(
            quarantine.source_after.content,
            content_identity(SOURCE_PREFIX)
        );
        assert_eq!(quarantine.quarantine.content, content_identity(SOURCE_TAIL));
        assert_eq!(
            fs::read(source_path(root)).expect("read retained source"),
            SOURCE_PREFIX
        );
        assert_eq!(
            fs::read(quarantine_path(root)).expect("read exact quarantine"),
            SOURCE_TAIL
        );
        assert!(!temporary_path(root).exists());
        let loaded: CanonicalLogSnapshot<TestPayload> = load_canonical_stream(
            &source_path(root),
            SESSION,
            STREAM,
            SCHEMA,
            CanonicalTailRecovery::Strict,
        )
        .expect("strict fresh-process canonical reopen");
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.records[0].payload, TestPayload { value: 1 });
    }

    fn provision_coordination(root: &Path) {
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(root)
            .expect("provision stable coordination entry");
        drop(guard);
    }

    fn child_action(action: &str) {
        match action {
            "seam" => emit_handshake("seam"),
            "recover" => {
                let root = child_root();
                assert!(matches!(
                    execute_recovery(&root),
                    CanonicalRecoveryOutcome::Accepted(_)
                ));
                checkpoint("acknowledgement_after");
                panic!("recovery child returned without reaching its requested checkpoint");
            }
            "retry_converge" => converge_recovery(&child_root()),
            "retry_exact_completed" => exact_completed_retry(&child_root()),
            "oracle" => {
                assert_final_recovery(&child_root());
                emit_handshake("oracle_ok");
            }
            "first_create" => {
                let root = child_root();
                let proof = CanonicalFilesystemQualification::for_test_root(&root)
                    .expect("bind first-create qualification");
                let guard = CanonicalDurability::new()
                    .try_lock_exclusive(&root)
                    .expect("lock first-create root");
                let outcome = guard.append(
                    &root.join("new-entry.bin"),
                    FIRST_CREATE_BYTES,
                    Some(&proof),
                    CanonicalRecoveryKey::from_opaque_bytes([5; 16]),
                );
                assert!(matches!(outcome, CanonicalDurabilityOutcome::Accepted(_)));
                panic!("first-create child returned without reaching its requested checkpoint");
            }
            "first_create_oracle" => {
                let root = child_root();
                assert_eq!(
                    fs::read(root.join("new-entry.bin")).expect("reopen first-created entry"),
                    FIRST_CREATE_BYTES
                );
                emit_handshake("first_create_oracle_ok");
            }
            "hold_exclusive" => {
                let root = child_root();
                let _guard = CanonicalDurability::new()
                    .try_lock_exclusive(&root)
                    .expect("hold exclusive coordination guard");
                checkpoint("exclusive_held");
                unreachable!("exclusive holder must be killed");
            }
            "hold_shared" => {
                let root = child_root();
                let _guard = CanonicalDurability::new()
                    .try_lock_shared(&root)
                    .expect("hold shared coordination guard");
                checkpoint("shared_held");
                unreachable!("shared holder must be killed");
            }
            "strict_reader" => {
                let root = child_root();
                let _guard = CanonicalDurability::new()
                    .try_lock_shared(&root)
                    .expect("strict reader shared guard");
                let loaded: CanonicalLogSnapshot<TestPayload> = load_canonical_stream(
                    &root.join("events.jsonl"),
                    SESSION,
                    STREAM,
                    SCHEMA,
                    CanonicalTailRecovery::Strict,
                )
                .expect("strict guarded child read");
                assert_eq!(loaded.records.len(), 1);
                checkpoint("strict_reader_held");
                unreachable!("strict reader must be killed");
            }
            "reader_handle" => {
                let root = child_root();
                let _guard = CanonicalDurability::new()
                    .try_lock_shared(&root)
                    .expect("reader handle shared guard");
                let path = root.join("data.bin");
                let mut file = File::open(&path).expect("open reader handle");
                await_parent_signal(&root, "reader_handle_open");
                file.seek(SeekFrom::Start(0)).expect("rewind reader handle");
                let mut held = Vec::new();
                file.read_to_end(&mut held).expect("read retained handle");
                assert_eq!(held, b"old-handle");
                assert_eq!(
                    fs::read(path).expect("read replacement path"),
                    b"new-handle"
                );
                emit_handshake("reader_handle_stable");
            }
            "try_exclusive_contended" => {
                assert!(matches!(
                    CanonicalDurability::new().try_lock_exclusive(&child_root()),
                    Err(CanonicalCoordinationError::Contended)
                ));
                emit_handshake("exclusive_contended_ok");
            }
            "try_exclusive_acquired" => {
                let root = child_root();
                let _guard = CanonicalDurability::new()
                    .try_lock_exclusive(&root)
                    .expect("exclusive lock released across process boundary");
                emit_handshake("exclusive_acquired_ok");
            }
            "rename_refusals" => {
                let root = child_root();
                let proof = CanonicalFilesystemQualification::for_test_root(&root)
                    .expect("bind rename qualification");
                let guard = CanonicalDurability::new()
                    .try_lock_exclusive(&root)
                    .expect("lock rename refusal root");
                let key = CanonicalRecoveryKey::from_opaque_bytes([6; 16]);
                assert_eq!(
                    guard.rename(
                        &root.join("source.bin"),
                        &root.join("collision.bin"),
                        Some(&proof),
                        key,
                    ),
                    CanonicalDurabilityOutcome::Rejected(
                        CanonicalDurabilityRejection::DestinationAlreadyExists
                    )
                );
                assert_eq!(
                    guard.rename(
                        &root.join("source.bin"),
                        &root.join("nested/destination.bin"),
                        Some(&proof),
                        key,
                    ),
                    CanonicalDurabilityOutcome::Rejected(
                        CanonicalDurabilityRejection::TargetOutsideManagedNamespace
                    )
                );
                emit_handshake("rename_refusals_ok");
            }
            _ => panic!("unknown content-free subprocess action"),
        }
    }

    #[test]
    fn subprocess_child_entrypoint() {
        let Some(action) = std::env::var_os(CHILD_ACTION_ENV) else {
            return;
        };
        child_action(action.to_str().expect("child action token is ASCII"));
    }

    #[test]
    fn public_subprocess_harness_seam_self_spawns_exact_child() {
        let mut child = spawn_child("seam");
        child.wait_for(&handshake("seam"));
        assert!(child.wait().success());
    }

    #[test]
    fn kill_and_reap_paths_are_deadline_bounded_and_check_kill() {
        let source = include_str!("canonical_crash_harness.rs");
        let ignored_kill = ["let _ = self.child.", "kill()"].concat();
        let unbounded_wait = ["self.child.", "wait()"].concat();
        assert!(!source.contains(&ignored_kill));
        assert!(!source.contains(&unbounded_wait));
    }

    #[test]
    fn dropping_a_live_holder_is_bounded_and_releases_its_lock() {
        let root = TempRoot::new("drop-reap");
        provision_coordination(root.path());
        let started = Instant::now();
        {
            let mut holder =
                spawn_root_child("hold_exclusive", root.path(), Some("exclusive_held"));
            holder.wait_for(&handshake("exclusive_held"));
        }
        assert!(started.elapsed() < DROP_REAP_TIMEOUT + Duration::from_secs(1));
        drop(
            CanonicalDurability::new()
                .try_lock_exclusive(root.path())
                .expect("bounded Drop reaps the holder and releases its lock"),
        );
    }

    #[test]
    fn every_crash_checkpoint_pair_brackets_its_unique_operation() {
        fn unique_offset(scope: &str, needle: &str, label: &str) -> usize {
            let mut offsets = scope.match_indices(needle).map(|(offset, _)| offset);
            let offset = offsets
                .next()
                .unwrap_or_else(|| panic!("{label}: missing unique marker {needle:?}"));
            assert!(
                offsets.next().is_none(),
                "{label}: marker is not unique in owning slice: {needle:?}"
            );
            offset
        }

        fn owning_slice<'a>(source: &'a str, start: &str, end: &str, label: &str) -> &'a str {
            let start = unique_offset(source, start, label);
            let end = unique_offset(source, end, label);
            assert!(start < end, "{label}: owning slice markers are reversed");
            &source[start..end]
        }

        fn assert_bracketed(scope: &str, label: &str, before: &str, operation: &str, after: &str) {
            let before = unique_offset(scope, before, label);
            let operation = unique_offset(scope, operation, label);
            let after = unique_offset(scope, after, label);
            assert!(
                before < operation && operation < after,
                "{label}: checkpoints do not bracket their concrete operation"
            );
        }

        fn assert_before(scope: &str, label: &str, before: &str, operation: &str) {
            assert!(
                unique_offset(scope, before, label) < unique_offset(scope, operation, label),
                "{label}: before checkpoint follows its concrete operation"
            );
        }

        fn assert_after(scope: &str, label: &str, operation: &str, after: &str) {
            assert!(
                unique_offset(scope, operation, label) < unique_offset(scope, after, label),
                "{label}: after checkpoint precedes its concrete operation"
            );
        }

        let log = include_str!("canonical_log.rs");
        let publish = owning_slice(
            log,
            "    fn publish_quarantine(&mut self) -> Option<CanonicalRecoveryOutcome> {",
            "    fn manifest_candidate(",
            "publish quarantine",
        );
        for (label, before, operation, after) in [
            (
                "quarantine create",
                "quarantine_create_before",
                "open_or_resume_recovery_temporary(&self.temporary_path, &self.tail)",
                "quarantine_create_after",
            ),
            (
                "quarantine write",
                "quarantine_write_before",
                "temporary.write_all(remaining)",
                "quarantine_write_after",
            ),
            (
                "quarantine flush",
                "quarantine_flush_before",
                "temporary.flush()",
                "quarantine_flush_after",
            ),
            (
                "quarantine file sync",
                "quarantine_file_sync_before",
                "temporary.sync_all()",
                "quarantine_file_sync_after",
            ),
        ] {
            assert_bracketed(publish, label, before, operation, after);
        }

        let execute = owning_slice(
            log,
            "    pub fn execute(&mut self) -> CanonicalRecoveryOutcome {",
            "    #[cfg(test)]\n    fn fail_once_at",
            "execute recovery",
        );
        for (label, before, operation, after) in [
            (
                "Prepared manifest CAS",
                "manifest_prepared_before",
                "self.compare_manifest_recovery(prepared, true)",
                "manifest_prepared_after",
            ),
            (
                "source truncate",
                "source_truncate_before",
                "self.source.set_len(self.source_after.content.byte_length)",
                "source_truncate_after",
            ),
            (
                "source sync",
                "source_sync_before",
                "self.source.sync_all()",
                "source_sync_after",
            ),
            (
                "Completed manifest CAS",
                "manifest_completed_before",
                "self.compare_manifest_recovery(completed, false)",
                "manifest_completed_after",
            ),
        ] {
            assert_bracketed(execute, label, before, operation, after);
        }
        assert_before(
            execute,
            "acknowledgement before return",
            "acknowledgement_before",
            "CanonicalRecoveryOutcome::Accepted(self.receipt())",
        );

        let durability = include_str!("canonical_durability.rs");
        let rename = owning_slice(
            durability,
            "    fn rename_inner(",
            "    /// Atomically install a complete snapshot under this guard.",
            "recovery rename",
        );
        for (label, before, operation, after) in [
            (
                "recovery rename",
                "quarantine_rename_before",
                "self.rename_source(source, destination)",
                "quarantine_rename_after",
            ),
            (
                "recovery parent sync",
                "quarantine_parent_sync_before",
                "source_parent_directory.sync_all()",
                "quarantine_parent_sync_after",
            ),
        ] {
            assert_bracketed(rename, label, before, operation, after);
        }

        let append = owning_slice(
            durability,
            "    fn append_opened(",
            "    fn open_existing_regular(",
            "append opened",
        );
        for (label, before, operation, after) in [
            (
                "first-create file sync",
                "first_create_file_sync_before",
                "self.checked(CanonicalDurabilityStage::FileSync, || file.sync_all())",
                "first_create_file_sync_after",
            ),
            (
                "first-create parent sync",
                "first_create_parent_sync_before",
                "self.checked(CanonicalDurabilityStage::ParentSync, || parent.sync_all())",
                "first_create_parent_sync_after",
            ),
        ] {
            assert_bracketed(append, label, before, operation, after);
        }

        let harness = include_str!("canonical_crash_harness.rs");
        let recover_start = ["\"", "recover", "\" => {"].concat();
        let recover_end = ["\"", "retry_", "converge", "\" =>"].concat();
        let recover = owning_slice(
            harness,
            &recover_start,
            &recover_end,
            "recovery child action",
        );
        assert_after(
            recover,
            "acknowledgement after return",
            "execute_recovery(&root)",
            "acknowledgement_after",
        );
    }

    /// Two claims, and both hold on every host without any privilege.
    ///
    /// First, the parsers and the Windows probe classifier map probe text to the
    /// right outcome — in particular a non-elevated "Access is denied." is
    /// absent evidence naming elevation, not an observation of a non-NTFS
    /// volume. Second, the live probe of a real fixture parent is bound to the
    /// path it was asked about, and its outcome is reported for what it is. Only
    /// the second claim depends on the host, and it degrades to an explicit
    /// unavailable-evidence line rather than to a bare failure.
    #[test]
    fn filesystem_evidence_is_bound_to_the_live_fixture_parent() {
        assert_eq!(
            parse_macos_filesystem_plist(
                "<plist><dict><key>FilesystemType</key><string>apfs</string></dict></plist>"
            )
            .as_deref(),
            Some("apfs")
        );
        assert_eq!(
            parse_windows_filesystem_output("File System Name : NTFS").as_deref(),
            Some("NTFS")
        );
        assert_eq!(parse_windows_filesystem_output("no filesystem token"), None);

        assert_eq!(
            classify_windows_filesystem_probe(true, "File System Name : NTFS"),
            ProbedFilesystem::Observed("NTFS".to_owned())
        );
        for (succeeded, output) in [(false, "Access is denied."), (true, "Access is denied.")] {
            assert_eq!(
                classify_windows_filesystem_probe(succeeded, output),
                ProbedFilesystem::Unavailable(WINDOWS_PROBE_REQUIRES_ELEVATION),
                "a denied probe is absent evidence whatever the exit status"
            );
        }
        assert_eq!(
            classify_windows_filesystem_probe(false, "some other refusal"),
            ProbedFilesystem::Unavailable(WINDOWS_PROBE_EXIT_REFUSED)
        );
        assert_eq!(
            classify_windows_filesystem_probe(true, "no filesystem token"),
            ProbedFilesystem::Unavailable(WINDOWS_PROBE_UNPARSABLE)
        );
        assert_eq!(
            classify_windows_filesystem_probe(true, "File System Name : exFAT"),
            ProbedFilesystem::Observed("exFAT".to_owned()),
            "a probe that names a non-required filesystem is an observation, not a gap"
        );

        // The preceding AUDIO_GRAPH_67D3_FIXTURE_FS_V1 line carries the platform,
        // the expectation, and the observation for this same context, so the
        // typed outcome line only has to name the conclusion.
        let root = TempRoot::new("fixture-mount-evidence");
        match fixture_contract_is_qualified(root.path(), "filesystem-evidence") {
            FixtureContract::Qualified => {}
            FixtureContract::RefusedFilesystem => println!(
                "AUDIO_GRAPH_67D3_TYPED_TEST_OUTCOME_V1 test=filesystem-evidence outcome=refused_wrong_filesystem"
            ),
            FixtureContract::EvidenceUnavailable(reason) => println!(
                "AUDIO_GRAPH_67D3_TYPED_TEST_OUTCOME_V1 test=filesystem-evidence outcome=unavailable_evidence detail={reason}"
            ),
        }
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn recovery_crash_cuts_reopen_and_retry_idempotently_in_fresh_processes() {
        let qualification_root = TempRoot::new("recovery-platform-qualification");
        if !macos_fixture_is_qualified(qualification_root.path(), "recovery-crash-cuts") {
            println!(
                "AUDIO_GRAPH_67D3_TYPED_TEST_OUTCOME_V1 platform=macos test=recovery-crash-cuts outcome=refused_not_apfs"
            );
            return;
        }
        for case in RECOVERY_CASES {
            let cut = case.checkpoint;
            let root = TempRoot::new(cut);
            setup_recovery_fixture(root.path());

            let mut recovery = spawn_root_child("recover", root.path(), Some(cut));
            recovery.wait_for(&handshake(cut));
            let killed = recovery.kill_and_wait();
            platform::assert_forced_termination(killed, &format!("cut {cut}"));

            let residual = observe_recovery(root.path());
            assert_expected_residual(*case, &residual);

            let first_retry_token =
                if case.residual.phase == ObservedManifestPhase::RecoveryCompleted {
                    "retry_already_completed"
                } else {
                    "retry_accepted"
                };
            let mut retry = spawn_root_child("retry_converge", root.path(), None);
            retry.wait_for(&handshake(first_retry_token));
            assert!(retry.wait().success(), "first retry cut {cut}");

            let mut second_retry = spawn_root_child("retry_exact_completed", root.path(), None);
            second_retry.wait_for(&handshake("second_retry_already_completed"));
            assert!(second_retry.wait().success(), "second retry cut {cut}");

            let mut oracle = spawn_root_child("oracle", root.path(), None);
            oracle.wait_for(&handshake("oracle_ok"));
            assert!(oracle.wait().success(), "oracle cut {cut}");
            let final_state = observe_recovery(root.path());
            let recovery_file = final_state
                .quarantine
                .as_ref()
                .expect("final quarantine evidence");
            println!(
                "AUDIO_GRAPH_B77B_EVIDENCE_V2 cut={cut} child=signal9 crash_phase={:?} crash_generation={} crash_source=length={},sha256={} crash_temp={} crash_final={} first_retry={} second_retry=already_completed reopen=pass generation={} source_length={} source_sha256={} recovery_length={} recovery_sha256={} temp_present={}",
                residual.phase,
                residual.generation,
                residual.source.length,
                residual.source.sha256,
                optional_file_summary(residual.temporary.as_ref()),
                optional_file_summary(residual.quarantine.as_ref()),
                first_retry_token,
                final_state.generation,
                final_state.source.length,
                final_state.source.sha256,
                recovery_file.length,
                recovery_file.sha256,
                final_state.temporary.is_some(),
            );
        }
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn first_create_distinguishes_file_sync_from_parent_namespace_sync() {
        let qualification_root = TempRoot::new("first-create-platform-qualification");
        if !macos_fixture_is_qualified(qualification_root.path(), "first-create-cuts") {
            println!(
                "AUDIO_GRAPH_67D3_TYPED_TEST_OUTCOME_V1 platform=macos test=first-create-cuts outcome=refused_not_apfs"
            );
            return;
        }
        for cut in FIRST_CREATE_CUTS {
            let root = TempRoot::new(cut);
            let mut creator = spawn_root_child("first_create", root.path(), Some(cut));
            creator.wait_for(&handshake(cut));
            let killed = creator.kill_and_wait();
            platform::assert_forced_termination(killed, &format!("cut {cut}"));

            let mut oracle = spawn_root_child("first_create_oracle", root.path(), None);
            oracle.wait_for(&handshake("first_create_oracle_ok"));
            assert!(oracle.wait().success(), "first-create oracle cut {cut}");
            let evidence = observe_file(&root.path().join("new-entry.bin"))
                .expect("first-created file evidence");
            println!(
                "AUDIO_GRAPH_B77B_FIRST_CREATE_V1 cut={cut} child=signal9 reopen=pass length={} sha256={}",
                evidence.length, evidence.sha256
            );
        }
    }

    #[test]
    fn cross_process_lock_matrix_and_killed_holder_release_are_explicit() {
        let root = TempRoot::new("lock-matrix");
        provision_coordination(root.path());
        let durability = CanonicalDurability::new();

        let mut exclusive = spawn_root_child("hold_exclusive", root.path(), Some("exclusive_held"));
        exclusive.wait_for(&handshake("exclusive_held"));
        assert!(matches!(
            durability.try_lock_exclusive(root.path()),
            Err(CanonicalCoordinationError::Contended)
        ));
        assert!(matches!(
            durability.try_lock_shared(root.path()),
            Err(CanonicalCoordinationError::Contended)
        ));
        platform::assert_forced_termination(exclusive.kill_and_wait(), "exclusive holder");
        drop(
            durability
                .try_lock_exclusive(root.path())
                .expect("killed exclusive holder releases lock"),
        );

        let mut shared = spawn_root_child("hold_shared", root.path(), Some("shared_held"));
        shared.wait_for(&handshake("shared_held"));
        assert!(matches!(
            durability.try_lock_exclusive(root.path()),
            Err(CanonicalCoordinationError::Contended)
        ));
        let second_shared = durability
            .try_lock_shared(root.path())
            .expect("shared and shared coexist across processes");
        drop(second_shared);
        platform::assert_forced_termination(shared.kill_and_wait(), "shared holder");
        drop(
            durability
                .try_lock_exclusive(root.path())
                .expect("killed shared holder releases lock"),
        );
    }

    #[test]
    fn strict_reader_participates_and_open_handle_survives_uncooperative_raw_rename() {
        let reader_root = TempRoot::new("strict-reader");
        fs::write(reader_root.path().join("events.jsonl"), SOURCE_PREFIX)
            .expect("seed strict reader stream");
        provision_coordination(reader_root.path());
        let mut reader = spawn_root_child(
            "strict_reader",
            reader_root.path(),
            Some("strict_reader_held"),
        );
        reader.wait_for(&handshake("strict_reader_held"));
        assert!(matches!(
            CanonicalDurability::new().try_lock_exclusive(reader_root.path()),
            Err(CanonicalCoordinationError::Contended)
        ));
        platform::assert_forced_termination(reader.kill_and_wait(), "strict reader");

        let handle_root = TempRoot::new("reader-handle");
        fs::write(handle_root.path().join("data.bin"), b"old-handle").expect("seed reader handle");
        provision_coordination(handle_root.path());
        let mut handle_reader = spawn_root_child("reader_handle", handle_root.path(), None);
        handle_reader.wait_for(&handshake("reader_handle_open"));
        fs::rename(
            handle_root.path().join("data.bin"),
            handle_root.path().join("moved.bin"),
        )
        .expect("uncooperative raw rename remains possible under advisory lock");
        fs::write(handle_root.path().join("data.bin"), b"new-handle")
            .expect("install replacement file");
        fs::write(handle_root.path().join(CONTINUE_FILE), b"go")
            .expect("signal reader handle child");
        handle_reader.wait_for(&handshake("reader_handle_stable"));
        assert!(handle_reader.wait().success());
        println!(
            "AUDIO_GRAPH_67D3_ADVISORY_LOCK_V1 outcome=observed limitation=cooperating_processes_only open_handle=stable raw_rename=outside_contract"
        );
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn rename_refusals_and_stable_coordination_identity_cross_processes() {
        let qualification_root = TempRoot::new("rename-platform-qualification");
        if !macos_fixture_is_qualified(qualification_root.path(), "rename-matrix") {
            println!(
                "AUDIO_GRAPH_67D3_TYPED_TEST_OUTCOME_V1 platform=macos test=rename-matrix outcome=refused_not_apfs"
            );
            return;
        }
        let refusal_root = TempRoot::new("rename-refusals");
        fs::create_dir_all(refusal_root.path().join("nested")).expect("create nested directory");
        fs::write(refusal_root.path().join("source.bin"), b"source").expect("seed rename source");
        fs::write(refusal_root.path().join("collision.bin"), b"collision")
            .expect("seed rename collision");
        let mut refusals = spawn_root_child("rename_refusals", refusal_root.path(), None);
        refusals.wait_for(&handshake("rename_refusals_ok"));
        assert!(refusals.wait().success());

        let stable_root = TempRoot::new("stable-lock-rename");
        fs::write(stable_root.path().join("before.bin"), b"stable")
            .expect("seed stable rename source");
        let proof = CanonicalFilesystemQualification::for_test_root(stable_root.path())
            .expect("bind stable rename qualification");
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(stable_root.path())
            .expect("hold stable coordination identity");
        assert!(matches!(
            guard.rename(
                &stable_root.path().join("before.bin"),
                &stable_root.path().join("after.bin"),
                Some(&proof),
                CanonicalRecoveryKey::from_opaque_bytes([8; 16]),
            ),
            CanonicalDurabilityOutcome::Accepted(_)
        ));
        let mut contender = spawn_root_child("try_exclusive_contended", stable_root.path(), None);
        contender.wait_for(&handshake("exclusive_contended_ok"));
        assert!(contender.wait().success());
        drop(guard);
        let mut successor = spawn_root_child("try_exclusive_acquired", stable_root.path(), None);
        successor.wait_for(&handshake("exclusive_acquired_ok"));
        assert!(successor.wait().success());
    }

    /// The refusals proven here come from platform policy, not from the volume:
    /// `namespace_supported_for` excludes Windows for every filesystem, so every
    /// assertion below holds on NTFS, ReFS, or exFAT alike. That is why a fixture
    /// volume whose filesystem could not be probed — the ordinary non-elevated
    /// case, since `fsutil` needs elevation — still runs the whole proof and
    /// gives up only the `filesystem=ntfs` claim in its evidence line. A probe
    /// that runs and names a non-NTFS volume is a different matter and still
    /// fails, inside `fixture_contract_is_qualified`.
    #[test]
    #[cfg(target_os = "windows")]
    fn windows_ntfs_namespace_paths_refuse_before_temp_head_or_source_mutation() {
        let root = TempRoot::new("windows-ntfs-refusal");
        let filesystem_evidence =
            match fixture_contract_is_qualified(root.path(), "windows-namespace-refusal") {
                FixtureContract::Qualified => "ntfs",
                FixtureContract::EvidenceUnavailable(reason) => reason,
                FixtureContract::RefusedFilesystem => panic!(
                    "this platform does not permit an explicit fixture refusal, so a probed \
                     non-NTFS volume must already have failed the contract"
                ),
            };
        fs::create_dir_all(root.path().join("streams")).expect("create stream fixture directory");
        fs::create_dir_all(root.path().join("recovery"))
            .expect("create recovery fixture directory");

        let source = root.path().join("streams/events.jsonl");
        let rename_destination = root.path().join("streams/events-renamed.jsonl");
        let first_create = root.path().join("first-create.jsonl");
        let recovery_temporary = root.path().join("recovery/events.tmp");
        let recovery_destination = root.path().join("recovery/events.bin");
        let snapshot_temporary = root.path().join("manifest.snapshot.tmp");
        let snapshot_head = root.path().join("manifest.snapshot.json");
        fs::write(&source, b"stable-source").expect("seed stable source");
        fs::write(&recovery_temporary, b"stable-recovery-temp").expect("seed stable recovery temp");
        fs::write(&snapshot_head, b"stable-head").expect("seed stable snapshot head");
        let expected_head = File::open(&snapshot_head).expect("open stable snapshot head");
        let key = CanonicalRecoveryKey::from_opaque_bytes([0x67; 16]);
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(root.path())
            .expect("acquire native Windows guard");

        assert_eq!(
            guard.append(&first_create, b"must-not-create", None, key),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Windows,
                    operation: CanonicalNamespaceOperation::FirstCreate,
                }
            )
        );
        assert_eq!(
            guard.rename(&source, &rename_destination, None, key),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Windows,
                    operation: CanonicalNamespaceOperation::Rename,
                }
            )
        );
        assert_eq!(
            guard.preflight_recovery_namespace(
                &source,
                &recovery_temporary,
                &recovery_destination,
                None,
            ),
            Err(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Windows,
                    operation: CanonicalNamespaceOperation::Rename,
                }
            )
        );
        assert_eq!(
            guard.rename_recovery(&recovery_temporary, &recovery_destination, None, key),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Windows,
                    operation: CanonicalNamespaceOperation::Rename,
                }
            )
        );
        assert_eq!(
            guard.install_snapshot(
                &snapshot_temporary,
                &snapshot_head,
                b"must-not-install",
                CanonicalSnapshotExpectation::Existing(&expected_head),
                None,
                key,
            ),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Windows,
                    operation: CanonicalNamespaceOperation::AtomicSnapshotInstall,
                }
            )
        );

        assert!(!first_create.exists());
        assert!(!rename_destination.exists());
        assert_eq!(fs::read(&source).expect("source remains"), b"stable-source");
        assert_eq!(
            fs::read(&recovery_temporary).expect("recovery temp remains"),
            b"stable-recovery-temp"
        );
        assert!(!recovery_destination.exists());
        assert!(!snapshot_temporary.exists());
        assert_eq!(
            fs::read(&snapshot_head).expect("snapshot head remains"),
            b"stable-head"
        );
        let mut contender = spawn_root_child("try_exclusive_contended", root.path(), None);
        contender.wait_for(&handshake("exclusive_contended_ok"));
        assert!(contender.wait().success());
        drop(guard);
        let mut successor = spawn_root_child("try_exclusive_acquired", root.path(), None);
        successor.wait_for(&handshake("exclusive_acquired_ok"));
        assert!(successor.wait().success());
        println!(
            "AUDIO_GRAPH_67D3_WINDOWS_REFUSAL_V1 filesystem={filesystem_evidence} first_create=refused rename=refused snapshot=refused recovery=refused mutation=none coordination_identity=stable advisory_lock=cooperating_processes_only"
        );
    }
}
