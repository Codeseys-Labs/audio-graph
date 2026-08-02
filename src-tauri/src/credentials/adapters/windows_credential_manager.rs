use super::super::super::keyring_entry::{EntryLocator, KeyringBoundary};
use super::super::super::windows_credential_manager_ffi::{
    DeleteFailure, PrepareFailure, RawCredentialStore, ReadFailure, WriteFailure, prepare_target,
    prepare_write,
};
use super::super::{ForbidPrompt, MutationInvocation, ensure_forbid_prompt};
use super::{
    CancellationToken, RecoveryBoundary, RecoveryFailure, RecoveryOutcome, RecoveryTarget,
    recovery_facade::AllowPrompt,
};
use crate::credentials::domain::CredentialStoreFailure;
use std::sync::Arc;

#[cfg(target_os = "windows")]
use super::super::super::windows_credential_manager_ffi::NativeWinCredStore;

pub(in crate::credentials::adapters) struct WindowsCredentialManagerBoundary {
    raw: Arc<dyn RawCredentialStore>,
}

impl WindowsCredentialManagerBoundary {
    fn from_raw(raw: Arc<dyn RawCredentialStore>) -> Self {
        Self { raw }
    }

    fn map_prepare_failure(failure: PrepareFailure) -> CredentialStoreFailure {
        match failure {
            PrepareFailure::InvalidTarget => CredentialStoreFailure::Internal,
            PrepareFailure::PayloadTooLarge => CredentialStoreFailure::PayloadTooLarge,
        }
    }

    fn map_read_failure(failure: ReadFailure) -> CredentialStoreFailure {
        match failure {
            ReadFailure::AccessDenied => CredentialStoreFailure::AccessDenied,
            ReadFailure::Missing => CredentialStoreFailure::Missing,
            ReadFailure::Cancelled => CredentialStoreFailure::Cancelled,
            ReadFailure::Unavailable => CredentialStoreFailure::Unavailable,
            ReadFailure::Internal => CredentialStoreFailure::Internal,
        }
    }
}

#[cfg(target_os = "windows")]
pub(in crate::credentials::adapters) fn production_keyring_boundary() -> Arc<dyn KeyringBoundary> {
    Arc::new(WindowsCredentialManagerBoundary::from_raw(Arc::new(
        NativeWinCredStore::new(),
    )))
}

#[cfg(target_os = "windows")]
pub(super) fn production_recovery_boundary() -> Box<dyn RecoveryBoundary> {
    Box::new(WindowsCredentialManagerBoundary::from_raw(Arc::new(
        NativeWinCredStore::new(),
    )))
}

impl KeyringBoundary for WindowsCredentialManagerBoundary {
    fn get_secret(
        &self,
        prompt: &ForbidPrompt<'_>,
        locator: &EntryLocator,
    ) -> Result<Vec<u8>, CredentialStoreFailure> {
        ensure_forbid_prompt(prompt)?;
        let target = prepare_target(locator.windows_target()).map_err(Self::map_prepare_failure)?;
        let mut secret = self.raw.read(target).map_err(Self::map_read_failure)?;
        Ok(std::mem::take(&mut *secret))
    }

    fn set_secret(
        &self,
        prompt: &ForbidPrompt<'_>,
        invocation: &mut MutationInvocation<'_>,
        locator: &EntryLocator,
        secret: &[u8],
    ) -> Result<(), CredentialStoreFailure> {
        ensure_forbid_prompt(prompt)?;
        let request =
            prepare_write(locator.windows_target(), secret).map_err(Self::map_prepare_failure)?;
        invocation.prepare()?;
        invocation.mark_started();
        match self.raw.write(request) {
            Ok(()) => Ok(()),
            Err(WriteFailure::AccessDenied) => Err(CredentialStoreFailure::AccessDenied),
            Err(WriteFailure::Cancelled) => Err(CredentialStoreFailure::Cancelled),
            Err(WriteFailure::Unavailable) => Err(CredentialStoreFailure::Unavailable),
            Err(WriteFailure::CommitUnknown) => Err(CredentialStoreFailure::CommitUnknown),
        }
    }

    fn delete_credential(
        &self,
        prompt: &ForbidPrompt<'_>,
        invocation: &mut MutationInvocation<'_>,
        locator: &EntryLocator,
    ) -> Result<(), CredentialStoreFailure> {
        ensure_forbid_prompt(prompt)?;
        let target = prepare_target(locator.windows_target()).map_err(Self::map_prepare_failure)?;
        invocation.prepare()?;
        invocation.mark_started();
        match self.raw.delete(target) {
            Ok(()) => Ok(()),
            Err(DeleteFailure::AccessDenied) => Err(CredentialStoreFailure::AccessDenied),
            Err(DeleteFailure::Missing) => Err(CredentialStoreFailure::Missing),
            Err(DeleteFailure::Cancelled) => Err(CredentialStoreFailure::Cancelled),
            Err(DeleteFailure::Unavailable) => Err(CredentialStoreFailure::Unavailable),
            Err(DeleteFailure::CommitUnknown) => Err(CredentialStoreFailure::CommitUnknown),
        }
    }
}

impl RecoveryBoundary for WindowsCredentialManagerBoundary {
    fn recover(
        &self,
        _prompt: &AllowPrompt<'_, '_>,
        target: RecoveryTarget,
        _cancellation: &CancellationToken,
    ) -> Result<(), RecoveryFailure> {
        match target {
            RecoveryTarget::CredentialStore => Ok(()),
        }
    }

    fn verify(
        &self,
        prompt: &ForbidPrompt<'_>,
        target: RecoveryTarget,
    ) -> Result<RecoveryOutcome, RecoveryFailure> {
        ensure_forbid_prompt(prompt).map_err(RecoveryFailure::Closed)?;
        let locator = match target {
            RecoveryTarget::CredentialStore => EntryLocator::authority(),
        };
        let prepared = prepare_target(locator.windows_target())
            .map_err(Self::map_prepare_failure)
            .map_err(RecoveryFailure::Closed)?;
        match self.raw.read(prepared) {
            Ok(_secret) => Ok(RecoveryOutcome::Ready),
            Err(ReadFailure::Missing) => Ok(RecoveryOutcome::Ready),
            Err(failure) => Err(RecoveryFailure::Closed(Self::map_read_failure(failure))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::super::keyring_entry::{
        EntryLocator, KeyringBoundary, KeyringEntryAdapter,
    };
    use super::super::super::super::native_interaction::NativeInteractionGate;
    use super::super::super::super::windows_credential_manager_ffi::test_support::{
        failing_mutation_store, masked_production_code, masked_rust_code, memory_store,
        production_code_with_strings, rust_code_with_strings, unknown_read_store,
    };
    use super::super::{CancellationToken, NativeRecoveryFacade, RecoveryOutcome, RecoveryTarget};
    use super::WindowsCredentialManagerBoundary;
    use crate::credentials::domain::CredentialStoreFailure;

    const RAW_SOURCE: &str = "credentials/adapters/windows_credential_manager_ffi.rs";
    const POLICY_SOURCE: &str = "credentials/adapters/windows_credential_manager.rs";

    #[derive(Debug, PartialEq, Eq)]
    struct SourceViolation {
        source: std::path::PathBuf,
        identifier: String,
    }

    fn record_violation(
        violations: &mut Vec<SourceViolation>,
        source: &std::path::Path,
        identifier: &str,
    ) {
        let violation = SourceViolation {
            source: source.to_path_buf(),
            identifier: identifier.to_owned(),
        };
        if !violations.contains(&violation) {
            violations.push(violation);
        }
    }

    fn production_rust_sources(root: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        let mut pending = vec![root.to_path_buf()];
        let mut sources = Vec::new();
        while let Some(directory) = pending.pop() {
            let mut entries = std::fs::read_dir(&directory)?.collect::<Result<Vec<_>, _>>()?;
            entries.sort_by_key(std::fs::DirEntry::path);
            for entry in entries {
                let path = entry.path();
                let file_type = entry.file_type()?;
                if file_type.is_dir() {
                    pending.push(path);
                } else if file_type.is_file()
                    && path.extension().and_then(std::ffi::OsStr::to_str) == Some("rs")
                {
                    sources.push(path);
                } else if file_type.is_symlink() {
                    return Err(std::io::Error::other("symlink in production source tree"));
                }
            }
        }
        sources.sort();
        Ok(sources)
    }

    fn is_direct_wincred_identifier(identifier: &str) -> bool {
        let native_pascal = identifier
            .strip_prefix("Cred")
            .and_then(|suffix| suffix.chars().next())
            .is_some_and(|character| character.is_ascii_uppercase());
        let native_upper = identifier.starts_with("CRED_")
            || identifier.starts_with("CREDUI_")
            || matches!(
                identifier,
                "CREDENTIALA"
                    | "CREDENTIALW"
                    | "CREDENTIAL_ATTRIBUTEA"
                    | "CREDENTIAL_ATTRIBUTEW"
                    | "CREDENTIAL_TARGET_INFORMATIONA"
                    | "CREDENTIAL_TARGET_INFORMATIONW"
            );
        native_pascal
            || native_upper
            || matches!(
                identifier,
                "cred_delete_w" | "cred_free" | "cred_read_w" | "cred_write_w"
            )
    }

    fn is_allowed_raw_wincred_identifier(identifier: &str) -> bool {
        matches!(
            identifier,
            "CREDENTIALW"
                | "CRED_FLAGS"
                | "CRED_PERSIST_LOCAL_MACHINE"
                | "CRED_TYPE"
                | "CRED_TYPE_GENERIC"
                | "CredDeleteW"
                | "CredFree"
                | "CredReadW"
                | "CredWriteW"
                | "cred_delete_w"
                | "cred_free"
                | "cred_read_w"
                | "cred_write_w"
        )
    }

    fn is_raw_boundary_identifier(identifier: &str) -> bool {
        matches!(
            identifier,
            "DeleteFailure"
                | "NativeWinCredStore"
                | "PrepareFailure"
                | "PreparedTarget"
                | "PreparedWrite"
                | "RawCredentialStore"
                | "ReadFailure"
                | "WriteFailure"
        )
    }

    fn identifier_count(source: &str, expected: &str) -> usize {
        source
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .filter(|identifier| *identifier == expected)
            .count()
    }

    fn contains_in_order(source: &str, expected: &[&str]) -> bool {
        let mut remainder = source;
        for item in expected {
            let Some(offset) = remainder.find(item) else {
                return false;
            };
            remainder = &remainder[offset + item.len()..];
        }
        true
    }

    fn safe_policy_raw_surface_is_exact(policy: &str) -> bool {
        let compact: String = policy
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        let get = compact
            .split_once("fnget_secret(")
            .and_then(|(_, tail)| tail.split_once("fnset_secret(").map(|(body, _)| body));
        let set = compact.split_once("fnset_secret(").and_then(|(_, tail)| {
            tail.split_once("fndelete_credential(")
                .map(|(body, _)| body)
        });
        let delete = compact
            .split_once("fndelete_credential(")
            .and_then(|(_, tail)| {
                tail.split_once("implRecoveryBoundary")
                    .map(|(body, _)| body)
            });
        let recovery = compact
            .split_once("fnrecover(")
            .and_then(|(_, tail)| tail.split_once("fnverify("));
        [
            ("DeleteFailure", 6),
            ("NativeWinCredStore", 3),
            ("PrepareFailure", 4),
            ("PreparedTarget", 0),
            ("PreparedWrite", 0),
            ("RawCredentialStore", 3),
            ("ReadFailure", 8),
            ("WriteFailure", 5),
            ("delete", 1),
            ("prepare_target", 4),
            ("prepare_write", 2),
            ("raw", 7),
            ("read", 2),
            ("write", 1),
        ]
        .into_iter()
        .all(|(identifier, expected)| identifier_count(policy, identifier) == expected)
            && !compact.contains('!')
            && compact.matches("NativeWinCredStore::new()").count() == 2
            && compact.matches("self.raw").count() == 4
            && compact.matches("self.raw.read(target)").count() == 1
            && compact.matches("self.raw.read(prepared)").count() == 1
            && compact.matches("self.raw.write(request)").count() == 1
            && compact.matches("self.raw.delete(target)").count() == 1
            && get.is_some_and(|body| {
                contains_in_order(
                    body,
                    &[
                        "ensure_forbid_prompt(prompt)?;",
                        "lettarget=prepare_target(",
                        "self.raw.read(target)",
                    ],
                )
            })
            && set.is_some_and(|body| {
                contains_in_order(
                    body,
                    &[
                        "ensure_forbid_prompt(prompt)?;",
                        "letrequest=prepare_write(",
                        "invocation.prepare()?;",
                        "invocation.mark_started();",
                        "self.raw.write(request)",
                    ],
                )
            })
            && delete.is_some_and(|body| {
                contains_in_order(
                    body,
                    &[
                        "ensure_forbid_prompt(prompt)?;",
                        "lettarget=prepare_target(",
                        "invocation.prepare()?;",
                        "invocation.mark_started();",
                        "self.raw.delete(target)",
                    ],
                )
            })
            && recovery.is_some_and(|(recover, verify)| {
                !recover.contains("self.raw")
                    && contains_in_order(
                        verify,
                        &[
                            "ensure_forbid_prompt(prompt).map_err(RecoveryFailure::Closed)?;",
                            "letprepared=prepare_target(",
                            "self.raw.read(prepared)",
                        ],
                    )
            })
    }

    fn source_violations(crate_src: &std::path::Path) -> std::io::Result<Vec<SourceViolation>> {
        let raw_source = std::path::Path::new(RAW_SOURCE);
        let policy_source = std::path::Path::new(POLICY_SOURCE);
        let mut violations = Vec::new();
        for path in production_rust_sources(crate_src)? {
            let relative = path
                .strip_prefix(crate_src)
                .map_err(std::io::Error::other)?;
            let source = std::fs::read_to_string(&path)?;
            let test_module = if relative == raw_source {
                "test_support"
            } else {
                "tests"
            };
            let production = masked_production_code(&source, test_module)
                .unwrap_or_else(|| masked_rust_code(&source));
            let production_with_strings = production_code_with_strings(&source, test_module)
                .unwrap_or_else(|| rust_code_with_strings(&source));
            let compact: String = production
                .chars()
                .filter(|character| !character.is_whitespace())
                .collect();
            let has_extern = production
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .any(|identifier| identifier == "extern");
            if relative != raw_source && (has_extern || compact.contains("windows::core::link")) {
                record_violation(&mut violations, relative, "ffi_declaration");
            }
            for identifier in production
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            {
                let direct_wincred = is_direct_wincred_identifier(identifier)
                    || (relative == raw_source && identifier.starts_with("CREDENTIAL"));
                let permitted = if direct_wincred {
                    relative == raw_source && is_allowed_raw_wincred_identifier(identifier)
                } else if is_raw_boundary_identifier(identifier) {
                    relative == raw_source || relative == policy_source
                } else {
                    true
                };
                if !permitted {
                    record_violation(&mut violations, relative, identifier);
                }
            }
            for identifier in production_with_strings
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            {
                let direct_wincred = is_direct_wincred_identifier(identifier)
                    || (relative == raw_source && identifier.starts_with("CREDENTIAL"));
                if direct_wincred
                    && !(relative == raw_source && is_allowed_raw_wincred_identifier(identifier))
                {
                    record_violation(&mut violations, relative, identifier);
                }
            }
        }
        Ok(violations)
    }

    #[test]
    fn ordinary_read_maps_unknown_win32_failure_to_internal_once_without_prose() {
        let (raw, calls) = unknown_read_store(0xdead_beef);
        let boundary = WindowsCredentialManagerBoundary::from_raw(raw);
        let gate = NativeInteractionGate::isolated_for_test();
        let lease = gate.acquire().expect("isolated native interaction lease");

        assert_eq!(
            boundary.get_secret(lease.forbid_prompt(), &EntryLocator::authority()),
            Err(CredentialStoreFailure::Internal)
        );
        assert_eq!(calls.snapshot(), (1, 0, 0));
    }

    #[test]
    fn generic_local_single_call_replace_has_exact_explicit_readback() {
        let (raw, calls) = memory_store();
        let entries = KeyringEntryAdapter::new(std::sync::Arc::new(
            WindowsCredentialManagerBoundary::from_raw(raw),
        ));
        let gate = NativeInteractionGate::isolated_for_test();
        let locator = EntryLocator::authority();
        let secret = vec![0xa5; 2_560];

        let mut write_lease = gate.acquire().expect("write lease");
        entries
            .write(&mut write_lease, &locator, &secret)
            .expect("replace credential");
        drop(write_lease);
        assert_eq!(calls.snapshot(), (0, 1, 0));

        let mut read_lease = gate.acquire().expect("readback lease");
        let readback = entries
            .read(&mut read_lease, &locator)
            .expect("readback result")
            .expect("readback present");
        assert_eq!(readback.as_slice(), secret.as_slice());
        assert_eq!(calls.snapshot(), (1, 1, 0));
    }

    #[test]
    fn unknown_mutations_mark_started_once_then_stall_later_access() {
        let locator = EntryLocator::authority();

        let (write_raw, write_calls) = failing_mutation_store(Some(0xfefe_fefe), None);
        let write_entries = KeyringEntryAdapter::new(std::sync::Arc::new(
            WindowsCredentialManagerBoundary::from_raw(write_raw),
        ));
        let write_gate = NativeInteractionGate::isolated_for_test();
        let mut write_lease = write_gate.acquire().expect("write lease");
        assert_eq!(
            write_entries.write(&mut write_lease, &locator, b"write-canary"),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        drop(write_lease);
        assert_eq!(write_calls.snapshot(), (0, 1, 0));
        assert!(matches!(
            write_gate.acquire(),
            Err(CredentialStoreFailure::StalledWorker)
        ));

        let (delete_raw, delete_calls) = failing_mutation_store(None, Some(0xfefe_fefe));
        let delete_entries = KeyringEntryAdapter::new(std::sync::Arc::new(
            WindowsCredentialManagerBoundary::from_raw(delete_raw),
        ));
        let delete_gate = NativeInteractionGate::isolated_for_test();
        let mut delete_lease = delete_gate.acquire().expect("delete lease");
        assert_eq!(
            delete_entries.delete_and_verify_absent(&mut delete_lease, &locator),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        drop(delete_lease);
        assert_eq!(delete_calls.snapshot(), (0, 0, 1));
        assert!(matches!(
            delete_gate.acquire(),
            Err(CredentialStoreFailure::StalledWorker)
        ));
    }

    #[test]
    fn recovery_diagnosis_is_zero_call_then_one_authority_verification() {
        let (cancelled_raw, cancelled_calls) = memory_store();
        let cancelled_gate = NativeInteractionGate::isolated_for_test();
        let cancelled_facade = NativeRecoveryFacade::from_boundary(
            cancelled_gate,
            Box::new(WindowsCredentialManagerBoundary::from_raw(cancelled_raw)),
        );
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert_eq!(
            cancelled_facade.diagnose_or_unlock(RecoveryTarget::CredentialStore, &cancellation),
            Err(CredentialStoreFailure::Cancelled)
        );
        assert_eq!(cancelled_calls.snapshot(), (0, 0, 0));

        let (raw, calls) = memory_store();
        let gate = NativeInteractionGate::isolated_for_test();
        let facade = NativeRecoveryFacade::from_boundary(
            gate,
            Box::new(WindowsCredentialManagerBoundary::from_raw(raw)),
        );
        assert_eq!(
            facade.diagnose_or_unlock(RecoveryTarget::CredentialStore, &CancellationToken::new(),),
            Ok(RecoveryOutcome::Ready)
        );
        assert_eq!(calls.snapshot(), (1, 0, 0));
    }

    #[test]
    fn invalid_write_is_rejected_before_mark_or_native_call() {
        let (raw, calls) = memory_store();
        let entries = KeyringEntryAdapter::new(std::sync::Arc::new(
            WindowsCredentialManagerBoundary::from_raw(raw),
        ));
        let gate = NativeInteractionGate::isolated_for_test();
        let mut lease = gate.acquire().expect("write lease");
        assert_eq!(
            entries.write(&mut lease, &EntryLocator::authority(), &vec![0xa5; 2_561]),
            Err(CredentialStoreFailure::PayloadTooLarge)
        );
        drop(lease);
        assert_eq!(calls.snapshot(), (0, 0, 0));
        assert!(gate.acquire().is_ok());
    }

    #[test]
    fn missing_delete_is_idempotent_only_after_one_absence_readback() {
        let (raw, calls) = memory_store();
        let entries = KeyringEntryAdapter::new(std::sync::Arc::new(
            WindowsCredentialManagerBoundary::from_raw(raw),
        ));
        let gate = NativeInteractionGate::isolated_for_test();
        let mut lease = gate.acquire().expect("delete lease");
        assert_eq!(
            entries.delete_and_verify_absent(&mut lease, &EntryLocator::authority()),
            Ok(())
        );
        drop(lease);
        assert_eq!(calls.snapshot(), (1, 0, 1));
        assert!(gate.acquire().is_ok());
    }

    #[test]
    fn recovery_found_missing_and_closed_failures_use_one_verification_read() {
        let (found_raw, found_calls) = memory_store();
        let found_entries = KeyringEntryAdapter::new(std::sync::Arc::new(
            WindowsCredentialManagerBoundary::from_raw(found_raw.clone()),
        ));
        let found_gate = NativeInteractionGate::isolated_for_test();
        let mut lease = found_gate.acquire().expect("authority write lease");
        found_entries
            .write(&mut lease, &EntryLocator::authority(), b"authority-marker")
            .expect("seed authority marker");
        drop(lease);
        let found_facade = NativeRecoveryFacade::from_boundary(
            found_gate,
            Box::new(WindowsCredentialManagerBoundary::from_raw(found_raw)),
        );
        assert_eq!(
            found_facade
                .diagnose_or_unlock(RecoveryTarget::CredentialStore, &CancellationToken::new(),),
            Ok(RecoveryOutcome::Ready)
        );
        assert_eq!(found_calls.snapshot(), (1, 1, 0));

        for (code, expected) in [
            (5, CredentialStoreFailure::AccessDenied),
            (1223, CredentialStoreFailure::Cancelled),
            (1312, CredentialStoreFailure::Unavailable),
            (0xfefe_fefe, CredentialStoreFailure::Internal),
        ] {
            let (raw, calls) = unknown_read_store(code);
            let gate = NativeInteractionGate::isolated_for_test();
            let facade = NativeRecoveryFacade::from_boundary(
                gate,
                Box::new(WindowsCredentialManagerBoundary::from_raw(raw)),
            );
            assert_eq!(
                facade.diagnose_or_unlock(
                    RecoveryTarget::CredentialStore,
                    &CancellationToken::new(),
                ),
                Err(expected)
            );
            assert_eq!(calls.snapshot(), (1, 0, 0));
        }
    }

    #[test]
    fn safe_policy_and_sole_raw_caller_source_inventories_are_closed() {
        let policy = masked_production_code(include_str!("windows_credential_manager.rs"), "tests")
            .expect("one root policy test module");
        let compact: String = policy
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        assert!(
            !policy
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .any(|token| token == "unsafe")
        );
        for raw_symbol in [
            "CredReadW",
            "CredWriteW",
            "CredDeleteW",
            "CredFree",
            "CredUI",
        ] {
            assert!(
                !policy.contains(raw_symbol),
                "raw symbol {raw_symbol} in policy"
            );
        }
        assert!(compact.contains("implKeyringBoundaryforWindowsCredentialManagerBoundary"));
        assert!(compact.contains("implRecoveryBoundaryforWindowsCredentialManagerBoundary"));
        assert!(safe_policy_raw_surface_is_exact(&policy));
        assert_eq!(compact.matches("invocation.mark_started();").count(), 2);
        let write = compact
            .split_once("fnset_secret(")
            .and_then(|(_, tail)| {
                tail.split_once("fndelete_credential(")
                    .map(|(body, _)| body)
            })
            .expect("closed write method");
        let write_prepare = write
            .find("letrequest=prepare_write(")
            .expect("write preparation");
        let write_mark = write
            .find("invocation.mark_started();")
            .expect("mutation cut");
        let write_invoke = write
            .find("self.raw.write(request)")
            .expect("one write invoke");
        assert!(write_prepare < write_mark && write_mark < write_invoke);
        let delete = compact
            .split_once("fndelete_credential(")
            .and_then(|(_, tail)| {
                tail.split_once("implRecoveryBoundary")
                    .map(|(body, _)| body)
            })
            .expect("closed delete method");
        let delete_prepare = delete.find("prepare_target(").expect("delete preparation");
        let delete_mark = delete
            .find("invocation.mark_started();")
            .expect("delete cut");
        let delete_invoke = delete
            .find("self.raw.delete(target)")
            .expect("one delete invoke");
        assert!(delete_prepare < delete_mark && delete_mark < delete_invoke);
        let recover = compact
            .split_once("fnrecover(")
            .and_then(|(_, tail)| tail.split_once("fnverify(").map(|(body, _)| body))
            .expect("closed recovery diagnosis");
        assert!(!recover.contains("self.raw"));

        let crate_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let violations = source_violations(&crate_src).expect("read all production Rust sources");
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn nested_production_modules_cannot_bypass_raw_or_wincred_inventories() {
        let fixture = std::env::temp_dir().join(format!(
            "audio-graph-12c4-nested-inventory-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let crate_src = fixture.join("src");
        let nested = crate_src.join("credentials/adapters/bypass");
        std::fs::create_dir_all(&nested).expect("create nested source fixture");
        std::fs::write(
            nested.join("mod.rs"),
            r#"
                fn bypass(raw: NativeWinCredStore, target: PreparedTarget) {
                    let raw = NativeWinCredStore::new();
                    let _ = RawCredentialStore::read(&raw, target);
                    let _ = CredProtectW;
                }
                windows::core::link!("credui.dll" "system"
                    "CredUIPromptForWindowsCredentialsW" fn invoke_prompt() -> BOOL);
            "#,
        )
        .expect("write nested source fixture");

        let violations = source_violations(&crate_src).expect("scan nested source fixture");
        std::fs::remove_dir_all(&fixture).expect("remove nested source fixture");
        assert!(
            violations
                .iter()
                .any(|violation| { violation.identifier == "CredUIPromptForWindowsCredentialsW" })
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.identifier == "CredProtectW")
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.identifier == "ffi_declaration")
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.identifier == "NativeWinCredStore")
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.identifier == "RawCredentialStore")
        );
    }

    #[test]
    fn policy_inventory_rejects_alternate_raw_call_spellings() {
        let policy = masked_production_code(include_str!("windows_credential_manager.rs"), "tests")
            .expect("one root policy test module");
        let method_mutant = policy.replacen(
            "let mut secret = self.raw.read(target).map_err(Self::map_read_failure)?;",
            "let _ = self.raw.as_ref().read(target);\nlet mut secret = self.raw.read(target).map_err(Self::map_read_failure)?;",
            1,
        );
        assert_ne!(method_mutant, policy);
        assert!(!safe_policy_raw_surface_is_exact(&method_mutant));

        let ufcs_mutant = format!(
            "{policy}\nfn raw_bypass(raw: &dyn RawCredentialStore, target: PreparedTarget) {{ let _ = RawCredentialStore::read(raw, target); }}"
        );
        assert!(!safe_policy_raw_surface_is_exact(&ufcs_mutant));
    }

    #[test]
    fn production_source_discovery_fails_closed_on_unreadable_input() {
        let fixture = std::env::temp_dir().join(format!(
            "audio-graph-12c4-unreadable-inventory-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let missing = fixture.join("missing");
        assert!(source_violations(&missing).is_err());

        let crate_src = fixture.join("src");
        std::fs::create_dir_all(&crate_src).expect("create unreadable source fixture");
        std::fs::write(crate_src.join("invalid.rs"), [0xff])
            .expect("write invalid Rust source fixture");
        assert!(source_violations(&crate_src).is_err());
        std::fs::remove_dir_all(&fixture).expect("remove unreadable source fixture");
    }
}
