//! Stage-aware, backend-private durable file replacement.
//!
//! Platform adapters own paths, native handles, native failures, and byte
//! comparisons. This module owns only the closed transition contract.

use std::fmt;

pub(crate) const MAX_ENVELOPE_BYTES: usize = 256 * 1024;
const MAX_CREATE_ATTEMPTS: u8 = 8;
const MAX_REPLACE_ATTEMPTS: u8 = 4;

#[cfg(target_os = "windows")]
// Preserve the adapter-facing module path while the dark native implementation
// remains nested under the detector that owns its qualified handles and paths.
#[allow(unused_imports)]
pub(crate) use crate::credentials::filesystem_policy::windows::file_replace as windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplaceStage {
    ValidateParent,
    CreateTemp,
    VerifyTempSecurity,
    Write,
    Flush,
    CaptureCandidateIdentity,
    InvokeReplace,
    Readback,
    VerifyFinalMetadata,
    Cleanup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReplaceFailureCode {
    InvalidEnvelope,
    UnsupportedTarget,
    PermissionHardeningFailure,
    OperationInProgress,
    CommitUnknown,
    RecoveryRequired,
    InternalBackendFailure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitState {
    DefinitelyNotCommitted,
    Unknown,
    Committed,
}

pub(crate) const fn missing_candidate_path_is_clean(state: CommitState) -> bool {
    matches!(state, CommitState::Committed)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReplaceFailure {
    pub(crate) stage: ReplaceStage,
    pub(crate) code: ReplaceFailureCode,
    pub(crate) commit_state: CommitState,
    pub(crate) cleanup_pending: bool,
}

impl fmt::Debug for ReplaceFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplaceFailure")
            .field("stage", &self.stage)
            .field("code", &self.code)
            .field("commit_state", &self.commit_state)
            .field("cleanup_pending", &self.cleanup_pending)
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReplaceReceipt {
    pub(crate) attempts: u8,
    pub(crate) cleanup_pending: bool,
}

impl fmt::Debug for ReplaceReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplaceReceipt")
            .field("commit_state", &CommitState::Committed)
            .field("attempts", &self.attempts)
            .field("cleanup_pending", &self.cleanup_pending)
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct EnvelopeIdentity {
    schema: u32,
    operation_id: [u8; 16],
    revision: [u8; 16],
}

impl EnvelopeIdentity {
    pub(crate) const fn new(schema: u32, operation_id: [u8; 16], revision: [u8; 16]) -> Self {
        Self {
            schema,
            operation_id,
            revision,
        }
    }
}

impl fmt::Debug for EnvelopeIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EnvelopeIdentity([REDACTED])")
    }
}

pub(crate) type ParseEnvelopeIdentity = fn(&[u8]) -> Option<EnvelopeIdentity>;

pub(crate) struct ReplaceEnvelope<'a> {
    prior: Option<&'a [u8]>,
    prior_identity: Option<EnvelopeIdentity>,
    candidate: &'a [u8],
    candidate_identity: EnvelopeIdentity,
    parse_identity: ParseEnvelopeIdentity,
}

impl<'a> ReplaceEnvelope<'a> {
    pub(crate) fn new(
        prior: Option<&'a [u8]>,
        candidate: &'a [u8],
        schema: u32,
        operation_id: [u8; 16],
        revision: [u8; 16],
        parse_identity: ParseEnvelopeIdentity,
    ) -> Result<Self, ReplaceFailure> {
        if candidate.is_empty()
            || candidate.len() > MAX_ENVELOPE_BYTES
            || prior.is_some_and(|prior| prior.len() > MAX_ENVELOPE_BYTES)
        {
            return Err(invalid_envelope_failure());
        }
        let candidate_identity = EnvelopeIdentity::new(schema, operation_id, revision);
        if parse_identity(candidate) != Some(candidate_identity) {
            return Err(invalid_envelope_failure());
        }
        let prior_identity = match prior {
            Some(prior) => Some(parse_identity(prior).ok_or_else(invalid_envelope_failure)?),
            None => None,
        };
        Ok(Self {
            prior,
            prior_identity,
            candidate,
            candidate_identity,
            parse_identity,
        })
    }

    pub(crate) fn candidate_is_exact(&self, bytes: &[u8]) -> bool {
        bytes == self.candidate && (self.parse_identity)(bytes) == Some(self.candidate_identity)
    }

    pub(crate) fn prior_is_exact(&self, bytes: Option<&[u8]>) -> bool {
        match (self.prior, self.prior_identity, bytes) {
            (None, None, None) => true,
            (Some(prior), Some(identity), Some(bytes)) => {
                bytes == prior && (self.parse_identity)(bytes) == Some(identity)
            }
            _ => false,
        }
    }

    pub(crate) const fn has_prior(&self) -> bool {
        self.prior.is_some()
    }
}

fn invalid_envelope_failure() -> ReplaceFailure {
    ReplaceFailure {
        stage: ReplaceStage::Write,
        code: ReplaceFailureCode::InvalidEnvelope,
        commit_state: CommitState::DefinitelyNotCommitted,
        cleanup_pending: false,
    }
}

impl fmt::Debug for ReplaceEnvelope<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReplaceEnvelope([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateTempFault {
    Collision,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlatformFault {
    UnsupportedTarget,
    Permission,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeReplaceReturn {
    True,
    False,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadbackState {
    ExactCandidate,
    ExactPriorWithCompleteCandidate,
    MixedOrUnknown,
}

pub(crate) trait ReplacePlatform {
    type Candidate;
    type CandidateIdentity;

    fn validate_parent(&mut self) -> Result<(), PlatformFault>;
    fn create_temp(&mut self, attempt: u8) -> Result<Self::Candidate, CreateTempFault>;
    fn verify_temp_security(&mut self, candidate: &Self::Candidate) -> Result<(), PlatformFault>;
    fn write_complete(
        &mut self,
        candidate: &Self::Candidate,
        bytes: &[u8],
    ) -> Result<(), PlatformFault>;
    fn flush(&mut self, candidate: &Self::Candidate) -> Result<(), PlatformFault>;
    fn capture_candidate_identity(
        &mut self,
        candidate: &Self::Candidate,
    ) -> Result<Self::CandidateIdentity, PlatformFault>;
    fn invoke_replace(
        &mut self,
        candidate: &Self::Candidate,
        identity: &Self::CandidateIdentity,
        state: CommitState,
    ) -> NativeReplaceReturn;
    fn readback(
        &mut self,
        candidate: &Self::Candidate,
        identity: &Self::CandidateIdentity,
        envelope: &ReplaceEnvelope<'_>,
    ) -> Result<ReadbackState, PlatformFault>;
    fn verify_final_metadata(
        &mut self,
        identity: &Self::CandidateIdentity,
    ) -> Result<(), PlatformFault>;
    fn cleanup(
        &mut self,
        candidate: &Self::Candidate,
        state: CommitState,
    ) -> Result<(), PlatformFault>;
}

pub(crate) fn replace_with<P: ReplacePlatform>(
    platform: &mut P,
    envelope: &ReplaceEnvelope<'_>,
) -> Result<ReplaceReceipt, ReplaceFailure> {
    platform.validate_parent().map_err(|fault| ReplaceFailure {
        stage: ReplaceStage::ValidateParent,
        code: match fault {
            PlatformFault::UnsupportedTarget => ReplaceFailureCode::UnsupportedTarget,
            PlatformFault::Permission => ReplaceFailureCode::PermissionHardeningFailure,
            PlatformFault::Failed => ReplaceFailureCode::InternalBackendFailure,
        },
        commit_state: CommitState::DefinitelyNotCommitted,
        cleanup_pending: false,
    })?;

    let mut candidate = None;
    for attempt in 1..=MAX_CREATE_ATTEMPTS {
        match platform.create_temp(attempt) {
            Ok(created) => {
                candidate = Some(created);
                break;
            }
            Err(CreateTempFault::Collision) if attempt < MAX_CREATE_ATTEMPTS => {}
            Err(CreateTempFault::Collision) => {
                return Err(ReplaceFailure {
                    stage: ReplaceStage::CreateTemp,
                    code: ReplaceFailureCode::OperationInProgress,
                    commit_state: CommitState::DefinitelyNotCommitted,
                    cleanup_pending: false,
                });
            }
            Err(CreateTempFault::Failed) => {
                return Err(ReplaceFailure {
                    stage: ReplaceStage::CreateTemp,
                    code: ReplaceFailureCode::InternalBackendFailure,
                    commit_state: CommitState::DefinitelyNotCommitted,
                    cleanup_pending: false,
                });
            }
        }
    }
    let candidate = candidate.expect("bounded creation loop either returns or creates");

    platform
        .verify_temp_security(&candidate)
        .map_err(|_| ReplaceFailure {
            stage: ReplaceStage::VerifyTempSecurity,
            code: ReplaceFailureCode::PermissionHardeningFailure,
            commit_state: CommitState::DefinitelyNotCommitted,
            cleanup_pending: platform
                .cleanup(&candidate, CommitState::DefinitelyNotCommitted)
                .is_err(),
        })?;
    platform
        .write_complete(&candidate, envelope.candidate)
        .map_err(|_| ReplaceFailure {
            stage: ReplaceStage::Write,
            code: ReplaceFailureCode::InternalBackendFailure,
            commit_state: CommitState::DefinitelyNotCommitted,
            cleanup_pending: platform
                .cleanup(&candidate, CommitState::DefinitelyNotCommitted)
                .is_err(),
        })?;
    platform.flush(&candidate).map_err(|_| ReplaceFailure {
        stage: ReplaceStage::Flush,
        code: ReplaceFailureCode::InternalBackendFailure,
        commit_state: CommitState::DefinitelyNotCommitted,
        cleanup_pending: platform
            .cleanup(&candidate, CommitState::DefinitelyNotCommitted)
            .is_err(),
    })?;
    let identity = platform
        .capture_candidate_identity(&candidate)
        .map_err(|fault| ReplaceFailure {
            stage: ReplaceStage::CaptureCandidateIdentity,
            code: match fault {
                PlatformFault::UnsupportedTarget => ReplaceFailureCode::UnsupportedTarget,
                PlatformFault::Permission => ReplaceFailureCode::PermissionHardeningFailure,
                PlatformFault::Failed => ReplaceFailureCode::InternalBackendFailure,
            },
            commit_state: CommitState::DefinitelyNotCommitted,
            cleanup_pending: platform
                .cleanup(&candidate, CommitState::DefinitelyNotCommitted)
                .is_err(),
        })?;

    for attempt in 1..=MAX_REPLACE_ATTEMPTS {
        let _native_return = platform.invoke_replace(&candidate, &identity, CommitState::Unknown);
        match platform.readback(&candidate, &identity, envelope) {
            Ok(ReadbackState::ExactCandidate) => {
                platform
                    .verify_final_metadata(&identity)
                    .map_err(|_| ReplaceFailure {
                        stage: ReplaceStage::VerifyFinalMetadata,
                        code: ReplaceFailureCode::RecoveryRequired,
                        commit_state: CommitState::Unknown,
                        cleanup_pending: true,
                    })?;
                return Ok(ReplaceReceipt {
                    attempts: attempt,
                    cleanup_pending: platform
                        .cleanup(&candidate, CommitState::Committed)
                        .is_err(),
                });
            }
            Ok(ReadbackState::ExactPriorWithCompleteCandidate)
                if attempt < MAX_REPLACE_ATTEMPTS => {}
            Ok(ReadbackState::ExactPriorWithCompleteCandidate) => {
                return Err(ReplaceFailure {
                    stage: ReplaceStage::Readback,
                    code: ReplaceFailureCode::OperationInProgress,
                    commit_state: CommitState::DefinitelyNotCommitted,
                    cleanup_pending: platform
                        .cleanup(&candidate, CommitState::DefinitelyNotCommitted)
                        .is_err(),
                });
            }
            Ok(ReadbackState::MixedOrUnknown) => {
                return Err(ReplaceFailure {
                    stage: ReplaceStage::Readback,
                    code: ReplaceFailureCode::RecoveryRequired,
                    commit_state: CommitState::Unknown,
                    cleanup_pending: true,
                });
            }
            Err(_) => {
                return Err(ReplaceFailure {
                    stage: ReplaceStage::Readback,
                    code: ReplaceFailureCode::CommitUnknown,
                    commit_state: CommitState::Unknown,
                    cleanup_pending: true,
                });
            }
        }
    }
    unreachable!("bounded replacement loop returns from its final attempt")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    struct ScriptedPlatform {
        calls: RefCell<Vec<(ReplaceStage, CommitState)>>,
        fault_at: Option<ReplaceStage>,
        create_collisions_remaining: u8,
        fail_create_after_collisions: bool,
        native_returns: VecDeque<NativeReplaceReturn>,
        readbacks: VecDeque<ReadbackState>,
        cleanup_fails: bool,
        fault: PlatformFault,
    }

    impl ScriptedPlatform {
        fn happy() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                fault_at: None,
                create_collisions_remaining: 0,
                fail_create_after_collisions: false,
                native_returns: VecDeque::from([NativeReplaceReturn::True]),
                readbacks: VecDeque::from([ReadbackState::ExactCandidate]),
                cleanup_fails: false,
                fault: PlatformFault::Failed,
            }
        }

        fn call(&self, stage: ReplaceStage, state: CommitState) {
            self.calls.borrow_mut().push((stage, state));
        }

        fn fails(&self, stage: ReplaceStage) -> Result<(), PlatformFault> {
            if self.fault_at == Some(stage) {
                Err(self.fault)
            } else {
                Ok(())
            }
        }
    }

    impl ReplacePlatform for ScriptedPlatform {
        type Candidate = ();
        type CandidateIdentity = ();

        fn validate_parent(&mut self) -> Result<(), PlatformFault> {
            self.call(
                ReplaceStage::ValidateParent,
                CommitState::DefinitelyNotCommitted,
            );
            self.fails(ReplaceStage::ValidateParent)
        }

        fn create_temp(&mut self, _attempt: u8) -> Result<Self::Candidate, CreateTempFault> {
            self.call(
                ReplaceStage::CreateTemp,
                CommitState::DefinitelyNotCommitted,
            );
            if self.create_collisions_remaining > 0 {
                self.create_collisions_remaining -= 1;
                return Err(CreateTempFault::Collision);
            }
            if self.fault_at == Some(ReplaceStage::CreateTemp) || self.fail_create_after_collisions
            {
                return Err(CreateTempFault::Failed);
            }
            Ok(())
        }

        fn verify_temp_security(
            &mut self,
            _candidate: &Self::Candidate,
        ) -> Result<(), PlatformFault> {
            self.call(
                ReplaceStage::VerifyTempSecurity,
                CommitState::DefinitelyNotCommitted,
            );
            self.fails(ReplaceStage::VerifyTempSecurity)
        }

        fn write_complete(
            &mut self,
            _candidate: &Self::Candidate,
            _bytes: &[u8],
        ) -> Result<(), PlatformFault> {
            self.call(ReplaceStage::Write, CommitState::DefinitelyNotCommitted);
            self.fails(ReplaceStage::Write)
        }

        fn flush(&mut self, _candidate: &Self::Candidate) -> Result<(), PlatformFault> {
            self.call(ReplaceStage::Flush, CommitState::DefinitelyNotCommitted);
            self.fails(ReplaceStage::Flush)
        }

        fn capture_candidate_identity(
            &mut self,
            _candidate: &Self::Candidate,
        ) -> Result<Self::CandidateIdentity, PlatformFault> {
            self.call(
                ReplaceStage::CaptureCandidateIdentity,
                CommitState::DefinitelyNotCommitted,
            );
            self.fails(ReplaceStage::CaptureCandidateIdentity)
        }

        fn invoke_replace(
            &mut self,
            _candidate: &Self::Candidate,
            _identity: &Self::CandidateIdentity,
            state: CommitState,
        ) -> NativeReplaceReturn {
            self.call(ReplaceStage::InvokeReplace, state);
            self.native_returns
                .pop_front()
                .unwrap_or(NativeReplaceReturn::False)
        }

        fn readback(
            &mut self,
            _candidate: &Self::Candidate,
            _identity: &Self::CandidateIdentity,
            _envelope: &ReplaceEnvelope<'_>,
        ) -> Result<ReadbackState, PlatformFault> {
            self.call(ReplaceStage::Readback, CommitState::Unknown);
            self.fails(ReplaceStage::Readback)?;
            Ok(self
                .readbacks
                .pop_front()
                .unwrap_or(ReadbackState::MixedOrUnknown))
        }

        fn verify_final_metadata(
            &mut self,
            _identity: &Self::CandidateIdentity,
        ) -> Result<(), PlatformFault> {
            self.call(ReplaceStage::VerifyFinalMetadata, CommitState::Unknown);
            self.fails(ReplaceStage::VerifyFinalMetadata)
        }

        fn cleanup(
            &mut self,
            _candidate: &Self::Candidate,
            state: CommitState,
        ) -> Result<(), PlatformFault> {
            self.call(ReplaceStage::Cleanup, state);
            if self.cleanup_fails || self.fault_at == Some(ReplaceStage::Cleanup) {
                Err(PlatformFault::Failed)
            } else {
                Ok(())
            }
        }
    }

    const fn fixture_envelope(schema: u32, operation_byte: u8, revision_byte: u8) -> [u8; 36] {
        let schema = schema.to_le_bytes();
        let mut bytes = [0u8; 36];
        bytes[0] = schema[0];
        bytes[1] = schema[1];
        bytes[2] = schema[2];
        bytes[3] = schema[3];
        let mut index = 4;
        while index < 20 {
            bytes[index] = operation_byte;
            index += 1;
        }
        while index < 36 {
            bytes[index] = revision_byte;
            index += 1;
        }
        bytes
    }

    const PRIOR_ENVELOPE: [u8; 36] = fixture_envelope(1, 0x10, 0x20);
    const CANDIDATE_ENVELOPE: [u8; 36] = fixture_envelope(1, 0x11, 0x22);

    fn envelope() -> ReplaceEnvelope<'static> {
        ReplaceEnvelope::new(
            Some(&PRIOR_ENVELOPE),
            &CANDIDATE_ENVELOPE,
            1,
            [0x11; 16],
            [0x22; 16],
            parse_fixture_identity,
        )
        .expect("bounded fixture")
    }

    fn parse_fixture_identity(bytes: &[u8]) -> Option<EnvelopeIdentity> {
        let header = bytes.get(..36)?;
        Some(EnvelopeIdentity::new(
            u32::from_le_bytes(header.get(..4)?.try_into().ok()?),
            header.get(4..20)?.try_into().ok()?,
            header.get(20..36)?.try_into().ok()?,
        ))
    }

    #[test]
    fn committed_candidate_crosses_the_unknown_boundary_before_replace() {
        let mut platform = ScriptedPlatform::happy();

        let receipt = replace_with(&mut platform, &envelope()).expect("verified commit");

        assert_eq!(
            receipt,
            ReplaceReceipt {
                attempts: 1,
                cleanup_pending: false,
            }
        );
        assert_eq!(
            platform.calls.into_inner(),
            [
                (
                    ReplaceStage::ValidateParent,
                    CommitState::DefinitelyNotCommitted
                ),
                (
                    ReplaceStage::CreateTemp,
                    CommitState::DefinitelyNotCommitted
                ),
                (
                    ReplaceStage::VerifyTempSecurity,
                    CommitState::DefinitelyNotCommitted,
                ),
                (ReplaceStage::Write, CommitState::DefinitelyNotCommitted),
                (ReplaceStage::Flush, CommitState::DefinitelyNotCommitted),
                (
                    ReplaceStage::CaptureCandidateIdentity,
                    CommitState::DefinitelyNotCommitted,
                ),
                (ReplaceStage::InvokeReplace, CommitState::Unknown),
                (ReplaceStage::Readback, CommitState::Unknown),
                (ReplaceStage::VerifyFinalMetadata, CommitState::Unknown),
                (ReplaceStage::Cleanup, CommitState::Committed),
            ]
        );
    }

    #[test]
    fn every_definite_pre_replace_failure_stops_before_invoke_and_cleans_residue() {
        let cases = [
            (
                ReplaceStage::ValidateParent,
                ReplaceFailureCode::InternalBackendFailure,
            ),
            (
                ReplaceStage::CreateTemp,
                ReplaceFailureCode::InternalBackendFailure,
            ),
            (
                ReplaceStage::VerifyTempSecurity,
                ReplaceFailureCode::PermissionHardeningFailure,
            ),
            (
                ReplaceStage::Write,
                ReplaceFailureCode::InternalBackendFailure,
            ),
            (
                ReplaceStage::Flush,
                ReplaceFailureCode::InternalBackendFailure,
            ),
            (
                ReplaceStage::CaptureCandidateIdentity,
                ReplaceFailureCode::InternalBackendFailure,
            ),
        ];

        for (stage, code) in cases {
            let mut platform = ScriptedPlatform::happy();
            platform.fault_at = Some(stage);

            let failure = replace_with(&mut platform, &envelope()).expect_err("injected fault");

            assert_eq!(failure.stage, stage);
            assert_eq!(failure.code, code);
            assert_eq!(failure.commit_state, CommitState::DefinitelyNotCommitted);
            assert!(!failure.cleanup_pending);
            let calls = platform.calls.into_inner();
            assert!(
                !calls
                    .iter()
                    .any(|(called, _)| *called == ReplaceStage::InvokeReplace)
            );
            assert_eq!(
                calls
                    .iter()
                    .filter(|(called, _)| *called == ReplaceStage::Cleanup)
                    .count(),
                usize::from(
                    stage != ReplaceStage::ValidateParent && stage != ReplaceStage::CreateTemp
                ),
                "only an actually-created candidate is cleaned for {stage:?}"
            );
        }
    }

    #[test]
    fn qualified_parent_and_candidate_volume_failures_preserve_unsupported_target() {
        for stage in [
            ReplaceStage::ValidateParent,
            ReplaceStage::CaptureCandidateIdentity,
        ] {
            let mut platform = ScriptedPlatform::happy();
            platform.fault_at = Some(stage);
            platform.fault = PlatformFault::UnsupportedTarget;

            let failure = replace_with(&mut platform, &envelope()).expect_err("unsupported target");

            assert_eq!(failure.stage, stage);
            assert_eq!(failure.code, ReplaceFailureCode::UnsupportedTarget);
            assert_eq!(failure.commit_state, CommitState::DefinitelyNotCommitted);
        }
    }

    #[test]
    fn failed_pre_replace_cleanup_is_reported_without_crossing_commit_unknown() {
        let mut platform = ScriptedPlatform::happy();
        platform.fault_at = Some(ReplaceStage::Write);
        platform.cleanup_fails = true;

        let failure = replace_with(&mut platform, &envelope()).expect_err("injected write fault");

        assert_eq!(failure.stage, ReplaceStage::Write);
        assert_eq!(failure.commit_state, CommitState::DefinitelyNotCommitted);
        assert!(failure.cleanup_pending);
        assert!(
            platform
                .calls
                .into_inner()
                .contains(&(ReplaceStage::Cleanup, CommitState::DefinitelyNotCommitted,))
        );
    }

    #[test]
    fn create_new_collisions_retry_only_to_the_fixed_bound() {
        let mut eventually_created = ScriptedPlatform::happy();
        eventually_created.create_collisions_remaining = MAX_CREATE_ATTEMPTS - 1;
        assert!(replace_with(&mut eventually_created, &envelope()).is_ok());
        assert_eq!(
            eventually_created
                .calls
                .into_inner()
                .iter()
                .filter(|(stage, _)| *stage == ReplaceStage::CreateTemp)
                .count(),
            usize::from(MAX_CREATE_ATTEMPTS)
        );

        let mut exhausted = ScriptedPlatform::happy();
        exhausted.create_collisions_remaining = MAX_CREATE_ATTEMPTS;
        let failure = replace_with(&mut exhausted, &envelope()).expect_err("bounded collision");
        assert_eq!(failure.stage, ReplaceStage::CreateTemp);
        assert_eq!(failure.code, ReplaceFailureCode::OperationInProgress);
        assert_eq!(failure.commit_state, CommitState::DefinitelyNotCommitted);
        assert!(!failure.cleanup_pending);
    }

    #[test]
    fn non_collision_create_failure_on_the_last_attempt_is_not_contention() {
        let mut platform = ScriptedPlatform::happy();
        platform.create_collisions_remaining = MAX_CREATE_ATTEMPTS - 1;
        platform.fail_create_after_collisions = true;

        let failure = replace_with(&mut platform, &envelope()).expect_err("native create fault");

        assert_eq!(failure.stage, ReplaceStage::CreateTemp);
        assert_eq!(failure.code, ReplaceFailureCode::InternalBackendFailure);
        assert_eq!(failure.commit_state, CommitState::DefinitelyNotCommitted);
    }

    #[test]
    fn native_false_return_can_still_reconcile_as_the_exact_candidate() {
        let mut platform = ScriptedPlatform::happy();
        platform.native_returns = VecDeque::from([NativeReplaceReturn::False]);

        let receipt = replace_with(&mut platform, &envelope()).expect("readback is authority");

        assert_eq!(receipt.attempts, 1);
        assert!(
            platform
                .calls
                .into_inner()
                .contains(&(ReplaceStage::InvokeReplace, CommitState::Unknown,))
        );
    }

    #[test]
    fn exact_prior_and_complete_candidate_is_the_only_retryable_state() {
        let mut platform = ScriptedPlatform::happy();
        platform.native_returns =
            VecDeque::from([NativeReplaceReturn::False, NativeReplaceReturn::True]);
        platform.readbacks = VecDeque::from([
            ReadbackState::ExactPriorWithCompleteCandidate,
            ReadbackState::ExactCandidate,
        ]);

        let receipt = replace_with(&mut platform, &envelope()).expect("second attempt commits");

        assert_eq!(receipt.attempts, 2);
        assert_eq!(
            platform
                .calls
                .into_inner()
                .iter()
                .filter(|(stage, state)| {
                    *stage == ReplaceStage::InvokeReplace && *state == CommitState::Unknown
                })
                .count(),
            2
        );
    }

    #[test]
    fn exact_prior_contention_exhaustion_is_typed_and_never_downgrades() {
        let mut platform = ScriptedPlatform::happy();
        platform.native_returns = VecDeque::from([
            NativeReplaceReturn::False,
            NativeReplaceReturn::False,
            NativeReplaceReturn::False,
            NativeReplaceReturn::False,
        ]);
        platform.readbacks = VecDeque::from([
            ReadbackState::ExactPriorWithCompleteCandidate,
            ReadbackState::ExactPriorWithCompleteCandidate,
            ReadbackState::ExactPriorWithCompleteCandidate,
            ReadbackState::ExactPriorWithCompleteCandidate,
        ]);

        let failure = replace_with(&mut platform, &envelope()).expect_err("retry bound");

        assert_eq!(failure.stage, ReplaceStage::Readback);
        assert_eq!(failure.code, ReplaceFailureCode::OperationInProgress);
        assert_eq!(failure.commit_state, CommitState::DefinitelyNotCommitted);
        assert!(!failure.cleanup_pending);
    }

    #[test]
    fn mixed_or_unreadable_post_invoke_state_never_becomes_success() {
        let mut mixed = ScriptedPlatform::happy();
        mixed.readbacks = VecDeque::from([ReadbackState::MixedOrUnknown]);
        let failure = replace_with(&mut mixed, &envelope()).expect_err("mixed state");
        assert_eq!(failure.code, ReplaceFailureCode::RecoveryRequired);
        assert_eq!(failure.commit_state, CommitState::Unknown);
        assert!(failure.cleanup_pending);

        let mut unreadable = ScriptedPlatform::happy();
        unreadable.fault_at = Some(ReplaceStage::Readback);
        let failure = replace_with(&mut unreadable, &envelope()).expect_err("readback unavailable");
        assert_eq!(failure.code, ReplaceFailureCode::CommitUnknown);
        assert_eq!(failure.commit_state, CommitState::Unknown);
        assert!(failure.cleanup_pending);
    }

    #[test]
    fn final_metadata_mismatch_requires_recovery_after_exact_candidate_bytes() {
        let mut platform = ScriptedPlatform::happy();
        platform.fault_at = Some(ReplaceStage::VerifyFinalMetadata);

        let failure = replace_with(&mut platform, &envelope()).expect_err("metadata mismatch");

        assert_eq!(failure.stage, ReplaceStage::VerifyFinalMetadata);
        assert_eq!(failure.code, ReplaceFailureCode::RecoveryRequired);
        assert_eq!(failure.commit_state, CommitState::Unknown);
        assert!(failure.cleanup_pending);
    }

    #[test]
    fn cleanup_failure_preserves_verified_commit_and_reports_pending_residue() {
        let mut platform = ScriptedPlatform::happy();
        platform.cleanup_fails = true;

        let receipt = replace_with(&mut platform, &envelope()).expect("commit remains verified");

        assert_eq!(receipt.attempts, 1);
        assert!(receipt.cleanup_pending);
    }

    #[test]
    fn envelope_bounds_and_debug_output_are_content_free() {
        const SECRET_CANARY: &[u8] = b"secret-path-sid-file-volume-native-canary";
        let oversized = vec![0x55; MAX_ENVELOPE_BYTES + 1];

        for (prior, candidate) in [
            (None, &[][..]),
            (None, oversized.as_slice()),
            (Some(oversized.as_slice()), b"candidate".as_slice()),
        ] {
            let failure = ReplaceEnvelope::new(
                prior,
                candidate,
                1,
                [1; 16],
                [2; 16],
                parse_fixture_identity,
            )
            .expect_err("invalid bound");
            assert_eq!(failure.code, ReplaceFailureCode::InvalidEnvelope);
            assert_eq!(failure.commit_state, CommitState::DefinitelyNotCommitted);
        }

        let mut secret_envelope = Vec::from(fixture_envelope(u32::MAX, 0x6f, 0x70));
        secret_envelope.extend_from_slice(SECRET_CANARY);
        let request = ReplaceEnvelope::new(
            Some(&secret_envelope),
            &secret_envelope,
            u32::MAX,
            [0x6f; 16],
            [0x70; 16],
            parse_fixture_identity,
        )
        .expect("bounded redaction fixture");
        let rendered = format!("{request:?}");
        assert_eq!(rendered, "ReplaceEnvelope([REDACTED])");
        assert!(!rendered.contains(std::str::from_utf8(SECRET_CANARY).unwrap()));
        assert_eq!(
            format!("{:?}", request.candidate_identity),
            "EnvelopeIdentity([REDACTED])"
        );

        let failure = ReplaceFailure {
            stage: ReplaceStage::Readback,
            code: ReplaceFailureCode::CommitUnknown,
            commit_state: CommitState::Unknown,
            cleanup_pending: true,
        };
        let rendered = format!("{failure:?}");
        assert!(!rendered.contains("path"));
        assert!(!rendered.contains("sid"));
        assert!(!rendered.contains("native"));
    }

    #[test]
    fn mismatched_encoded_metadata_is_rejected_before_platform_work() {
        let mut candidate = Vec::from(7u32.to_le_bytes());
        candidate.extend_from_slice(&[0x11; 16]);
        candidate.extend_from_slice(&[0x33; 16]);
        candidate.extend_from_slice(b"dummy-only-payload");

        let failure = ReplaceEnvelope::new(
            None,
            &candidate,
            7,
            [0x11; 16],
            [0x22; 16],
            parse_fixture_identity,
        )
        .expect_err("encoded revision differs from the expected revision");

        assert_eq!(failure.code, ReplaceFailureCode::InvalidEnvelope);
        assert_eq!(failure.commit_state, CommitState::DefinitelyNotCommitted);
    }

    #[test]
    fn readback_predicates_reject_corrupted_schema_operation_and_revision_bytes() {
        let request = envelope();
        assert!(request.candidate_is_exact(&CANDIDATE_ENVELOPE));
        assert!(request.prior_is_exact(Some(&PRIOR_ENVELOPE)));

        for offset in [0, 4, 20] {
            let mut corrupted_candidate = CANDIDATE_ENVELOPE;
            corrupted_candidate[offset] ^= 0xff;
            assert!(!request.candidate_is_exact(&corrupted_candidate));

            let mut corrupted_prior = PRIOR_ENVELOPE;
            corrupted_prior[offset] ^= 0xff;
            assert!(!request.prior_is_exact(Some(&corrupted_prior)));
        }
        assert!(!request.prior_is_exact(None));
    }

    #[test]
    fn absent_candidate_path_is_clean_only_after_a_verified_commit() {
        assert!(missing_candidate_path_is_clean(CommitState::Committed));
        assert!(!missing_candidate_path_is_clean(
            CommitState::DefinitelyNotCommitted
        ));
        assert!(!missing_candidate_path_is_clean(CommitState::Unknown));
    }
}
