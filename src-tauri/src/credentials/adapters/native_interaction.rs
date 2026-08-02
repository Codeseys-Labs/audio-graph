#![forbid(unsafe_code)]

use crate::credentials::domain::CredentialStoreFailure;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

static PROCESS_NATIVE_INTERACTION_GATE: OnceLock<NativeInteractionGate> = OnceLock::new();

pub(super) fn process_native_interaction_gate() -> &'static NativeInteractionGate {
    PROCESS_NATIVE_INTERACTION_GATE.get_or_init(NativeInteractionGate::new)
}

pub(super) struct NativeInteractionGate {
    access: Mutex<()>,
    stalled: AtomicBool,
    // NATIVE_INTERACTION_TEST_ONLY_BEGIN
    #[cfg(test)]
    queued_waiters: std::sync::atomic::AtomicUsize,
    // NATIVE_INTERACTION_TEST_ONLY_END
}

pub(super) struct NativeInteractionLease<'gate> {
    _guard: MutexGuard<'gate, ()>,
    forbid_prompt: ForbidPrompt<'gate>,
    mutation_invocation: MutationInvocation<'gate>,
}

mod ordinary_prompt {
    use super::NativeInteractionGate;

    pub(super) struct Seal<'gate> {
        gate: &'gate NativeInteractionGate,
    }

    pub(super) fn mint(gate: &NativeInteractionGate) -> Seal<'_> {
        Seal { gate }
    }

    pub(super) fn is_stalled(seal: &Seal<'_>) -> bool {
        seal.gate.is_stalled()
    }

    pub(super) fn latch_stalled(seal: &Seal<'_>) {
        seal.gate.latch_stalled();
    }
}

pub(super) struct ForbidPrompt<'gate> {
    _seal: ordinary_prompt::Seal<'gate>,
}

pub(super) struct MutationInvocation<'gate> {
    gate: &'gate NativeInteractionGate,
    started: bool,
}

impl NativeInteractionGate {
    fn new() -> Self {
        Self {
            access: Mutex::new(()),
            stalled: AtomicBool::new(false),
            // NATIVE_INTERACTION_TEST_ONLY_BEGIN
            #[cfg(test)]
            queued_waiters: std::sync::atomic::AtomicUsize::new(0),
            // NATIVE_INTERACTION_TEST_ONLY_END
        }
    }

    pub(super) fn acquire(&self) -> Result<NativeInteractionLease<'_>, CredentialStoreFailure> {
        if self.is_stalled() {
            return Err(CredentialStoreFailure::StalledWorker);
        }
        // NATIVE_INTERACTION_TEST_ONLY_BEGIN
        #[cfg(test)]
        self.queued_waiters.fetch_add(1, Ordering::AcqRel);
        // NATIVE_INTERACTION_TEST_ONLY_END
        let locked = self.access.lock();
        // NATIVE_INTERACTION_TEST_ONLY_BEGIN
        #[cfg(test)]
        self.queued_waiters.fetch_sub(1, Ordering::AcqRel);
        // NATIVE_INTERACTION_TEST_ONLY_END
        let guard = match locked {
            Ok(guard) => guard,
            Err(_) => {
                self.latch_stalled();
                return Err(CredentialStoreFailure::StalledWorker);
            }
        };
        if self.is_stalled() {
            return Err(CredentialStoreFailure::StalledWorker);
        }
        Ok(NativeInteractionLease {
            _guard: guard,
            forbid_prompt: ForbidPrompt {
                _seal: ordinary_prompt::mint(self),
            },
            mutation_invocation: MutationInvocation {
                gate: self,
                started: false,
            },
        })
    }

    fn is_stalled(&self) -> bool {
        self.stalled.load(Ordering::Acquire)
    }

    fn latch_stalled(&self) {
        self.stalled.store(true, Ordering::Release);
    }

    // NATIVE_INTERACTION_TEST_ONLY_BEGIN
    #[cfg(test)]
    pub(super) fn isolated_for_test() -> &'static Self {
        Box::leak(Box::new(Self::new()))
    }

    #[cfg(test)]
    pub(super) fn poison_for_test(&self) -> ! {
        let _guard = self.access.lock().expect("acquire isolated native gate");
        panic!("poison isolated native interaction gate")
    }

    #[cfg(test)]
    pub(super) fn queued_waiters_for_test(&self) -> usize {
        self.queued_waiters.load(Ordering::Acquire)
    }
    // NATIVE_INTERACTION_TEST_ONLY_END
}

impl<'gate> NativeInteractionLease<'gate> {
    pub(super) fn ensure_healthy(&self) -> Result<(), CredentialStoreFailure> {
        ensure_forbid_prompt(&self.forbid_prompt)
    }

    pub(super) fn forbid_prompt(&self) -> &ForbidPrompt<'gate> {
        &self.forbid_prompt
    }

    pub(super) fn mutation_capabilities<'borrow>(
        &'borrow mut self,
    ) -> (
        &'borrow ForbidPrompt<'gate>,
        &'borrow mut MutationInvocation<'gate>,
    ) {
        (&self.forbid_prompt, &mut self.mutation_invocation)
    }

    pub(super) fn latch_commit_unknown(&self) -> CredentialStoreFailure {
        ordinary_prompt::latch_stalled(&self.forbid_prompt._seal);
        CredentialStoreFailure::CommitUnknown
    }

    fn latch_stalled(&self) -> CredentialStoreFailure {
        ordinary_prompt::latch_stalled(&self.forbid_prompt._seal);
        CredentialStoreFailure::StalledWorker
    }
}

pub(super) fn ensure_forbid_prompt(
    prompt: &ForbidPrompt<'_>,
) -> Result<(), CredentialStoreFailure> {
    if ordinary_prompt::is_stalled(&prompt._seal) {
        Err(CredentialStoreFailure::StalledWorker)
    } else {
        Ok(())
    }
}

pub(super) fn latch_forbid_uncertainty(prompt: &ForbidPrompt<'_>) -> CredentialStoreFailure {
    ordinary_prompt::latch_stalled(&prompt._seal);
    CredentialStoreFailure::StalledWorker
}

impl MutationInvocation<'_> {
    pub(super) fn prepare(&mut self) -> Result<(), CredentialStoreFailure> {
        if self.gate.is_stalled() {
            return Err(CredentialStoreFailure::StalledWorker);
        }
        Ok(())
    }

    pub(super) fn mark_started(&mut self) {
        self.started = true;
    }

    pub(super) fn has_started(&self) -> bool {
        self.started
    }

    pub(super) fn latch_uncertainty(&self) -> CredentialStoreFailure {
        self.gate.latch_stalled();
        if self.started {
            CredentialStoreFailure::CommitUnknown
        } else {
            CredentialStoreFailure::StalledWorker
        }
    }

    pub(super) fn latch_commit_unknown(&self) -> CredentialStoreFailure {
        self.gate.latch_stalled();
        CredentialStoreFailure::CommitUnknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecoveryTarget {
    CredentialStore,
}

pub(super) struct CancellationToken {
    cancelled: AtomicBool,
}

impl CancellationToken {
    pub(super) fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
        }
    }

    pub(super) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecoveryOutcome {
    Ready,
    RecoveryRequired,
}

// EXPLICIT_RECOVERY_BOUNDARY_BEGIN
mod explicit_recovery {
    use super::{
        CancellationToken, ForbidPrompt, NativeInteractionGate, RecoveryOutcome, RecoveryTarget,
        ensure_forbid_prompt, latch_forbid_uncertainty,
    };
    use crate::credentials::domain::CredentialStoreFailure;

    #[derive(Clone, Copy)]
    enum RecoveryFailure {
        Closed(CredentialStoreFailure),
        Uncertain,
    }

    impl RecoveryFailure {
        fn into_closed(self, lease: &super::NativeInteractionLease<'_>) -> CredentialStoreFailure {
            match self {
                Self::Closed(
                    failure @ (CredentialStoreFailure::StalledWorker
                    | CredentialStoreFailure::CommitUnknown),
                ) => {
                    if failure == CredentialStoreFailure::CommitUnknown {
                        let _ = lease.latch_commit_unknown();
                    } else {
                        let _ = lease.latch_stalled();
                    }
                    failure
                }
                Self::Uncertain => lease.latch_stalled(),
                Self::Closed(failure) => failure,
            }
        }

        fn stalls_gate(self) -> bool {
            matches!(
                self,
                Self::Uncertain
                    | Self::Closed(
                        CredentialStoreFailure::StalledWorker
                            | CredentialStoreFailure::CommitUnknown
                    )
            )
        }
    }

    trait RecoveryBoundary: Send + Sync {
        fn recover(
            &self,
            prompt: &recovery_facade::AllowPrompt<'_, '_>,
            target: RecoveryTarget,
            cancellation: &CancellationToken,
        ) -> Result<(), RecoveryFailure>;

        fn verify(
            &self,
            prompt: &ForbidPrompt<'_>,
            target: RecoveryTarget,
        ) -> Result<RecoveryOutcome, RecoveryFailure>;
    }

    mod recovery_facade {
        use super::super::NativeInteractionLease;
        use super::{
            CancellationToken, CredentialStoreFailure, NativeInteractionGate, RecoveryBoundary,
            RecoveryOutcome, RecoveryTarget, ensure_forbid_prompt, latch_forbid_uncertainty,
        };
        use std::panic::{AssertUnwindSafe, catch_unwind};

        pub(super) struct AllowPrompt<'lease, 'gate> {
            lease: &'lease NativeInteractionLease<'gate>,
        }

        #[allow(dead_code)]
        pub(in crate::credentials::adapters) struct NativeRecoveryFacade {
            gate: &'static NativeInteractionGate,
            boundary: Box<dyn RecoveryBoundary>,
        }

        impl NativeRecoveryFacade {
            pub(super) fn from_boundary(
                gate: &'static NativeInteractionGate,
                boundary: Box<dyn RecoveryBoundary>,
            ) -> Self {
                Self { gate, boundary }
            }

            pub(in crate::credentials::adapters) fn diagnose_or_unlock(
                &self,
                target: RecoveryTarget,
                cancellation: &CancellationToken,
            ) -> Result<RecoveryOutcome, CredentialStoreFailure> {
                if cancellation.is_cancelled() {
                    return Err(CredentialStoreFailure::Cancelled);
                }
                let lease = self.gate.acquire()?;
                if cancellation.is_cancelled() {
                    return Err(CredentialStoreFailure::Cancelled);
                }
                let recovery = {
                    let prompt = AllowPrompt { lease: &lease };
                    match catch_unwind(AssertUnwindSafe(|| {
                        self.boundary.recover(&prompt, target, cancellation)
                    })) {
                        Ok(result) => result,
                        Err(_) => {
                            let _ = prompt.lease.latch_stalled();
                            return Err(CredentialStoreFailure::StalledWorker);
                        }
                    }
                };

                let recovery_failure = match recovery {
                    Ok(()) => None,
                    Err(failure) if failure.stalls_gate() => {
                        return Err(failure.into_closed(&lease));
                    }
                    Err(failure) => Some(failure.into_closed(&lease)),
                };

                ensure_forbid_prompt(lease.forbid_prompt())?;
                let verification = match catch_unwind(AssertUnwindSafe(|| {
                    self.boundary.verify(lease.forbid_prompt(), target)
                })) {
                    Ok(result) => result,
                    Err(_) => return Err(latch_forbid_uncertainty(lease.forbid_prompt())),
                };
                match verification {
                    Err(failure) => Err(failure.into_closed(&lease)),
                    Ok(outcome) => match recovery_failure {
                        Some(failure) => Err(failure),
                        None => Ok(outcome),
                    },
                }
            }
        }
    }

    use recovery_facade::AllowPrompt;
    pub(in crate::credentials::adapters) use recovery_facade::NativeRecoveryFacade;

    fn recovery_boundary_signature_witness(
        boundary: &dyn RecoveryBoundary,
        allow: &AllowPrompt<'_, '_>,
        forbid: &ForbidPrompt<'_>,
        target: RecoveryTarget,
        cancellation: &CancellationToken,
    ) {
        let _ = boundary.recover(allow, target, cancellation);
        let _ = boundary.verify(forbid, target);
    }

    const _: fn(
        &dyn RecoveryBoundary,
        &AllowPrompt<'_, '_>,
        &ForbidPrompt<'_>,
        RecoveryTarget,
        &CancellationToken,
    ) = recovery_boundary_signature_witness;

    const _: fn(&'static NativeInteractionGate, Box<dyn RecoveryBoundary>) -> NativeRecoveryFacade =
        NativeRecoveryFacade::from_boundary;

    // EXPLICIT_RECOVERY_TEST_SUPPORT_BEGIN
    #[cfg(test)]
    pub(in crate::credentials::adapters::native_interaction) mod test_support {
        use super::{
            AllowPrompt, CancellationToken, ForbidPrompt, NativeInteractionGate,
            NativeRecoveryFacade, RecoveryBoundary, RecoveryFailure, RecoveryOutcome,
            RecoveryTarget,
        };
        use crate::credentials::domain::CredentialStoreFailure;
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Copy)]
        pub(in crate::credentials::adapters::native_interaction) enum RecoveryStep {
            Success,
            Closed(CredentialStoreFailure),
            Uncertain,
            Panic,
        }

        struct ScriptedRecoveryBoundary {
            trace: Arc<Mutex<Vec<&'static str>>>,
            recover: Mutex<Option<RecoveryStep>>,
            verify: Mutex<Option<RecoveryStep>>,
        }

        impl ScriptedRecoveryBoundary {
            fn run_step<T>(
                step: &Mutex<Option<RecoveryStep>>,
                success: T,
            ) -> Result<T, RecoveryFailure> {
                match step
                    .lock()
                    .expect("scripted recovery step lock")
                    .take()
                    .expect("one scripted recovery step")
                {
                    RecoveryStep::Success => Ok(success),
                    RecoveryStep::Closed(failure) => Err(RecoveryFailure::Closed(failure)),
                    RecoveryStep::Uncertain => Err(RecoveryFailure::Uncertain),
                    RecoveryStep::Panic => panic!("scripted recovery panic canary"),
                }
            }
        }

        impl RecoveryBoundary for ScriptedRecoveryBoundary {
            fn recover(
                &self,
                _prompt: &AllowPrompt<'_, '_>,
                _target: RecoveryTarget,
                _cancellation: &CancellationToken,
            ) -> Result<(), RecoveryFailure> {
                self.trace
                    .lock()
                    .expect("scripted recovery trace lock")
                    .push("allow");
                Self::run_step(&self.recover, ())
            }

            fn verify(
                &self,
                _prompt: &ForbidPrompt<'_>,
                _target: RecoveryTarget,
            ) -> Result<RecoveryOutcome, RecoveryFailure> {
                self.trace
                    .lock()
                    .expect("scripted recovery trace lock")
                    .push("forbid");
                Self::run_step(&self.verify, RecoveryOutcome::Ready)
            }
        }

        pub(in crate::credentials::adapters::native_interaction) fn scripted_facade(
            gate: &'static NativeInteractionGate,
            trace: Arc<Mutex<Vec<&'static str>>>,
            recover: RecoveryStep,
            verify: RecoveryStep,
        ) -> NativeRecoveryFacade {
            NativeRecoveryFacade::from_boundary(
                gate,
                Box::new(ScriptedRecoveryBoundary {
                    trace,
                    recover: Mutex::new(Some(recover)),
                    verify: Mutex::new(Some(verify)),
                }),
            )
        }
    }
    // EXPLICIT_RECOVERY_TEST_SUPPORT_END
}
// EXPLICIT_RECOVERY_BOUNDARY_END

// NATIVE_RECOVERY_FACADE_SEAM_BEGIN
#[allow(unused_imports)]
pub(super) use explicit_recovery::NativeRecoveryFacade;

const _: fn(
    &NativeRecoveryFacade,
    RecoveryTarget,
    &CancellationToken,
) -> Result<RecoveryOutcome, CredentialStoreFailure> = NativeRecoveryFacade::diagnose_or_unlock;
// NATIVE_RECOVERY_FACADE_SEAM_END

#[cfg(test)]
mod tests {
    use super::explicit_recovery::test_support::{RecoveryStep, scripted_facade};
    use super::{CancellationToken, NativeInteractionGate, RecoveryOutcome, RecoveryTarget};
    use crate::credentials::domain::CredentialStoreFailure;
    use std::sync::{Arc, Mutex, mpsc};

    const NATIVE_INTERACTION_SOURCE: &str = include_str!("native_interaction.rs");
    const KEYRING_ENTRY_SOURCE: &str = include_str!("keyring_entry.rs");
    const NATIVE_KEYRING_SOURCE: &str = include_str!("native_keyring.rs");
    const ADAPTERS_MOD_SOURCE: &str = include_str!("mod.rs");

    fn production_source(source: &str) -> String {
        source
            .split_once("#[cfg(test)]\nmod tests {")
            .map_or(source, |(production, _)| production)
            .to_owned()
    }

    fn between<'a>(source: &'a str, start: &str, end: &str) -> Option<&'a str> {
        source
            .split_once(start)
            .and_then(|(_, tail)| tail.split_once(end))
            .map(|(body, _)| body)
    }

    fn compact_whitespace(source: &str) -> String {
        source
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect()
    }

    fn declaration_prefix_is_public(compact: &str, keyword_index: usize) -> bool {
        let prefix = &compact[..keyword_index];
        let declaration_start = prefix.rfind([';', '{', '}']).map_or(0, |index| index + 1);
        let mut visibility = &prefix[declaration_start..];
        if let Some(attribute_end) = visibility.rfind(']') {
            visibility = &visibility[attribute_end + 1..];
        }
        visibility == "pub" || (visibility.starts_with("pub(") && visibility.ends_with(')'))
    }

    fn capability_has_forbidden_derive(source: &str, capability: &str) -> bool {
        let compact = compact_whitespace(source);
        let declaration = format!("struct{capability}");
        let Some(declaration_index) = compact.find(&declaration) else {
            return true;
        };
        let prefix = &compact[..declaration_index];
        let item_start = prefix.rfind([';', '}']).map_or(0, |index| index + 1);
        let attributes = &prefix[item_start..];
        let Some(derive_start) = attributes.rfind("#[derive(") else {
            return false;
        };
        let derive = &attributes[derive_start..];
        ["Clone", "Copy", "Default"].iter().any(|forbidden| {
            derive
                .split([',', '(', ')', '[', ']'])
                .any(|part| part == *forbidden)
        })
    }

    fn capability_has_impl(source: &str, capability: &str) -> bool {
        for (index, _) in source.match_indices("impl") {
            let previous = source[..index].chars().next_back();
            let next = source[index + "impl".len()..].chars().next();
            let is_keyword = previous
                .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '_')
                && next.is_some_and(|character| character.is_whitespace() || character == '<');
            if !is_keyword {
                continue;
            }
            let header = source[index..]
                .split_once('{')
                .map_or(&source[index..], |(header, _)| header);
            if header.contains(capability) {
                return true;
            }
        }
        false
    }

    fn aliases_capability(source: &str, capability: &str) -> bool {
        let compact = compact_whitespace(source);
        compact.split(';').any(|statement| {
            statement.contains("type") && statement.contains('=') && statement.contains(capability)
        })
    }

    fn token_has_forbidden_convenience(source: &str, capability: &str) -> bool {
        capability_has_forbidden_derive(source, capability)
            || capability_has_impl(source, capability)
            || aliases_capability(source, capability)
    }

    fn exports_capability(source: &str, capability: &str) -> bool {
        let compact = compact_whitespace(source);
        for (use_index, _) in compact.match_indices("use") {
            let statement = &compact[use_index..];
            let statement = statement
                .split_once(';')
                .map_or(statement, |(statement, _)| statement);
            if statement.contains(capability) && declaration_prefix_is_public(&compact, use_index) {
                return true;
            }
        }
        false
    }

    fn exposes_explicit_recovery_module(source: &str) -> bool {
        let compact = compact_whitespace(source);
        compact
            .match_indices("modexplicit_recovery{")
            .any(|(index, _)| declaration_prefix_is_public(&compact, index))
    }

    fn ordinary_trait_exposes_raw_keyring_error(source: &str) -> bool {
        between(
            source,
            "pub(super) trait KeyringBoundary",
            "struct SystemKeyringBoundary",
        )
        .is_some_and(|boundary| boundary.contains("keyring::Error"))
    }

    fn recovery_contract_has_forbidden_payload(source: &str) -> bool {
        let Some(explicit) = between(
            source,
            "// EXPLICIT_RECOVERY_BOUNDARY_BEGIN",
            "// EXPLICIT_RECOVERY_BOUNDARY_END",
        ) else {
            return true;
        };
        let Some(boundary) = between(explicit, "trait RecoveryBoundary", "mod recovery_facade {")
        else {
            return true;
        };
        let Some(facade) = between(
            explicit,
            "struct NativeRecoveryFacade",
            "fn recovery_boundary_signature_witness",
        ) else {
            return true;
        };
        let Some(seam) = between(
            source,
            "// NATIVE_RECOVERY_FACADE_SEAM_BEGIN",
            "// NATIVE_RECOVERY_FACADE_SEAM_END",
        ) else {
            return true;
        };
        let contract = format!("{boundary}\n{facade}\n{seam}");
        let compact = compact_whitespace(&contract);
        let lower = compact.to_ascii_lowercase();
        [
            "secret",
            "bytes",
            "locator",
            "account",
            "service",
            "record",
            "write",
            "replace",
            "delete",
            "mutation",
            "continuation",
            "closure",
            "callback",
        ]
        .iter()
        .any(|forbidden| lower.contains(forbidden))
            || [
                "&[u8]", "Vec<u8>", "String", "Path", "Fn(", "FnMut(", "FnOnce(", "dynFn", "implFn",
            ]
            .iter()
            .any(|forbidden| compact.contains(forbidden))
            || compact.contains(":bool")
            || compact.contains("->bool")
            || lower.contains("policy")
            || compact.contains("fnrecover<")
            || compact.contains("fnverify<")
            || compact.contains("fndiagnose_or_unlock<")
    }

    fn recovery_value_shapes_are_payload_free(source: &str) -> bool {
        let Some(target) = between(
            source,
            "pub(super) enum RecoveryTarget",
            "pub(super) struct CancellationToken",
        ) else {
            return false;
        };
        let Some(cancellation) = between(
            source,
            "pub(super) struct CancellationToken",
            "impl CancellationToken",
        ) else {
            return false;
        };
        let Some(outcome) = between(
            source,
            "pub(super) enum RecoveryOutcome",
            "// EXPLICIT_RECOVERY_BOUNDARY_BEGIN",
        ) else {
            return false;
        };
        compact_whitespace(target) == "{CredentialStore,}"
            && compact_whitespace(cancellation) == "{cancelled:AtomicBool,}"
            && compact_whitespace(outcome) == "{Ready,RecoveryRequired,}"
    }

    fn uses_poison_reset(source: &str) -> bool {
        let compact = compact_whitespace(source);
        [
            "PoisonError::into_inner",
            ".into_inner()",
            "clear_poison",
            "reset_poison",
            "stalled.store(false,",
            "stalled.swap(false,",
        ]
        .iter()
        .any(|forbidden| compact.contains(forbidden))
    }

    #[test]
    fn production_inventory_does_not_trust_comment_markers() {
        let fixture = production_source(
            "pub(super) struct ForbidPrompt;\n// NATIVE_INTERACTION_TEST_ONLY_BEGIN\nimpl ForbidPrompt { fn forge() -> Self { Self } }\npub(crate) use child::ForbidPrompt;\nself.stalled.store(false, Ordering::Release);\n// NATIVE_INTERACTION_TEST_ONLY_END\n#[cfg(test)]\nmod tests {",
        );

        assert!(token_has_forbidden_convenience(&fixture, "ForbidPrompt"));
        assert!(exports_capability(&fixture, "ForbidPrompt"));
        assert!(uses_poison_reset(&fixture));
    }

    #[test]
    fn sealed_prompt_policy_source_inventory_is_closed() {
        // Fixture self-checks make every structural detector prove that it can
        // reject the dangerous shape it is intended to freeze out.
        assert!(token_has_forbidden_convenience(
            "#[derive(\nDebug,\nClone,\nCopy,\nDefault\n)]\npub(super) struct ForbidPrompt;",
            "ForbidPrompt",
        ));
        assert!(token_has_forbidden_convenience(
            "pub(super) struct ForbidPrompt;\nimpl<'gate> ForbidPrompt<'gate> {\nfn forge() -> Self { Self { _seal: unreachable!() } }\n}",
            "ForbidPrompt",
        ));
        assert!(aliases_capability(
            "type\nPromptAlias<'gate>\n=\nForbidPrompt<'gate>;",
            "ForbidPrompt",
        ));
        assert!(exports_capability(
            "pub(\nin crate::credentials::adapters\n)\nuse child::{\nForbidPrompt as PromptAlias\n};",
            "ForbidPrompt",
        ));
        for exposed in [
            "pub mod explicit_recovery {}",
            "pub(super) mod explicit_recovery {}",
            "pub(crate) mod explicit_recovery {}",
            "pub(in crate::credentials::adapters) mod explicit_recovery {}",
            "pub(\nin crate::credentials::adapters\n)\nmod explicit_recovery {}",
        ] {
            assert!(exposes_explicit_recovery_module(exposed));
        }
        assert!(ordinary_trait_exposes_raw_keyring_error(
            "pub(super) trait KeyringBoundary { fn get(&self) -> keyring::Error; } struct SystemKeyringBoundary;"
        ));
        let recovery_fixture = |fragment: &str| {
            format!(
                "// EXPLICIT_RECOVERY_BOUNDARY_BEGIN\nmod explicit_recovery {{\ntrait RecoveryBoundary {{ {fragment} }}\nmod recovery_facade {{\n#[allow(dead_code)]\nstruct NativeRecoveryFacade;\nimpl NativeRecoveryFacade {{ fn diagnose_or_unlock(&self) {{}} }}\n}}\nfn recovery_boundary_signature_witness() {{}}\n}}\n// EXPLICIT_RECOVERY_BOUNDARY_END\n// NATIVE_RECOVERY_FACADE_SEAM_BEGIN\nconst _: () = ();\n// NATIVE_RECOVERY_FACADE_SEAM_END"
            )
        };
        for forbidden_contract in [
            "fn recover(secret: &[u8]);",
            "fn recover(bytes: Vec<u8>);",
            "fn recover(locator: String);",
            "fn recover(account: String);",
            "fn recover(service: String);",
            "fn recover(record: String);",
            "fn write();",
            "fn replace();",
            "fn delete();",
            "fn mutation();",
            "fn continuation();",
            "fn closure();",
            "fn callback();",
            "fn recover(path: Path);",
            "fn recover(handler: Box<dyn Fn()>);",
            "fn recover(handler: impl Fn());",
            "fn recover(flag: bool);",
            "fn recover(policy: InteractionPolicy);",
            "fn recover<T>();",
        ] {
            assert!(recovery_contract_has_forbidden_payload(&recovery_fixture(
                forbidden_contract
            )));
        }
        let recovery_shapes = |target: &str, token: &str, outcome: &str| {
            format!(
                "pub(super) enum RecoveryTarget {target}\npub(super) struct CancellationToken {token}\nimpl CancellationToken {{}}\npub(super) enum RecoveryOutcome {outcome}\n// EXPLICIT_RECOVERY_BOUNDARY_BEGIN"
            )
        };
        assert!(!recovery_value_shapes_are_payload_free(&recovery_shapes(
            "{ CredentialStore([u8; 32]), }",
            "{ cancelled: AtomicBool, }",
            "{ Ready, RecoveryRequired, }",
        )));
        assert!(!recovery_value_shapes_are_payload_free(&recovery_shapes(
            "{ CredentialStore, }",
            "{ cancelled: AtomicBool, payload: Vec<u8>, }",
            "{ Ready, RecoveryRequired, }",
        )));
        assert!(!recovery_value_shapes_are_payload_free(&recovery_shapes(
            "{ CredentialStore, }",
            "{ cancelled: AtomicBool, }",
            "{ Ready(Vec<u8>), RecoveryRequired, }",
        )));
        assert!(uses_poison_reset("poison.into_inner()"));
        assert!(uses_poison_reset(
            "self.stalled.store(false, Ordering::Release);"
        ));

        let native = production_source(NATIVE_INTERACTION_SOURCE);
        let keyring = production_source(KEYRING_ENTRY_SOURCE);
        let store = production_source(NATIVE_KEYRING_SOURCE);

        assert!(native.starts_with("#![forbid(unsafe_code)]"));
        assert!(!native.contains("fn sealed_prompt_policy_source_inventory_is_closed"));
        assert!(native.contains("pub(super) struct NativeInteractionGate"));
        assert!(native.contains("pub(super) struct NativeInteractionLease<'gate>"));
        assert!(native.contains("pub(super) struct ForbidPrompt<'gate>"));
        assert!(native.contains("pub(super) struct MutationInvocation<'gate>"));
        assert!(native.contains("mod explicit_recovery {"));
        assert!(!exposes_explicit_recovery_module(&native));
        assert!(native.contains("struct AllowPrompt<'lease, 'gate>"));
        assert!(native.contains("trait RecoveryBoundary"));
        assert!(native.contains("const _: fn("));
        assert!(recovery_value_shapes_are_payload_free(&native));

        let explicit_recovery = between(
            &native,
            "// EXPLICIT_RECOVERY_BOUNDARY_BEGIN",
            "// EXPLICIT_RECOVERY_BOUNDARY_END",
        )
        .expect("private explicit recovery source interval");
        assert!(explicit_recovery.contains("struct NativeRecoveryFacade"));
        assert!(!native.contains("struct RecoveryPorts"));
        assert_eq!(
            compact_whitespace(explicit_recovery)
                .matches("AllowPrompt{")
                .count(),
            1
        );

        let allow_capability = between(
            explicit_recovery,
            "mod recovery_facade {",
            "use recovery_facade::AllowPrompt;",
        )
        .expect("opaque allow-prompt capability interval");
        assert!(!token_has_forbidden_convenience(&native, "AllowPrompt"));
        assert!(!aliases_capability(&native, "AllowPrompt"));
        assert!(!exports_capability(&native, "AllowPrompt"));
        assert!(!compact_whitespace(allow_capability).contains("pub(super)fnmint"));
        let allow_token = between(
            allow_capability,
            "pub(super) struct AllowPrompt<'lease, 'gate>",
            "#[allow(dead_code)]",
        )
        .expect("opaque allow-prompt representation interval");
        let allow_compact = compact_whitespace(allow_token);
        assert!(allow_compact.contains("{lease:&'leaseNativeInteractionLease<'gate>,}"));
        assert!(!allow_compact.contains("gate:"));

        let token = between(
            &native,
            "pub(super) struct ForbidPrompt<'gate>",
            "pub(super) struct MutationInvocation<'gate>",
        )
        .expect("sealed ordinary token source interval");
        assert!(!token_has_forbidden_convenience(&native, "ForbidPrompt"));
        assert!(!aliases_capability(&native, "ForbidPrompt"));
        assert_eq!(
            compact_whitespace(&native).matches("ForbidPrompt{").count(),
            1
        );
        assert_eq!(
            compact_whitespace(token),
            "{_seal:ordinary_prompt::Seal<'gate>,}"
        );
        assert!(!exports_capability(&native, "ForbidPrompt"));
        assert!(!exports_capability(ADAPTERS_MOD_SOURCE, "ForbidPrompt"));

        let ordinary_boundary = between(
            &keyring,
            "pub(super) trait KeyringBoundary",
            "struct SystemKeyringBoundary",
        )
        .expect("ordinary keyring boundary source interval");
        assert_eq!(ordinary_boundary.matches("&ForbidPrompt<'_>").count(), 3);
        assert_eq!(
            ordinary_boundary
                .matches("&mut MutationInvocation<'_>")
                .count(),
            2
        );
        assert_eq!(ordinary_boundary.matches("Result<").count(), 3);
        assert_eq!(
            ordinary_boundary.matches("CredentialStoreFailure").count(),
            3
        );
        assert!(!ordinary_trait_exposes_raw_keyring_error(&keyring));
        assert!(!ordinary_boundary.contains("InteractionPolicy"));
        assert!(!ordinary_boundary.contains(": bool"));
        assert!(!ordinary_boundary.contains("fn get_secret<"));
        assert!(!ordinary_boundary.contains("fn set_secret<"));
        assert!(!ordinary_boundary.contains("fn delete_credential<"));
        assert!(!keyring.contains("AllowPrompt"));
        assert!(!keyring.contains("allow_prompt"));
        assert!(!store.contains("AllowPrompt"));
        assert!(!store.contains("allow_prompt"));
        assert!(!store.contains("RecoveryBoundary"));
        assert!(!store.contains("InteractionPolicy"));
        assert!(store.contains("NativeInteractionLease<'static>"));
        assert!(!store.contains("MutexGuard<'static, ()>"));
        let compact_keyring = compact_whitespace(&keyring);
        assert_eq!(
            compact_keyring
                .matches(".get_secret(prompt,locator)")
                .count(),
            2
        );
        assert_eq!(
            compact_keyring
                .matches(".set_secret(prompt,invocation,locator,secret)")
                .count(),
            2
        );
        assert_eq!(
            compact_keyring
                .matches(".delete_credential(prompt,invocation,locator)")
                .count(),
            1
        );

        assert!(ADAPTERS_MOD_SOURCE.contains("mod native_interaction;"));
        assert!(!ADAPTERS_MOD_SOURCE.contains("pub(crate) mod native_interaction"));

        assert!(!recovery_contract_has_forbidden_payload(&native));
        assert!(!uses_poison_reset(&native));
        assert!(!uses_poison_reset(&keyring));
        assert!(!uses_poison_reset(&store));
    }

    #[test]
    fn explicit_recovery_is_cancelled_before_or_while_queued_without_calls() {
        let gate = NativeInteractionGate::isolated_for_test();
        let trace = Arc::new(Mutex::new(Vec::new()));
        let facade = scripted_facade(
            gate,
            trace.clone(),
            RecoveryStep::Success,
            RecoveryStep::Success,
        );
        let pre_cancelled = CancellationToken::new();
        pre_cancelled.cancel();
        assert_eq!(
            facade.diagnose_or_unlock(RecoveryTarget::CredentialStore, &pre_cancelled),
            Err(CredentialStoreFailure::Cancelled)
        );
        assert!(trace.lock().expect("recovery trace lock").is_empty());

        let held = gate.acquire().expect("live ordinary interaction lease");
        let queued_trace = Arc::new(Mutex::new(Vec::new()));
        let queued_facade = scripted_facade(
            gate,
            queued_trace.clone(),
            RecoveryStep::Success,
            RecoveryStep::Success,
        );
        let queued_cancellation = Arc::new(CancellationToken::new());
        let worker_cancellation = queued_cancellation.clone();
        let queued = std::thread::spawn(move || {
            queued_facade.diagnose_or_unlock(RecoveryTarget::CredentialStore, &worker_cancellation)
        });
        wait_until_gate_has_queued_waiter(gate);
        queued_cancellation.cancel();
        drop(held);

        assert_eq!(
            queued.join().expect("queued recovery joined"),
            Err(CredentialStoreFailure::Cancelled)
        );
        assert!(
            queued_trace
                .lock()
                .expect("queued recovery trace lock")
                .is_empty()
        );
    }

    #[test]
    fn explicit_recovery_uses_one_allow_then_distinct_forbid_verification() {
        let gate = NativeInteractionGate::isolated_for_test();
        let trace = Arc::new(Mutex::new(Vec::new()));
        let facade = scripted_facade(
            gate,
            trace.clone(),
            RecoveryStep::Success,
            RecoveryStep::Success,
        );

        assert_eq!(
            facade.diagnose_or_unlock(RecoveryTarget::CredentialStore, &CancellationToken::new(),),
            Ok(RecoveryOutcome::Ready)
        );
        assert_eq!(
            trace.lock().expect("recovery trace lock").as_slice(),
            ["allow", "forbid"]
        );
    }

    #[test]
    fn recovery_serializes_against_a_live_ordinary_session() {
        let gate = NativeInteractionGate::isolated_for_test();
        let held = gate.acquire().expect("live ordinary interaction lease");
        let trace = Arc::new(Mutex::new(Vec::new()));
        let facade = scripted_facade(
            gate,
            trace.clone(),
            RecoveryStep::Success,
            RecoveryStep::Success,
        );
        let (result_tx, result_rx) = mpsc::channel();
        let worker =
            std::thread::spawn(move || {
                result_tx
                    .send(facade.diagnose_or_unlock(
                        RecoveryTarget::CredentialStore,
                        &CancellationToken::new(),
                    ))
                    .expect("send recovery result");
            });
        wait_until_gate_has_queued_waiter(gate);
        assert!(trace.lock().expect("recovery trace lock").is_empty());
        assert!(matches!(
            result_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(held);

        assert_eq!(
            result_rx.recv().expect("recovery result"),
            Ok(RecoveryOutcome::Ready)
        );
        worker.join().expect("recovery worker joined");
        assert_eq!(
            trace.lock().expect("recovery trace lock").as_slice(),
            ["allow", "forbid"]
        );
    }

    #[test]
    fn returned_uncertainty_and_recovery_panic_permanently_stall_the_gate() {
        let returned_gate = NativeInteractionGate::isolated_for_test();
        let returned_trace = Arc::new(Mutex::new(Vec::new()));
        let returned = scripted_facade(
            returned_gate,
            returned_trace.clone(),
            RecoveryStep::Closed(CredentialStoreFailure::CommitUnknown),
            RecoveryStep::Success,
        );
        assert_eq!(
            returned
                .diagnose_or_unlock(RecoveryTarget::CredentialStore, &CancellationToken::new(),),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        assert_eq!(
            returned_trace
                .lock()
                .expect("returned uncertainty trace lock")
                .as_slice(),
            ["allow"]
        );
        assert!(matches!(
            returned_gate.acquire(),
            Err(CredentialStoreFailure::StalledWorker)
        ));

        let panic_gate = NativeInteractionGate::isolated_for_test();
        let panic_trace = Arc::new(Mutex::new(Vec::new()));
        let panicking = scripted_facade(
            panic_gate,
            panic_trace,
            RecoveryStep::Panic,
            RecoveryStep::Success,
        );
        assert_eq!(
            panicking
                .diagnose_or_unlock(RecoveryTarget::CredentialStore, &CancellationToken::new(),),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert!(matches!(
            panic_gate.acquire(),
            Err(CredentialStoreFailure::StalledWorker)
        ));
    }

    fn wait_until_gate_has_queued_waiter(gate: &NativeInteractionGate) {
        for _ in 0..100_000 {
            if gate.queued_waiters_for_test() > 0 {
                return;
            }
            std::thread::yield_now();
        }
        panic!("native interaction waiter did not queue")
    }
}
