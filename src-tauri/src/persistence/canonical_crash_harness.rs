//! Linux subprocess evidence for dormant canonical durability and recovery.
//!
//! This module is compiled only into the library test binary. Handshakes are
//! versioned stage tokens and never include managed paths or fixture bytes.

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

#[cfg(all(test, target_os = "linux"))]
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
    use std::os::unix::process::ExitStatusExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitStatus, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
    use std::thread::JoinHandle;

    const ROOT_ENV: &str = "AUDIO_GRAPH_B77B_ROOT";
    const CHILD_TEST: &str =
        "persistence::canonical_crash_harness::tests::subprocess_child_entrypoint";
    const CHILD_TIMEOUT: Duration = Duration::from_secs(10);
    const SESSION: &str = "session-1";
    const STREAM: &str = "transcript";
    const SCHEMA: u32 = 1;
    const SOURCE_IDENTITY: &str = "streams/events.jsonl";
    const TEMP_IDENTITY: &str = "recovery/events.recovery.tmp";
    const QUARANTINE_IDENTITY: &str = "recovery/events.recovery.bin";
    const SOURCE_PREFIX: &[u8] = b"{\"value\":1}\n";
    const SOURCE_TAIL: &[u8] = b"private incomplete tail";
    const FIRST_CREATE_BYTES: &[u8] = b"first-create-proof";
    const CONTINUE_FILE: &str = ".audio-graph-b77b-continue";

    const RECOVERY_CUTS: &[&str] = &[
        "quarantine_create_before",
        "quarantine_create_after",
        "quarantine_write_before",
        "quarantine_write_after",
        "quarantine_flush_before",
        "quarantine_flush_after",
        "quarantine_file_sync_before",
        "quarantine_file_sync_after",
        "quarantine_rename_before",
        "quarantine_rename_after",
        "quarantine_parent_sync_before",
        "quarantine_parent_sync_after",
        "manifest_prepared_before",
        "manifest_prepared_after",
        "source_truncate_before",
        "source_truncate_after",
        "source_sync_before",
        "source_sync_after",
        "manifest_completed_before",
        "manifest_completed_after",
        "acknowledgement_before",
        "acknowledgement_after",
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
        reader: Option<JoinHandle<()>>,
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
                    Ok(Some(status)) => {
                        self.finish_reader();
                        return status;
                    }
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
                        let status = self.force_kill_and_wait();
                        panic!("poll bounded child failed: {error}; cleanup status={status:?}");
                    }
                }
            }
        }

        fn kill_and_wait(&mut self) -> ExitStatus {
            let status = match self.child.try_wait() {
                Ok(Some(status)) => status,
                Ok(None) => {
                    return self.force_kill_and_wait();
                }
                Err(_) => return self.force_kill_and_wait(),
            };
            self.finish_reader();
            status
        }

        fn force_kill_and_wait(&mut self) -> ExitStatus {
            let _ = self.child.kill();
            let status = self
                .child
                .wait()
                .expect("wait for bounded subprocess child");
            self.finish_reader();
            status
        }

        fn finish_reader(&mut self) {
            if let Some(reader) = self.reader.take() {
                reader.join().expect("join subprocess stdout reader");
            }
        }
    }

    impl Drop for ManagedChild {
        fn drop(&mut self) {
            if !matches!(self.child.try_wait(), Ok(Some(_))) {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
            self.finish_reader();
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
        let reader = std::thread::spawn(move || {
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
        });
        ManagedChild {
            child,
            lines,
            reader: Some(reader),
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
        let source = [SOURCE_PREFIX, SOURCE_TAIL].concat();
        fs::write(source_path(root), &source).expect("write damaged synthetic stream");

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
                        content: content_identity(&source),
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

    fn retry_recovery(root: &Path) {
        assert!(matches!(
            execute_recovery(root),
            CanonicalRecoveryOutcome::Accepted(_) | CanonicalRecoveryOutcome::AlreadyCompleted(_)
        ));
        emit_handshake("retry_ok");
    }

    #[derive(Debug)]
    struct ObservedFile {
        length: u64,
        sha256: String,
    }

    #[derive(Debug)]
    struct RecoveryObservation {
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
        RecoveryObservation {
            generation: manifest.generation,
            source: observe_file(&source_path(root)).expect("source evidence must be present"),
            temporary: observe_file(&temporary_path(root)),
            quarantine: observe_file(&quarantine_path(root)),
        }
    }

    fn expected_generation_at_cut(cut: &str) -> u64 {
        match cut {
            "manifest_completed_after" | "acknowledgement_before" | "acknowledgement_after" => 3,
            "manifest_prepared_after"
            | "source_truncate_before"
            | "source_truncate_after"
            | "source_sync_before"
            | "source_sync_after"
            | "manifest_completed_before" => 2,
            _ => 1,
        }
    }

    fn source_is_truncated_at_cut(cut: &str) -> bool {
        matches!(
            cut,
            "source_truncate_after"
                | "source_sync_before"
                | "source_sync_after"
                | "manifest_completed_before"
                | "manifest_completed_after"
                | "acknowledgement_before"
                | "acknowledgement_after"
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
            "retry" => retry_recovery(&child_root()),
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
            "reader_inode" => {
                let root = child_root();
                let _guard = CanonicalDurability::new()
                    .try_lock_shared(&root)
                    .expect("reader inode shared guard");
                let path = root.join("data.bin");
                let mut file = File::open(&path).expect("open reader inode");
                await_parent_signal(&root, "reader_inode_open");
                file.seek(SeekFrom::Start(0)).expect("rewind reader inode");
                let mut held = Vec::new();
                file.read_to_end(&mut held).expect("read retained inode");
                assert_eq!(held, b"old-inode");
                assert_eq!(fs::read(path).expect("read replacement path"), b"new-inode");
                emit_handshake("reader_inode_stable");
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
    fn recovery_crash_cuts_reopen_and_retry_idempotently_in_fresh_processes() {
        for cut in RECOVERY_CUTS {
            let root = TempRoot::new(cut);
            setup_recovery_fixture(root.path());

            let mut recovery = spawn_root_child("recover", root.path(), Some(cut));
            recovery.wait_for(&handshake(cut));
            let killed = recovery.kill_and_wait();
            assert_eq!(killed.signal(), Some(9), "cut {cut}");

            let residual = observe_recovery(root.path());
            assert_eq!(
                residual.generation,
                expected_generation_at_cut(cut),
                "cut {cut}"
            );
            let full_source = [SOURCE_PREFIX, SOURCE_TAIL].concat();
            let expected_source = if source_is_truncated_at_cut(cut) {
                SOURCE_PREFIX
            } else {
                full_source.as_slice()
            };
            assert_eq!(residual.source.sha256, digest(expected_source).as_str());
            if source_is_truncated_at_cut(cut) {
                assert_eq!(
                    residual
                        .quarantine
                        .as_ref()
                        .expect("truncation requires published quarantine")
                        .sha256,
                    digest(SOURCE_TAIL).as_str()
                );
            }

            let mut retry = spawn_root_child("retry", root.path(), None);
            retry.wait_for(&handshake("retry_ok"));
            assert!(retry.wait().success(), "retry cut {cut}");

            let mut oracle = spawn_root_child("oracle", root.path(), None);
            oracle.wait_for(&handshake("oracle_ok"));
            assert!(oracle.wait().success(), "oracle cut {cut}");
            let final_state = observe_recovery(root.path());
            let recovery_file = final_state
                .quarantine
                .as_ref()
                .expect("final quarantine evidence");
            println!(
                "AUDIO_GRAPH_B77B_EVIDENCE_V1 cut={cut} child=signal9 reopen=pass retry=pass generation={} source_length={} source_sha256={} recovery_length={} recovery_sha256={} temp_present={}",
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
    fn first_create_distinguishes_file_sync_from_parent_namespace_sync() {
        for cut in FIRST_CREATE_CUTS {
            let root = TempRoot::new(cut);
            let mut creator = spawn_root_child("first_create", root.path(), Some(cut));
            creator.wait_for(&handshake(cut));
            let killed = creator.kill_and_wait();
            assert_eq!(killed.signal(), Some(9), "cut {cut}");

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
        assert_eq!(exclusive.kill_and_wait().signal(), Some(9));
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
        assert_eq!(shared.kill_and_wait().signal(), Some(9));
        drop(
            durability
                .try_lock_exclusive(root.path())
                .expect("killed shared holder releases lock"),
        );
    }

    #[test]
    fn strict_reader_participates_and_open_inode_survives_uncooperative_raw_rename() {
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
        assert_eq!(reader.kill_and_wait().signal(), Some(9));

        let inode_root = TempRoot::new("reader-inode");
        fs::write(inode_root.path().join("data.bin"), b"old-inode").expect("seed reader inode");
        provision_coordination(inode_root.path());
        let mut inode_reader = spawn_root_child("reader_inode", inode_root.path(), None);
        inode_reader.wait_for(&handshake("reader_inode_open"));
        fs::rename(
            inode_root.path().join("data.bin"),
            inode_root.path().join("moved.bin"),
        )
        .expect("uncooperative raw rename remains possible under advisory lock");
        fs::write(inode_root.path().join("data.bin"), b"new-inode")
            .expect("install replacement inode");
        fs::write(inode_root.path().join(CONTINUE_FILE), b"go").expect("signal reader inode child");
        inode_reader.wait_for(&handshake("reader_inode_stable"));
        assert!(inode_reader.wait().success());
    }

    #[test]
    fn rename_refusals_and_stable_coordination_identity_cross_processes() {
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
}
