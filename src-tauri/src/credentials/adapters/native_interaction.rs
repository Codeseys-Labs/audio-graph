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

impl Drop for NativeInteractionLease<'_> {
    fn drop(&mut self) {
        // Deliberately empty: implementing Drop is the compiler-enforced move
        // barrier that keeps every owned capability inside the guarded lease.
    }
}

#[allow(drop_bounds)]
fn native_interaction_lease_must_implement_drop<T: Drop>() {}

const _: fn() = native_interaction_lease_must_implement_drop::<NativeInteractionLease<'static>>;

const _: for<'gate> fn(
    &'gate NativeInteractionGate,
) -> Result<NativeInteractionLease<'gate>, CredentialStoreFailure> = NativeInteractionGate::acquire;
const _: for<'borrow> fn(
    &'borrow NativeInteractionLease<'static>,
) -> Result<(), CredentialStoreFailure> = NativeInteractionLease::ensure_healthy;
const _: for<'borrow> fn(
    &'borrow NativeInteractionLease<'static>,
) -> &'borrow ForbidPrompt<'static> = NativeInteractionLease::forbid_prompt;
const _: for<'borrow> fn(
    &'borrow mut NativeInteractionLease<'static>,
) -> (
    &'borrow ForbidPrompt<'static>,
    &'borrow mut MutationInvocation<'static>,
) = NativeInteractionLease::mutation_capabilities;
const _: for<'borrow> fn(&'borrow NativeInteractionLease<'static>) -> CredentialStoreFailure =
    NativeInteractionLease::latch_commit_unknown;

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
    const NATIVE_INTERACTION_BYTES: &[u8] = include_bytes!("native_interaction.rs");
    const KEYRING_ENTRY_SOURCE: &str = include_str!("keyring_entry.rs");
    const NATIVE_KEYRING_SOURCE: &str = include_str!("native_keyring.rs");
    const ADAPTERS_MOD_SOURCE: &str = include_str!("mod.rs");

    #[derive(Clone, Copy)]
    struct RustToken<'source> {
        text: &'source str,
        start: usize,
    }

    fn is_identifier_byte(byte: u8) -> bool {
        byte.is_ascii_alphanumeric() || byte == b'_'
    }

    fn blank_non_code(masked: &mut [u8], start: usize, end: usize) {
        for byte in &mut masked[start..end] {
            if !matches!(*byte, b'\n' | b'\r') {
                *byte = b' ';
            }
        }
    }

    fn raw_string_end(bytes: &[u8], start: usize) -> Option<usize> {
        if start > 0 && is_identifier_byte(bytes[start - 1]) {
            return None;
        }
        let prefix_len = if bytes[start..].starts_with(b"br") || bytes[start..].starts_with(b"cr") {
            2
        } else if bytes[start] == b'r' {
            1
        } else {
            return None;
        };
        let mut quote = start + prefix_len;
        while bytes.get(quote) == Some(&b'#') {
            quote += 1;
        }
        if bytes.get(quote) != Some(&b'"') {
            return None;
        }
        let hashes = quote - start - prefix_len;
        let mut cursor = quote + 1;
        while cursor < bytes.len() {
            let closing_end = cursor + 1 + hashes;
            if bytes[cursor] == b'"'
                && closing_end <= bytes.len()
                && bytes[cursor + 1..closing_end]
                    .iter()
                    .all(|byte| *byte == b'#')
            {
                return Some(closing_end);
            }
            cursor += 1;
        }
        Some(bytes.len())
    }

    fn quoted_string_end(bytes: &[u8], start: usize) -> Option<usize> {
        let quote = if bytes[start] == b'"' {
            start
        } else if start == 0 || !is_identifier_byte(bytes[start - 1]) {
            if matches!(bytes[start], b'b' | b'c') && bytes.get(start + 1) == Some(&b'"') {
                start + 1
            } else {
                return None;
            }
        } else {
            return None;
        };
        let mut cursor = quote + 1;
        while cursor < bytes.len() {
            match bytes[cursor] {
                b'\\' => cursor = (cursor + 2).min(bytes.len()),
                b'"' => return Some(cursor + 1),
                _ => cursor += 1,
            }
        }
        Some(bytes.len())
    }

    fn char_literal_end(bytes: &[u8], start: usize) -> Option<usize> {
        let quote = if bytes[start] == b'\'' {
            start
        } else if bytes[start] == b'b'
            && (start == 0 || !is_identifier_byte(bytes[start - 1]))
            && bytes.get(start + 1) == Some(&b'\'')
        {
            start + 1
        } else {
            return None;
        };
        let content = quote + 1;
        if bytes.get(content) == Some(&b'\\') {
            let mut cursor = content + 2;
            while cursor < bytes.len() && !matches!(bytes[cursor], b'\'' | b'\n' | b'\r') {
                cursor += 1;
            }
            return (bytes.get(cursor) == Some(&b'\'')).then_some(cursor + 1);
        }
        let character = std::str::from_utf8(bytes.get(content..)?)
            .ok()?
            .chars()
            .next()?;
        let closing = content + character.len_utf8();
        (bytes.get(closing) == Some(&b'\'')).then_some(closing + 1)
    }

    fn mask_rust_non_code(source: &str) -> String {
        let bytes = source.as_bytes();
        let mut masked = bytes.to_vec();
        let mut cursor = 0;
        while cursor < bytes.len() {
            if bytes[cursor..].starts_with(b"//") {
                let end = bytes[cursor..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(bytes.len(), |offset| cursor + offset);
                blank_non_code(&mut masked, cursor, end);
                cursor = end;
            } else if bytes[cursor..].starts_with(b"/*") {
                let mut end = cursor + 2;
                let mut depth = 1_usize;
                while end < bytes.len() && depth > 0 {
                    if bytes[end..].starts_with(b"/*") {
                        depth += 1;
                        end += 2;
                    } else if bytes[end..].starts_with(b"*/") {
                        depth -= 1;
                        end += 2;
                    } else {
                        end += 1;
                    }
                }
                blank_non_code(&mut masked, cursor, end);
                cursor = end;
            } else if let Some(end) = raw_string_end(bytes, cursor) {
                blank_non_code(&mut masked, cursor, end);
                cursor = end;
            } else if let Some(end) = quoted_string_end(bytes, cursor) {
                blank_non_code(&mut masked, cursor, end);
                cursor = end;
            } else if let Some(end) = char_literal_end(bytes, cursor) {
                blank_non_code(&mut masked, cursor, end);
                cursor = end;
            } else {
                cursor += source[cursor..]
                    .chars()
                    .next()
                    .expect("cursor remains on a UTF-8 character boundary")
                    .len_utf8();
            }
        }
        String::from_utf8(masked).expect("masking preserves valid UTF-8")
    }

    fn rust_tokens(source: &str) -> Vec<RustToken<'_>> {
        let bytes = source.as_bytes();
        let mut tokens = Vec::new();
        let mut cursor = 0;
        while cursor < bytes.len() {
            if bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            } else if bytes[cursor].is_ascii_alphabetic() || bytes[cursor] == b'_' {
                let start = cursor;
                cursor += 1;
                while cursor < bytes.len() && is_identifier_byte(bytes[cursor]) {
                    cursor += 1;
                }
                tokens.push(RustToken {
                    text: &source[start..cursor],
                    start,
                });
            } else {
                let start = cursor;
                let width =
                    if bytes[cursor..].starts_with(b"->") || bytes[cursor..].starts_with(b"::") {
                        2
                    } else {
                        source[cursor..]
                            .chars()
                            .next()
                            .expect("cursor remains on a UTF-8 character boundary")
                            .len_utf8()
                    };
                cursor += width;
                tokens.push(RustToken {
                    text: &source[start..cursor],
                    start,
                });
            }
        }
        tokens
    }

    fn root_test_module_ranges(source: &str) -> Option<Vec<(usize, usize)>> {
        const TEST_MODULE_TOKENS: [&str; 10] =
            ["#", "[", "cfg", "(", "test", ")", "]", "mod", "tests", "{"];
        let masked = mask_rust_non_code(source);
        let tokens = rust_tokens(&masked);
        let mut ranges = Vec::new();
        let mut brace_depth = 0_usize;
        let mut index = 0_usize;
        while index < tokens.len() {
            if brace_depth == 0
                && tokens[index..]
                    .iter()
                    .take(TEST_MODULE_TOKENS.len())
                    .map(|candidate| candidate.text)
                    .eq(TEST_MODULE_TOKENS)
            {
                let open = index + TEST_MODULE_TOKENS.len() - 1;
                let close = matching_brace(&tokens, open)?;
                let range_start = outer_attribute_cluster_start(&tokens, index);
                ranges.push((tokens[range_start].start, tokens[close].start + 1));
                index = close + 1;
                continue;
            }
            match tokens[index].text {
                "{" => brace_depth += 1,
                "}" => brace_depth = brace_depth.checked_sub(1)?,
                _ => {}
            }
            index += 1;
        }
        (brace_depth == 0).then_some(ranges)
    }

    fn production_source(source: &str) -> String {
        let Some(ranges) = root_test_module_ranges(source) else {
            return source.to_owned();
        };
        let mut production = source.as_bytes().to_vec();
        for (start, end) in ranges {
            blank_non_code(&mut production, start, end);
        }
        String::from_utf8(production).expect("test-module masking preserves valid UTF-8")
    }

    fn root_test_module_layout_is_closed(source: &str) -> bool {
        root_test_module_ranges(source).is_some_and(|ranges| ranges.len() == 1)
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

    fn matching_brace(tokens: &[RustToken<'_>], open: usize) -> Option<usize> {
        if tokens.get(open)?.text != "{" {
            return None;
        }
        let mut depth = 0_usize;
        for (index, token) in tokens.iter().enumerate().skip(open) {
            match token.text {
                "{" => depth += 1,
                "}" => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn matching_square_bracket(tokens: &[RustToken<'_>], open: usize) -> Option<usize> {
        if tokens.get(open)?.text != "[" {
            return None;
        }
        let mut depth = 0_usize;
        for (index, token) in tokens.iter().enumerate().skip(open) {
            match token.text {
                "[" => depth += 1,
                "]" => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn matching_open_square_bracket(tokens: &[RustToken<'_>], close: usize) -> Option<usize> {
        if tokens.get(close)?.text != "]" {
            return None;
        }
        let mut depth = 0_usize;
        for index in (0..=close).rev() {
            match tokens[index].text {
                "]" => depth += 1,
                "[" => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn outer_attribute_cluster_start(tokens: &[RustToken<'_>], attribute: usize) -> usize {
        let mut start = attribute;
        while let Some(close) = start.checked_sub(1) {
            if tokens[close].text != "]" {
                break;
            }
            let Some(open) = matching_open_square_bracket(tokens, close) else {
                break;
            };
            let Some(hash) = open.checked_sub(1) else {
                break;
            };
            if tokens[hash].text != "#"
                || hash
                    .checked_sub(1)
                    .and_then(|index| tokens.get(index))
                    .is_some_and(|token| token.text == "!")
            {
                break;
            }
            start = hash;
        }
        start
    }

    fn impl_block_ranges(tokens: &[RustToken<'_>]) -> Vec<(usize, usize, usize)> {
        let mut ranges = Vec::new();
        for (impl_index, token) in tokens.iter().enumerate() {
            if token.text != "impl" {
                continue;
            }
            let mut index = impl_index + 1;
            let mut angle_depth = 0_usize;
            let mut parenthesis_depth = 0_usize;
            let mut square_depth = 0_usize;
            let mut open = None;
            while index < tokens.len() {
                match tokens[index].text {
                    "{" if angle_depth == 0 && parenthesis_depth == 0 && square_depth == 0 => {
                        open = Some(index);
                        break;
                    }
                    "{" => {
                        let Some(close) = matching_brace(tokens, index) else {
                            break;
                        };
                        index = close;
                    }
                    ";" if angle_depth == 0 && parenthesis_depth == 0 && square_depth == 0 => {
                        break;
                    }
                    "<" => angle_depth += 1,
                    ">" => angle_depth = angle_depth.saturating_sub(1),
                    "(" => parenthesis_depth += 1,
                    ")" => parenthesis_depth = parenthesis_depth.saturating_sub(1),
                    "[" => square_depth += 1,
                    "]" => square_depth = square_depth.saturating_sub(1),
                    _ => {}
                }
                index += 1;
            }
            let Some(open) = open else {
                continue;
            };
            if let Some(close) = matching_brace(tokens, open) {
                ranges.push((impl_index, open, close));
            }
        }
        ranges
    }

    fn declaration_start(tokens: &[RustToken<'_>], item: usize) -> usize {
        (0..item)
            .rev()
            .find(|index| matches!(tokens[*index].text, ";" | "{" | "}"))
            .map_or(0, |index| index + 1)
    }

    fn declaration_is_test_only(tokens: &[RustToken<'_>], item: usize) -> bool {
        let mut cursor = declaration_start(tokens, item);
        while cursor < item {
            if tokens[cursor].text == "#" && tokens.get(cursor + 1).is_some_and(|t| t.text == "[") {
                let Some(close) = matching_square_bracket(tokens, cursor + 1) else {
                    return false;
                };
                if close < item && normalized_tokens(&tokens[cursor..=close]) == "#[cfg(test)]" {
                    return true;
                }
                cursor = close + 1;
            } else {
                cursor += 1;
            }
        }
        false
    }

    fn normalized_tokens(tokens: &[RustToken<'_>]) -> String {
        tokens.iter().map(|token| token.text).collect()
    }

    fn impl_method_inventory(source: &str, owner: &str) -> Vec<(String, Vec<String>)> {
        let masked = mask_rust_non_code(source);
        let tokens = rust_tokens(&masked);
        let mut inventory = Vec::new();
        for (implementation, open, close) in impl_block_ranges(&tokens) {
            if declaration_is_test_only(&tokens, implementation)
                || !tokens[implementation..open]
                    .iter()
                    .any(|token| token.text == owner)
            {
                continue;
            }

            let header =
                normalized_tokens(&tokens[declaration_start(&tokens, implementation)..open]);
            let mut methods = Vec::new();
            let mut depth = 0_usize;
            let mut method_signature_until = None;
            for function in open + 1..close {
                if depth == 0 && tokens[function].text == "fn" {
                    let Some((terminator, _)) = function_terminator(&tokens, function) else {
                        if !declaration_is_test_only(&tokens, function) {
                            methods.push("<malformed-method>".to_owned());
                        }
                        continue;
                    };
                    method_signature_until = Some(terminator);
                    if !declaration_is_test_only(&tokens, function) {
                        let declaration_start = declaration_start(&tokens, function);
                        methods.push(normalized_tokens(&tokens[declaration_start..terminator]));
                    }
                }
                if depth == 0
                    && method_signature_until.is_none_or(|end| function > end)
                    && (matches!(tokens[function].text, "const" | "static" | "type")
                        || tokens[function].text == "!")
                {
                    methods.push(format!("<unexpected-impl-item:{}>", tokens[function].text));
                }
                match tokens[function].text {
                    "{" => depth += 1,
                    "}" => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
            inventory.push((header, methods));
        }
        inventory
    }

    #[derive(Debug, PartialEq, Eq)]
    struct RootStructInventory {
        header: String,
        fields: Option<String>,
    }

    fn root_struct_inventories(source: &str, owner: &str) -> Option<Vec<RootStructInventory>> {
        let masked = mask_rust_non_code(source);
        let tokens = rust_tokens(&masked);
        let mut inventory = Vec::new();
        let mut brace_depth = 0_usize;
        for (declaration, token) in tokens.iter().enumerate() {
            if brace_depth == 0
                && token.text == "struct"
                && tokens
                    .get(declaration + 1)
                    .is_some_and(|name| name.text == owner)
            {
                let terminator = (declaration + 2..tokens.len())
                    .find(|index| matches!(tokens[*index].text, "{" | ";"))?;
                let fields = if tokens[terminator].text == "{" {
                    let close = matching_brace(&tokens, terminator)?;
                    Some(normalized_tokens(&tokens[terminator + 1..close]))
                } else {
                    None
                };
                inventory.push(RootStructInventory {
                    header: normalized_tokens(
                        &tokens[declaration_start(&tokens, declaration)..terminator],
                    ),
                    fields,
                });
            }
            match token.text {
                "{" => brace_depth += 1,
                "}" => brace_depth = brace_depth.checked_sub(1)?,
                _ => {}
            }
        }
        (brace_depth == 0).then_some(inventory)
    }

    fn brace_opens_inline_module(tokens: &[RustToken<'_>], open: usize) -> bool {
        tokens[declaration_start(tokens, open)..open]
            .windows(2)
            .any(|pair| {
                pair[0].text == "mod"
                    && pair[1]
                        .text
                        .as_bytes()
                        .first()
                        .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
            })
    }

    fn source_has_item_position_macro(source: &str) -> bool {
        let masked = mask_rust_non_code(source);
        let tokens = rust_tokens(&masked);
        let mut module_item_scopes = Vec::new();
        for (index, token) in tokens.iter().enumerate() {
            let is_module_item_position = module_item_scopes.iter().all(|scope| *scope);
            if is_module_item_position && token.text == "!" {
                let previous = index.checked_sub(1).and_then(|index| tokens.get(index));
                let next = tokens.get(index + 1);
                if previous.is_some_and(|token| token.text == "macro_rules")
                    || (previous.is_some_and(|token| {
                        token.text != "#"
                            && token
                                .text
                                .as_bytes()
                                .first()
                                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
                    }) && next.is_some_and(|token| matches!(token.text, "(" | "[" | "{")))
                {
                    return true;
                }
            }
            match token.text {
                "{" => module_item_scopes
                    .push(is_module_item_position && brace_opens_inline_module(&tokens, index)),
                "}" if module_item_scopes.pop().is_none() => return true,
                "}" => {}
                _ => {}
            }
        }
        !module_item_scopes.is_empty()
    }

    fn source_has_forbidden_lease_escape_primitive(source: &str) -> bool {
        let masked = mask_rust_non_code(source);
        let tokens = rust_tokens(&masked);
        tokens
            .iter()
            .any(|token| matches!(token.text, "unsafe" | "ManuallyDrop" | "forget"))
            || tokens.windows(3).any(|tokens| {
                tokens[0].text == "ptr" && tokens[1].text == "::" && tokens[2].text == "read"
            })
    }

    fn native_gate_and_lease_inventory_is_closed(source: &str) -> bool {
        let production = production_source(source);
        let masked = mask_rust_non_code(&production);
        let gate = impl_method_inventory(&production, "NativeInteractionGate");
        let lease = impl_method_inventory(&production, "NativeInteractionLease");
        let gate_declarations = root_struct_inventories(&production, "NativeInteractionGate");
        let lease_declarations = root_struct_inventories(&production, "NativeInteractionLease");

        gate_declarations
            == Some(vec![RootStructInventory {
                header: "pub(super)structNativeInteractionGate".to_owned(),
                fields: Some(
                    "access:Mutex<()>,stalled:AtomicBool,#[cfg(test)]queued_waiters:std::sync::atomic::AtomicUsize,"
                        .to_owned(),
                ),
            }])
            && lease_declarations
                == Some(vec![RootStructInventory {
                    header: "pub(super)structNativeInteractionLease<'gate>".to_owned(),
                    fields: Some(
                        "_guard:MutexGuard<'gate,()>,forbid_prompt:ForbidPrompt<'gate>,mutation_invocation:MutationInvocation<'gate>,"
                            .to_owned(),
                    ),
                }])
            && gate == vec![(
            "implNativeInteractionGate".to_owned(),
            vec![
                "fnnew()->Self".to_owned(),
                "pub(super)fnacquire(&self)->Result<NativeInteractionLease<'_>,CredentialStoreFailure>"
                    .to_owned(),
                "fnis_stalled(&self)->bool".to_owned(),
                "fnlatch_stalled(&self)".to_owned(),
            ],
        )] && lease
            == vec![
                (
                    "implDropforNativeInteractionLease<'_>".to_owned(),
                    vec!["fndrop(&mutself)".to_owned()],
                ),
                (
                    "impl<'gate>NativeInteractionLease<'gate>".to_owned(),
                    vec![
                        "pub(super)fnensure_healthy(&self)->Result<(),CredentialStoreFailure>"
                            .to_owned(),
                        "pub(super)fnforbid_prompt(&self)->&ForbidPrompt<'gate>".to_owned(),
                        "pub(super)fnmutation_capabilities<'borrow>(&'borrowmutself,)->(&'borrowForbidPrompt<'gate>,&'borrowmutMutationInvocation<'gate>,)"
                            .to_owned(),
                        "pub(super)fnlatch_commit_unknown(&self)->CredentialStoreFailure"
                            .to_owned(),
                        "fnlatch_stalled(&self)->CredentialStoreFailure".to_owned(),
                    ],
                ),
            ]
            && !aliases_capability(&masked, "NativeInteractionGate")
            && !aliases_capability(&masked, "NativeInteractionLease")
            && !imports_capability_alias(&masked, "NativeInteractionGate")
            && !imports_capability_alias(&masked, "NativeInteractionLease")
            && !source_has_item_position_macro(&production)
            && !source_has_forbidden_lease_escape_primitive(&production)
    }

    fn append_to_impl(source: &str, header: &str, addition: &str) -> Option<String> {
        let masked = mask_rust_non_code(source);
        let tokens = rust_tokens(&masked);
        let (_, _, close) =
            impl_block_ranges(&tokens)
                .into_iter()
                .find(|(implementation, open, _)| {
                    normalized_tokens(&tokens[*implementation..*open]) == header
                })?;
        let mut mutant = source.to_owned();
        mutant.insert_str(tokens[close].start, addition);
        Some(mutant)
    }

    fn remove_impl(source: &str, header: &str) -> Option<String> {
        let masked = mask_rust_non_code(source);
        let tokens = rust_tokens(&masked);
        let (implementation, _, close) =
            impl_block_ranges(&tokens)
                .into_iter()
                .find(|(implementation, open, _)| {
                    normalized_tokens(&tokens[*implementation..*open]) == header
                })?;
        let mut mutant = source.to_owned();
        mutant.replace_range(tokens[implementation].start..tokens[close].start + 1, "");
        Some(mutant)
    }

    fn function_terminator(
        tokens: &[RustToken<'_>],
        function: usize,
    ) -> Option<(usize, Option<usize>)> {
        for index in function + 2..tokens.len() {
            match tokens[index].text {
                "{" => return Some((index, matching_brace(tokens, index))),
                ";" => return Some((index, None)),
                _ => {}
            }
        }
        None
    }

    fn function_is_externally_visible(tokens: &[RustToken<'_>], function: usize) -> bool {
        let declaration_start = (0..function)
            .rev()
            .find(|index| matches!(tokens[*index].text, ";" | "{" | "}"))
            .map_or(0, |index| index + 1);
        tokens[declaration_start..function]
            .iter()
            .any(|token| token.text == "pub")
    }

    fn token_range_contains_capability_literal(
        tokens: &[RustToken<'_>],
        start: usize,
        end: usize,
        capability: &str,
    ) -> bool {
        tokens[start..end]
            .windows(2)
            .any(|pair| pair[0].text == capability && pair[1].text == "{")
    }

    fn return_type_contains_owned_capability(
        tokens: &[RustToken<'_>],
        start: usize,
        end: usize,
        capability: &str,
    ) -> bool {
        tokens[start..end]
            .iter()
            .enumerate()
            .filter(|(_, token)| token.text == capability)
            .any(|(offset, _)| {
                !tokens[start..start + offset]
                    .iter()
                    .rev()
                    .take_while(|token| !matches!(token.text, "," | "(" | ")" | "->" | "=" | ";"))
                    .any(|token| token.text == "&")
            })
    }

    fn externally_reachable_capability_factory(source: &str, capability: &str) -> bool {
        let masked = mask_rust_non_code(source);
        let tokens = rust_tokens(&masked);
        let impl_ranges = impl_block_ranges(&tokens);
        for (function, token) in tokens.iter().enumerate() {
            if token.text != "fn"
                || !tokens.get(function + 1).is_some_and(|name| {
                    name.text
                        .as_bytes()
                        .first()
                        .is_some_and(|first| first.is_ascii_alphabetic() || *first == b'_')
                })
                || !function_is_externally_visible(&tokens, function)
            {
                continue;
            }
            let Some((terminator, body_close)) = function_terminator(&tokens, function) else {
                return true;
            };
            let is_impl_method = impl_ranges
                .iter()
                .any(|(_, open, close)| *open < function && function < *close);
            if is_impl_method {
                let return_arrow =
                    (function + 2..terminator).find(|index| tokens[*index].text == "->");
                if return_arrow.is_some_and(|arrow| {
                    return_type_contains_owned_capability(
                        &tokens,
                        arrow + 1,
                        terminator,
                        capability,
                    )
                }) {
                    return true;
                }
                continue;
            }
            let return_arrow = (function + 2..terminator).find(|index| tokens[*index].text == "->");
            if return_arrow.is_some_and(|arrow| {
                tokens[arrow + 1..terminator]
                    .iter()
                    .any(|candidate| candidate.text == capability)
            }) {
                return true;
            }
            if body_close.is_some_and(|close| {
                token_range_contains_capability_literal(&tokens, terminator + 1, close, capability)
            }) {
                return true;
            }
        }
        false
    }

    fn intended_method_body(
        tokens: &[RustToken<'_>],
        owner: &str,
        method: &str,
    ) -> Option<(usize, usize)> {
        for (impl_index, open, close) in impl_block_ranges(tokens) {
            if !tokens[impl_index + 1..open]
                .iter()
                .any(|token| token.text == owner)
            {
                continue;
            }
            let mut depth = 0_usize;
            for index in open + 1..close {
                if depth == 0
                    && tokens[index].text == "fn"
                    && tokens
                        .get(index + 1)
                        .is_some_and(|name| name.text == method)
                {
                    let (body_open, body_close) = function_terminator(tokens, index)?;
                    return Some((body_open, body_close?));
                }
                match tokens[index].text {
                    "{" => depth += 1,
                    "}" => depth = depth.checked_sub(1)?,
                    _ => {}
                }
            }
        }
        None
    }

    fn capability_construction_is_lexically_sealed(
        source: &str,
        capability: &str,
        owner: &str,
        method: &str,
    ) -> bool {
        let masked = mask_rust_non_code(source);
        let tokens = rust_tokens(&masked);
        let Some((body_open, body_close)) = intended_method_body(&tokens, owner, method) else {
            return false;
        };
        let literal_sites: Vec<usize> = tokens
            .windows(2)
            .enumerate()
            .filter_map(|(index, pair)| {
                (pair[0].text == capability && pair[1].text == "{").then_some(index)
            })
            .collect();
        literal_sites.len() == 1 && body_open < literal_sites[0] && literal_sites[0] < body_close
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

    fn imports_capability_alias(source: &str, capability: &str) -> bool {
        let tokens = rust_tokens(source);
        tokens.iter().enumerate().any(|(use_index, token)| {
            if token.text != "use" {
                return false;
            }
            let end = (use_index + 1..tokens.len())
                .find(|index| tokens[*index].text == ";")
                .unwrap_or(tokens.len());
            let statement = &tokens[use_index..end];
            statement.iter().any(|token| token.text == capability)
                && statement.iter().any(|token| token.text == "as")
        })
    }

    fn token_has_forbidden_convenience(source: &str, capability: &str) -> bool {
        capability_has_forbidden_derive(source, capability)
            || capability_has_impl(source, capability)
            || aliases_capability(source, capability)
            || externally_reachable_capability_factory(source, capability)
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
    fn inventory_rejects_arbitrarily_named_forbid_prompt_free_factory() {
        let mutant = r#"
pub(super) struct ForbidPrompt<'gate> {
    _seal: ordinary_prompt::Seal<'gate>,
}

pub(
    in crate::credentials::adapters
)
fn fabricate_ordinary<'gate>(
    gate: &'gate NativeInteractionGate,
)
    -> crate::credentials::adapters::native_interaction::
        ForbidPrompt<'gate>
{
    ForbidPrompt {
        _seal: ordinary_prompt::mint(gate),
    }
}
"#;

        assert!(token_has_forbidden_convenience(mutant, "ForbidPrompt"));
    }

    #[test]
    fn inventory_rejects_arbitrarily_named_allow_prompt_free_factory() {
        let mutant = r#"
pub(super) struct AllowPrompt<'lease, 'gate> {
    lease: &'lease NativeInteractionLease<'gate>,
}

pub(super)
fn assemble_recovery<'lease, 'gate>(
    lease: &'lease NativeInteractionLease<'gate>,
)
    -> self::recovery_facade::
        AllowPrompt<'lease, 'gate>
{
    self::recovery_facade::AllowPrompt { lease }
}
"#;

        assert!(token_has_forbidden_convenience(mutant, "AllowPrompt"));
    }

    #[test]
    fn inventory_rejects_owned_capability_returning_lease_methods() {
        let direct = r#"
impl<'gate> NativeInteractionLease<'gate> {
    pub(super) fn into_forbid_prompt(self)
        -> crate::credentials::adapters::native_interaction::
            ForbidPrompt<'gate>
    {
        self.forbid_prompt
    }
}
"#;
        let wrapped = r#"
impl<'gate> NativeInteractionLease<'gate> {
    pub(super) fn into_wrapped_forbid_prompt(self)
        -> Option<self::ForbidPrompt<'gate>>
    {
        Some(self.forbid_prompt)
    }
}
"#;
        let tuple = r#"
impl<'gate> NativeInteractionLease<'gate> {
    pub(in crate::credentials::adapters)
    fn into_tuple(self)
        -> (RecoveryOutcome, super::ForbidPrompt<'gate>)
    {
        (RecoveryOutcome::Ready, self.forbid_prompt)
    }
}
"#;
        let borrowed = r#"
impl<'gate> NativeInteractionLease<'gate> {
    pub(super) fn forbid_prompt(&self) -> &ForbidPrompt<'gate> {
        &self.forbid_prompt
    }
}
"#;
        let borrowed_tuple = r#"
impl<'gate> NativeInteractionLease<'gate> {
    pub(super) fn mutation_capabilities<'borrow>(
        &'borrow mut self,
    ) -> (
        &'borrow ForbidPrompt<'gate>,
        &'borrow mut MutationInvocation<'gate>,
    ) {
        (&self.forbid_prompt, &mut self.mutation_invocation)
    }
}
"#;

        assert_eq!(
            [
                externally_reachable_capability_factory(direct, "ForbidPrompt"),
                externally_reachable_capability_factory(wrapped, "ForbidPrompt"),
                externally_reachable_capability_factory(tuple, "ForbidPrompt"),
                externally_reachable_capability_factory(borrowed, "ForbidPrompt"),
                externally_reachable_capability_factory(borrowed_tuple, "ForbidPrompt"),
            ],
            [true, true, true, false, false]
        );
    }

    #[test]
    fn production_inventory_freezes_all_gate_and_lease_methods_and_fields() {
        let native = production_source(NATIVE_INTERACTION_SOURCE);
        assert!(
            native_gate_and_lease_inventory_is_closed(&native),
            "gate={:?}, lease={:?}, gate_declarations={:?}, lease_declarations={:?}, item_macro={}, forbidden={}",
            impl_method_inventory(&native, "NativeInteractionGate"),
            impl_method_inventory(&native, "NativeInteractionLease"),
            root_struct_inventories(&native, "NativeInteractionGate"),
            root_struct_inventories(&native, "NativeInteractionLease"),
            source_has_item_position_macro(&native),
            source_has_forbidden_lease_escape_primitive(&native),
        );

        for addition in [
            r#"
    fn into_forbid_prompt(self) -> ForbidPrompt<'gate> {
        loop {}
    }
"#,
            r#"
    pub(super) fn into_wrapped_prompt(self)
        -> Option<ForbidPrompt<'gate>>
    {
        loop {}
    }
"#,
            r#"
    pub(in crate::credentials::adapters)
    fn into_prompt_tuple(self)
        -> (
            crate::credentials::adapters::native_interaction::ForbidPrompt<'gate>,
            MutationInvocation<'gate>,
        )
    {
        loop {}
    }
"#,
            r#"
    fn with_prompt<R>(
        &self,
        callback: impl FnOnce(&ForbidPrompt<'gate>) -> R,
    ) -> R {
        callback(&self.forbid_prompt)
    }
"#,
        ] {
            let mutant = append_to_impl(
                &native,
                "impl<'gate>NativeInteractionLease<'gate>",
                addition,
            )
            .expect("lease implementation exists");
            assert!(!native_gate_and_lease_inventory_is_closed(&mutant));
        }

        let gate_callback = append_to_impl(
            &native,
            "implNativeInteractionGate",
            r#"
    fn with_lease<R>(
        &self,
        callback: impl FnOnce(&NativeInteractionLease<'_>) -> R,
    ) -> R {
        loop {}
    }
"#,
        )
        .expect("gate implementation exists");
        assert!(!native_gate_and_lease_inventory_is_closed(&gate_callback));

        let trait_escape = format!(
            "{native}\ntrait LeaseEscape {{ fn escape(self) -> ForbidPrompt<'static>; }}\n\
             impl LeaseEscape for NativeInteractionLease<'static> {{\n\
                 fn escape(self) -> ForbidPrompt<'static> {{ loop {{}} }}\n\
             }}\n"
        );
        assert!(!native_gate_and_lease_inventory_is_closed(&trait_escape));

        let const_generic_trait_escape = format!(
            "{native}\n#[allow(dead_code)]\n\
             trait ConstLeaseEscape<'gate, const N: usize> {{\n\
             fn with_prompt<R, F>(&self, callback: F) -> R\n\
             where F: FnOnce(&ForbidPrompt<'gate>) -> R;\n\
             }}\n\
             impl<'gate> ConstLeaseEscape<'gate, {{ 1 + 1 }}>\n\
             for NativeInteractionLease<'gate> {{\n\
             fn with_prompt<R, F>(&self, callback: F) -> R\n\
             where F: FnOnce(&ForbidPrompt<'gate>) -> R {{\n\
             callback(&self.forbid_prompt)\n\
             }}\n\
             }}\n"
        );
        assert!(!native_gate_and_lease_inventory_is_closed(
            &const_generic_trait_escape
        ));

        let missing_drop = remove_impl(&native, "implDropforNativeInteractionLease<'_>")
            .expect("lease Drop implementation exists");
        assert!(!native_gate_and_lease_inventory_is_closed(&missing_drop));

        let optional_prompt = native.replacen(
            "forbid_prompt: ForbidPrompt<'gate>",
            "forbid_prompt: Option<ForbidPrompt<'gate>>",
            1,
        );
        assert_ne!(optional_prompt, native);
        assert!(!native_gate_and_lease_inventory_is_closed(&optional_prompt));

        for alias in [
            "type LeaseAlias<'gate> = NativeInteractionLease<'gate>;",
            "type GateAlias = NativeInteractionGate;",
            "use self::NativeInteractionLease as LeaseAlias;",
            "use self::NativeInteractionGate as GateAlias;",
        ] {
            let mutant = format!("{native}\n{alias}\n");
            assert!(!native_gate_and_lease_inventory_is_closed(&mutant));
        }

        for accessor in [
            "pub(super) fn forbid_prompt(self) -> ForbidPrompt<'gate>",
            "pub(super) fn forbid_prompt(self) -> crate::credentials::adapters::native_interaction::ForbidPrompt<'gate>",
            "pub(super) fn forbid_prompt(&self) -> (&ForbidPrompt<'gate>, ForbidPrompt<'gate>)",
        ] {
            let mutant = native.replacen(
                "pub(super) fn forbid_prompt(&self) -> &ForbidPrompt<'gate>",
                accessor,
                1,
            );
            assert_ne!(mutant, native);
            assert!(!native_gate_and_lease_inventory_is_closed(&mutant));
        }

        for associated_item in [
            "\n    pub(super) const ESCAPE: fn() = escape;\n",
            "\n    generate_escape_method!();\n",
        ] {
            let mutant = append_to_impl(
                &native,
                "impl<'gate>NativeInteractionLease<'gate>",
                associated_item,
            )
            .expect("lease implementation exists");
            assert!(!native_gate_and_lease_inventory_is_closed(&mutant));
        }

        let attributed_method = native.replacen(
            "pub(super) fn forbid_prompt(&self) -> &ForbidPrompt<'gate>",
            "#[inline]\n    pub(super) fn forbid_prompt(&self) -> &ForbidPrompt<'gate>",
            1,
        );
        assert_ne!(attributed_method, native);
        assert!(!native_gate_and_lease_inventory_is_closed(
            &attributed_method
        ));

        let attributed_impl = native.replacen(
            "impl<'gate> NativeInteractionLease<'gate> {",
            "#[allow(dead_code)]\nimpl<'gate> NativeInteractionLease<'gate> {",
            1,
        );
        assert_ne!(attributed_impl, native);
        assert!(!native_gate_and_lease_inventory_is_closed(&attributed_impl));

        let cfg_attr_method = native.replacen(
            "pub(super) fn forbid_prompt(&self) -> &ForbidPrompt<'gate>",
            "#[cfg_attr(any(), cfg(test))]\n    pub(super) fn forbid_prompt(&self) -> &ForbidPrompt<'gate>",
            1,
        );
        assert_ne!(cfg_attr_method, native);
        assert!(!native_gate_and_lease_inventory_is_closed(&cfg_attr_method));

        let nested_doc_attribute = native.replacen(
            "pub(super) fn forbid_prompt(&self) -> &ForbidPrompt<'gate>",
            "#[allow(dead_code)]\n    #[doc = stringify!(#[cfg(test)])]\n    pub(super) fn forbid_prompt(&self) -> &ForbidPrompt<'gate>",
            1,
        );
        assert_ne!(nested_doc_attribute, native);
        assert!(!native_gate_and_lease_inventory_is_closed(
            &nested_doc_attribute
        ));

        let cfg_attr_impl = native.replacen(
            "impl<'gate> NativeInteractionLease<'gate> {",
            "#[cfg_attr(any(), cfg(test))]\nimpl<'gate> NativeInteractionLease<'gate> {",
            1,
        );
        assert_ne!(cfg_attr_impl, native);
        assert!(!native_gate_and_lease_inventory_is_closed(&cfg_attr_impl));

        let root_macro = format!(
            "{native}\nmacro_rules! add_escape {{ ($owner:ident) => {{\n\
             impl<'gate> $owner<'gate> {{ fn escape(&self) {{}} }}\n\
             }} }}\nadd_escape!(NativeInteractionLease);\n"
        );
        assert!(source_has_item_position_macro(&root_macro));
        assert!(!native_gate_and_lease_inventory_is_closed(&root_macro));

        let nested_item_macro = format!(
            "{native}\n#[allow(dead_code)]\nmod lease_escape_bypass {{\n\
             use super::{{ForbidPrompt, NativeInteractionLease}};\n\
             macro_rules! add_escape {{ ($owner:ident) => {{\n\
             impl<'gate> $owner<'gate> {{\n\
             pub(super) fn with_prompt<R, F>(&self, callback: F) -> R\n\
             where F: FnOnce(&ForbidPrompt<'gate>) -> R,\n\
             {{ callback(&self.forbid_prompt) }}\n\
             }}\n\
             }} }}\nadd_escape!(NativeInteractionLease);\n}}\n"
        );
        assert!(source_has_item_position_macro(&nested_item_macro));
        assert!(!native_gate_and_lease_inventory_is_closed(
            &nested_item_macro
        ));

        for protected_declaration_drift in [
            native.replacen(
                "pub(super) struct NativeInteractionGate {",
                "#[repr(C)]\npub(super) struct NativeInteractionGate {",
                1,
            ),
            native.replacen(
                "pub(super) struct NativeInteractionLease<'gate> {",
                "#[repr(C)]\npub(super) struct NativeInteractionLease<'gate> {",
                1,
            ),
            native.replacen(
                "    stalled: AtomicBool,",
                "    stalled: AtomicBool,\n    extra_state: bool,",
                1,
            ),
        ] {
            assert_ne!(protected_declaration_drift, native);
            assert!(!native_gate_and_lease_inventory_is_closed(
                &protected_declaration_drift
            ));
        }
    }

    #[test]
    fn production_inventory_rejects_destructor_escape_primitives() {
        for mutant in [
            "fn escape() { unsafe {} }",
            "fn escape<T>(lease: T) { let _ = core::mem::ManuallyDrop::new(lease); }",
            "fn escape<T>(lease: T) { std::mem::forget(lease); }",
            "fn escape<T>(lease: &T) { core :: ptr :: read(lease); }",
        ] {
            assert!(source_has_forbidden_lease_escape_primitive(mutant));
        }

        let controls = r###"
#![forbid(unsafe_code)]
// unsafe { core::mem::ManuallyDrop::new(value); std::mem::forget(value); }
const COMMENTARY: &str = r#"core::ptr::read(value)"#;
"###;
        assert!(!source_has_forbidden_lease_escape_primitive(controls));
    }

    #[test]
    fn inventory_rejects_capability_literals_hidden_in_visible_free_fn_bodies() {
        let mutant = r#"
pub(super) struct ForbidPrompt<'gate> {
    _seal: ordinary_prompt::Seal<'gate>,
}

pub(super) struct AllowPrompt<'lease, 'gate> {
    lease: &'lease NativeInteractionLease<'gate>,
}

pub(super) fn conceal_ordinary_inside_lease<'gate>(
    gate: &'gate NativeInteractionGate,
) -> NativeInteractionLease<'gate> {
    let forbid_prompt = ForbidPrompt {
        _seal: ordinary_prompt::mint(gate),
    };
    build_lease(forbid_prompt)
}

pub(in crate::credentials::adapters)
fn conceal_recovery_inside_outcome<'lease, 'gate>(
    lease: &'lease NativeInteractionLease<'gate>,
) -> RecoveryOutcome {
    let allow_prompt = self::recovery_facade::AllowPrompt { lease };
    run_recovery(allow_prompt)
}
"#;

        assert_eq!(
            [
                token_has_forbidden_convenience(mutant, "ForbidPrompt"),
                token_has_forbidden_convenience(mutant, "AllowPrompt"),
            ],
            [true, true]
        );
    }

    #[test]
    fn inventory_requires_capability_literals_to_remain_in_intended_methods() {
        let displaced_forbid = r#"
pub(super) struct ForbidPrompt<'gate> {
    _seal: ordinary_prompt::Seal<'gate>,
}
impl NativeInteractionGate {
    pub(super) fn acquire(&self) -> NativeInteractionLease<'_> {
        build_lease(self)
    }
}
fn local_lease_factory<'gate>(gate: &'gate NativeInteractionGate) -> NativeInteractionLease<'gate> {
    let forbid_prompt = ForbidPrompt {
        _seal: ordinary_prompt::mint(gate),
    };
    build_lease_with(forbid_prompt)
}
"#;
        assert!(!capability_construction_is_lexically_sealed(
            displaced_forbid,
            "ForbidPrompt",
            "NativeInteractionGate",
            "acquire",
        ));

        let displaced_allow = r#"
pub(super) struct AllowPrompt<'lease, 'gate> {
    lease: &'lease NativeInteractionLease<'gate>,
}
impl NativeRecoveryFacade {
    pub(in crate::credentials::adapters) fn diagnose_or_unlock(
        &self,
    ) -> RecoveryOutcome {
        run_recovery()
    }
}
fn local_recovery<'lease, 'gate>(
    lease: &'lease NativeInteractionLease<'gate>,
) -> RecoveryOutcome {
    let allow_prompt = AllowPrompt { lease };
    run_with(allow_prompt)
}
"#;
        assert!(!capability_construction_is_lexically_sealed(
            displaced_allow,
            "AllowPrompt",
            "NativeRecoveryFacade",
            "diagnose_or_unlock",
        ));
    }

    #[test]
    fn production_inventory_does_not_truncate_at_test_delimiter_in_raw_string() {
        let mutant = r###"
pub(super) struct ForbidPrompt<'gate> {
    _seal: ordinary_prompt::Seal<'gate>,
}
const RAW_DELIMITER: &str = r#"#[cfg(test)]
mod tests {"#;
pub(super) fn fabricate_after_raw_delimiter<'gate>(
    gate: &'gate NativeInteractionGate,
) -> ForbidPrompt<'gate> {
    ForbidPrompt {
        _seal: ordinary_prompt::mint(gate),
    }
}
#[cfg(test)]
mod tests {
}
"###;

        let production = production_source(mutant);
        assert!(production.contains("fabricate_after_raw_delimiter"));
        assert!(token_has_forbidden_convenience(&production, "ForbidPrompt"));
    }

    #[test]
    fn production_inventory_does_not_truncate_at_test_delimiter_in_block_comment() {
        let mutant = r#"
pub(super) struct AllowPrompt<'lease, 'gate> {
    lease: &'lease NativeInteractionLease<'gate>,
}
/*
#[cfg(test)]
mod tests {
*/
pub(super) fn assemble_after_commented_delimiter<'lease, 'gate>(
    lease: &'lease NativeInteractionLease<'gate>,
) -> AllowPrompt<'lease, 'gate> {
    self.stalled.store(false, Ordering::Release);
    AllowPrompt { lease }
}
#[cfg(test)]
mod tests {
}
"#;

        let production = production_source(mutant);
        assert!(production.contains("assemble_after_commented_delimiter"));
        assert!(token_has_forbidden_convenience(&production, "AllowPrompt"));
        assert!(uses_poison_reset(&production));
    }

    #[test]
    fn production_inventory_scans_every_compiled_suffix_after_the_root_test_module() {
        let append = |suffix: &str| format!("{NATIVE_INTERACTION_SOURCE}\n{suffix}\n");
        assert!(root_test_module_layout_is_closed(NATIVE_INTERACTION_SOURCE));

        let factory = production_source(&append(
            r#"
#[allow(dead_code)]
pub(super) fn fabricate_after_tests(
    gate: &NativeInteractionGate,
) -> ForbidPrompt<'_> {
    ForbidPrompt {
        _seal: ordinary_prompt::mint(gate),
    }
}
"#,
        ));
        assert!(factory.contains("fabricate_after_tests"));
        assert!(token_has_forbidden_convenience(&factory, "ForbidPrompt"));
        assert!(!capability_construction_is_lexically_sealed(
            &factory,
            "ForbidPrompt",
            "NativeInteractionGate",
            "acquire",
        ));

        let unsafe_suffix =
            production_source(&append("pub(super) fn escape_after_tests() { unsafe {} }"));
        assert!(source_has_forbidden_lease_escape_primitive(&unsafe_suffix));

        let alias_suffix = production_source(&append(
            "type LeaseAfterTests<'gate> = NativeInteractionLease<'gate>;",
        ));
        assert!(aliases_capability(
            &mask_rust_non_code(&alias_suffix),
            "NativeInteractionLease"
        ));
        assert!(!native_gate_and_lease_inventory_is_closed(&alias_suffix));

        let impl_suffix = production_source(&append(
            "impl NativeInteractionGate { fn after_tests(&self) {} }",
        ));
        assert!(!native_gate_and_lease_inventory_is_closed(&impl_suffix));

        let reset_suffix = production_source(&append(
            "fn reset_after_tests(gate: &NativeInteractionGate) { gate.stalled.store(false, Ordering::Release); }",
        ));
        assert!(uses_poison_reset(&reset_suffix));

        let macro_suffix = production_source(&append(
            "macro_rules! add_escape { ($owner:ident) => { impl<'gate> $owner<'gate> { fn escape(&self) {} } } } add_escape!(NativeInteractionLease);",
        ));
        assert!(source_has_item_position_macro(&macro_suffix));
        assert!(!native_gate_and_lease_inventory_is_closed(&macro_suffix));

        let second_root_tests = append("#[cfg(test)] mod tests {}");
        assert!(!root_test_module_layout_is_closed(&second_root_tests));

        let doubled_test_attribute = NATIVE_INTERACTION_SOURCE.replacen(
            "#[cfg(test)]\nmod tests {",
            "#[cfg(test)]\n#[cfg(test)]\nmod tests {",
            1,
        );
        assert_ne!(doubled_test_attribute, NATIVE_INTERACTION_SOURCE);
        let doubled_with_suffix = format!(
            "{doubled_test_attribute}\n\
             impl NativeInteractionGate {{ fn compiled_suffix(&self) {{}} }}\n"
        );
        let doubled_production = production_source(&doubled_with_suffix);
        assert!(!native_gate_and_lease_inventory_is_closed(
            &doubled_production
        ));
    }

    #[test]
    fn reviewed_native_interaction_source_digest_is_current() {
        use sha2::{Digest, Sha256};

        let actual: [u8; 32] = Sha256::digest(NATIVE_INTERACTION_BYTES).into();
        assert_eq!(
            actual,
            super::super::NATIVE_INTERACTION_REVIEWED_SHA256,
            "native_interaction.rs changed; actual SHA-256 is {} and requires explicit second-file review",
            hex::encode(actual),
        );
    }

    #[test]
    fn production_inventory_rejects_cfg_disabled_protected_struct_decoys() {
        let native = production_source(NATIVE_INTERACTION_SOURCE);
        let live_option = native
            .replacen(
                "_guard: MutexGuard<'gate, ()>",
                "_guard: Option<MutexGuard<'gate, ()>>",
                1,
            )
            .replacen("_guard: guard,", "_guard: Some(guard),", 1)
            .replacen(
                "        (&self.forbid_prompt, &mut self.mutation_invocation)",
                "        drop(self._guard.take());\n        (&self.forbid_prompt, &mut self.mutation_invocation)",
                1,
            );
        assert_ne!(live_option, native);

        let decoy = r#"
#[cfg(any())]
pub(super) struct NativeInteractionLease<'gate> {
    _guard: MutexGuard<'gate, ()>,
    forbid_prompt: ForbidPrompt<'gate>,
    mutation_invocation: MutationInvocation<'gate>,
}

"#;
        let mutant = live_option.replacen(
            "pub(super) struct NativeInteractionLease<'gate>",
            &format!("{decoy}pub(super) struct NativeInteractionLease<'gate>"),
            1,
        );
        assert_ne!(mutant, live_option);
        assert!(!native_gate_and_lease_inventory_is_closed(&mutant));
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

        assert!(root_test_module_layout_is_closed(NATIVE_INTERACTION_SOURCE));
        assert!(native.starts_with("#![forbid(unsafe_code)]"));
        assert!(!native.contains("fn sealed_prompt_policy_source_inventory_is_closed"));
        assert!(native_gate_and_lease_inventory_is_closed(&native));
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
        assert!(capability_construction_is_lexically_sealed(
            &native,
            "AllowPrompt",
            "NativeRecoveryFacade",
            "diagnose_or_unlock",
        ));

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
        assert!(capability_construction_is_lexically_sealed(
            &native,
            "ForbidPrompt",
            "NativeInteractionGate",
            "acquire",
        ));
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
