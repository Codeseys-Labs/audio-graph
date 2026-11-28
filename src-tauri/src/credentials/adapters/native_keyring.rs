use super::authority_journal::{
    AuthorityJournalBoundary, AuthorityMutationLock, FileAuthorityJournalBoundary,
    decode_authority_journal, decode_authority_marker, encode_authority_journal,
    encode_authority_marker, new_authority_instance_id, repair_root_ownership,
};
#[cfg(test)]
use super::keyring_entry::KeyringBoundary;
use super::keyring_entry::{EntryLocator, KeyringEntryAdapter};
use super::native_interaction::{
    NativeInteractionGate, NativeInteractionLease, process_native_interaction_gate,
};
use crate::credentials::domain::{
    AuthorityJournal, CredentialStoreFailure, EncodedCredentialRecord,
};
use crate::credentials::service::{
    AuthoritativeActiveRead, CredentialEntryStore, CredentialMutationSession, ObservedActivation,
    StoreOwnershipRepair, UntrackedRecordOutcome, UntrackedRecordRepair,
};
use audio_graph_ipc_contract::credential_contract::{
    BUILT_IN_CREDENTIAL_SET_IDS, CredentialBackendKind, CredentialOperationId, CredentialSetId,
};
use std::path::PathBuf;
use std::sync::Arc;

/// A store bound to one authority. Only opening a ready authority or
/// initializing a fresh one produces it, so it is never unbound.
pub(crate) struct NativeKeyringCredentialStore {
    entries: KeyringEntryAdapter,
    journal: Arc<dyn AuthorityJournalBoundary>,
    interaction_gate: &'static NativeInteractionGate,
    /// The authority this store was opened or initialized against. Every
    /// later read and mutation must find this same pair, so a journal lost or
    /// replaced under an open store stops it instead of being trusted.
    bound_authority: String,
}

/// A store opened where no authority ever existed. It serves no reads or
/// mutations, and it is the only type with `initialize`: a bound store,
/// which holds positive evidence of the authority it was bound to, can
/// never authorize a new authority over a lost one.
pub(crate) struct UninitializedNativeCredentialStore {
    entries: KeyringEntryAdapter,
    journal: Arc<dyn AuthorityJournalBoundary>,
    interaction_gate: &'static NativeInteractionGate,
}

pub(crate) enum NativeCredentialStoreOpen {
    Uninitialized(UninitializedNativeCredentialStore),
    Ready {
        store: NativeKeyringCredentialStore,
        journal: Box<AuthorityJournal>,
    },
    RecoveryRequired,
}

enum AuthorityPairState {
    Uninitialized,
    Ready {
        authority_instance_id: String,
        journal: Box<AuthorityJournal>,
    },
    RecoveryRequired,
}

struct NativeCredentialMutationSession<'a> {
    store: &'a NativeKeyringCredentialStore,
    operation_id: CredentialOperationId,
    set_id: CredentialSetId,
    journal_loaded: bool,
    interaction: NativeInteractionLease<'static>,
    expected_active_readback: Option<(CredentialSetId, EncodedCredentialRecord)>,
    expected_staging_readback: Option<(
        CredentialOperationId,
        CredentialSetId,
        EncodedCredentialRecord,
    )>,
    _mutation_lock: Box<dyn AuthorityMutationLock>,
}

impl NativeKeyringCredentialStore {
    /// Fails with `Unsupported` before journal or native I/O when this platform
    /// has no reviewed no-prompt boundary; it never selects a fallback store.
    /// The journal root and the namespace lock root must qualify, on their
    /// held handles, before the lock file exists or any native call runs.
    pub(crate) fn open(
        root: impl Into<PathBuf>,
    ) -> Result<NativeCredentialStoreOpen, CredentialStoreFailure> {
        let entries = KeyringEntryAdapter::production()?;
        Self::open_with_adapters(
            entries,
            Arc::new(FileAuthorityJournalBoundary::open_qualified(root)?),
        )
    }

    /// Explicit, user-invoked repair for an `open` that hardening refused
    /// with `PermissionHardeningFailed` because another principal owns the
    /// journal root, the namespace lock root, or a file in either. Where
    /// the platform lets this user take ownership (Windows, given
    /// `WRITE_OWNER`), it does and hardens; elsewhere the refusal stands. It
    /// performs no native credential I/O.
    pub(crate) fn repair_journal_ownership(
        root: impl Into<PathBuf>,
    ) -> Result<StoreOwnershipRepair, CredentialStoreFailure> {
        Ok(if repair_root_ownership(root)? {
            StoreOwnershipRepair::Repaired
        } else {
            StoreOwnershipRepair::Unchanged
        })
    }

    #[cfg(test)]
    fn open_with_boundaries(
        entries: Arc<dyn KeyringBoundary>,
        journal: Arc<dyn AuthorityJournalBoundary>,
    ) -> Result<NativeCredentialStoreOpen, CredentialStoreFailure> {
        Self::open_with_adapters_and_gate(
            KeyringEntryAdapter::new(entries),
            journal,
            NativeInteractionGate::isolated_for_test(),
        )
    }

    #[cfg(test)]
    fn open_with_boundaries_and_gate(
        entries: Arc<dyn KeyringBoundary>,
        journal: Arc<dyn AuthorityJournalBoundary>,
        interaction_gate: &'static NativeInteractionGate,
    ) -> Result<NativeCredentialStoreOpen, CredentialStoreFailure> {
        Self::open_with_adapters_and_gate(
            KeyringEntryAdapter::new(entries),
            journal,
            interaction_gate,
        )
    }

    fn open_with_adapters(
        entries: KeyringEntryAdapter,
        journal: Arc<dyn AuthorityJournalBoundary>,
    ) -> Result<NativeCredentialStoreOpen, CredentialStoreFailure> {
        Self::open_with_adapters_and_gate(entries, journal, process_native_interaction_gate())
    }

    fn open_with_adapters_and_gate(
        entries: KeyringEntryAdapter,
        journal: Arc<dyn AuthorityJournalBoundary>,
        interaction_gate: &'static NativeInteractionGate,
    ) -> Result<NativeCredentialStoreOpen, CredentialStoreFailure> {
        let (authority_state, fresh) = {
            let mut interaction = interaction_gate.acquire()?;
            let _mutation_lock = journal.acquire_mutation_lock()?;
            journal.remove_stray_staged()?;
            let state =
                read_authority_state_unlocked(&entries, journal.as_ref(), &mut interaction)?;
            let fresh = match state {
                AuthorityPairState::Uninitialized => {
                    no_authority_ever_existed_unlocked(&entries, &mut interaction)?
                }
                AuthorityPairState::Ready { .. } | AuthorityPairState::RecoveryRequired => false,
            };
            (state, fresh)
        };

        match authority_state {
            AuthorityPairState::Uninitialized if fresh => Ok(
                NativeCredentialStoreOpen::Uninitialized(UninitializedNativeCredentialStore {
                    entries,
                    journal,
                    interaction_gate,
                }),
            ),
            AuthorityPairState::Uninitialized => Ok(NativeCredentialStoreOpen::RecoveryRequired),
            AuthorityPairState::Ready {
                authority_instance_id,
                journal: authority_journal,
            } => Ok(NativeCredentialStoreOpen::Ready {
                store: Self {
                    entries,
                    journal,
                    interaction_gate,
                    bound_authority: authority_instance_id,
                },
                journal: authority_journal,
            }),
            AuthorityPairState::RecoveryRequired => Ok(NativeCredentialStoreOpen::RecoveryRequired),
        }
    }

    /// Re-reads the journal and marker and requires the pair this store was
    /// bound to. Run under the interaction lease and the mutation lock, so no
    /// cooperating process can change the pair between this check and the
    /// read that follows. Both halves missing is treated like one half
    /// missing: either way the bound authority is gone.
    fn require_bound_authority_unlocked(
        &self,
        interaction: &mut NativeInteractionLease<'_>,
    ) -> Result<Box<AuthorityJournal>, CredentialStoreFailure> {
        match read_authority_state_unlocked(&self.entries, self.journal.as_ref(), interaction)? {
            AuthorityPairState::Ready {
                authority_instance_id,
                journal,
            } if authority_instance_id == self.bound_authority => Ok(journal),
            AuthorityPairState::Ready { .. }
            | AuthorityPairState::Uninitialized
            | AuthorityPairState::RecoveryRequired => Err(CredentialStoreFailure::CorruptRecord),
        }
    }

    fn read_record_unlocked(
        &self,
        interaction: &mut NativeInteractionLease<'_>,
        locator: &EntryLocator,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
        self.entries
            .read(interaction, locator)
            .map(|record| record.map(EncodedCredentialRecord::from_zeroizing_boundary_bytes))
    }
}

impl UninitializedNativeCredentialStore {
    /// Publishes the first authority pair, after requiring under the lock
    /// that the pair is still absent and that no authority ever existed.
    pub(crate) fn initialize(
        self,
        journal: AuthorityJournal,
    ) -> Result<NativeCredentialStoreOpen, CredentialStoreFailure> {
        journal.validate_persisted_for_backend(CredentialBackendKind::Native)?;

        let authority_instance_id = {
            let mut interaction = self.interaction_gate.acquire()?;
            let _mutation_lock = self.journal.acquire_mutation_lock()?;
            if !matches!(
                read_authority_state_unlocked(
                    &self.entries,
                    self.journal.as_ref(),
                    &mut interaction
                )?,
                AuthorityPairState::Uninitialized
            ) {
                return Err(CredentialStoreFailure::RevisionConflict);
            }
            if !no_authority_ever_existed_unlocked(&self.entries, &mut interaction)? {
                return Err(CredentialStoreFailure::CorruptRecord);
            }

            let authority_instance_id = new_authority_instance_id();
            let marker = encode_authority_marker(&authority_instance_id)?;
            let journal_bytes = encode_authority_journal(&authority_instance_id, &journal)?;
            self.entries
                .write_authority(&mut interaction, &EntryLocator::authority(), &marker)?;
            interaction.ensure_healthy()?;
            // `replace` succeeds only after reading these exact bytes back
            // through the held root it published into. Reading the journal
            // again by path could reach another directory and turn this
            // verified publication into uncertainty, so only the marker,
            // which this call wrote to the native store, is read back.
            self.journal
                .replace(&journal_bytes)
                .map_err(|_| interaction.latch_commit_unknown())?;
            match self
                .entries
                .read(&mut interaction, &EntryLocator::authority())
            {
                Ok(Some(marker))
                    if decode_authority_marker(marker.as_slice()).as_deref()
                        == Some(authority_instance_id.as_str()) => {}
                Ok(_) | Err(_) => return Err(interaction.latch_commit_unknown()),
            }
            authority_instance_id
        };

        Ok(NativeCredentialStoreOpen::Ready {
            store: NativeKeyringCredentialStore {
                entries: self.entries,
                journal: self.journal,
                interaction_gate: self.interaction_gate,
                bound_authority: authority_instance_id,
            },
            journal: Box::new(journal),
        })
    }
}

fn read_authority_state_unlocked(
    entries: &KeyringEntryAdapter,
    journal: &dyn AuthorityJournalBoundary,
    interaction: &mut NativeInteractionLease<'_>,
) -> Result<AuthorityPairState, CredentialStoreFailure> {
    interaction.ensure_healthy()?;
    let journal_bytes = journal.read()?;
    let marker_bytes = match entries.read(interaction, &EntryLocator::authority()) {
        Ok(marker) => marker,
        Err(CredentialStoreFailure::CorruptRecord)
        | Err(CredentialStoreFailure::UnsupportedSchema) => {
            return Ok(AuthorityPairState::RecoveryRequired);
        }
        Err(failure) => return Err(failure),
    };
    match (journal_bytes, marker_bytes) {
        (None, None) => Ok(AuthorityPairState::Uninitialized),
        (Some(journal_bytes), Some(marker_bytes)) => {
            let Some(journal) = decode_authority_journal(&journal_bytes) else {
                return Ok(AuthorityPairState::RecoveryRequired);
            };
            let Some(marker_id) = decode_authority_marker(marker_bytes.as_slice()) else {
                return Ok(AuthorityPairState::RecoveryRequired);
            };
            if journal.authority_instance_id == marker_id
                && journal
                    .journal
                    .validate_persisted_for_backend(CredentialBackendKind::Native)
                    .is_ok()
            {
                Ok(AuthorityPairState::Ready {
                    authority_instance_id: marker_id,
                    journal: Box::new(journal.journal),
                })
            } else {
                Ok(AuthorityPairState::RecoveryRequired)
            }
        }
        _ => Ok(AuthorityPairState::RecoveryRequired),
    }
}

/// The independent evidence that authorizes first initialization.
///
/// Run only once both halves of the authority pair are absent; then only
/// native state can tell a fresh install from a lost authority. The store
/// counts as fresh only if the active record and the staging guard of every
/// built-in set are all absent. Any bytes at any of those locators block
/// first initialization, including bytes another authority wrote, and an
/// entry that cannot be decoded counts as present. Every other read failure
/// is returned as it is, so a fresh machine whose keychain cannot be read is
/// refused without anything being written.
///
/// A staging record is named by its operation, which only the lost journal
/// knew, so it cannot be looked up; its set's guard, written before the
/// staging record and removed only after it, stands in for it. The limit:
/// a staging record written before guards existed, or one that outlived its
/// guard, is invisible here. Custom sets never reach this store's
/// mutations, and the pre-v2 (v1) namespace is not evidence of a v2
/// authority.
fn no_authority_ever_existed_unlocked(
    entries: &KeyringEntryAdapter,
    interaction: &mut NativeInteractionLease<'_>,
) -> Result<bool, CredentialStoreFailure> {
    for set_id in BUILT_IN_CREDENTIAL_SET_IDS {
        let set_id = CredentialSetId::BuiltIn(*set_id);
        for locator in [
            EntryLocator::active(&set_id),
            EntryLocator::staging_guard(&set_id),
        ] {
            match entries.read(interaction, &locator) {
                Ok(None) => {}
                Ok(Some(_))
                | Err(CredentialStoreFailure::CorruptRecord)
                | Err(CredentialStoreFailure::UnsupportedSchema) => return Ok(false),
                Err(failure) => return Err(failure),
            }
        }
    }
    Ok(true)
}

/// A staging guard names the operation whose staging record it guards, so
/// only that operation's cleanup removes it. It carries no secret.
const STAGING_GUARD_PREFIX: &str = "audiograph.credential-v2.staging-guard/";

fn staging_guard_bytes(operation_id: &CredentialOperationId) -> Vec<u8> {
    format!("{STAGING_GUARD_PREFIX}{}", operation_id.as_str()).into_bytes()
}

/// The operation a guard names, or `None` for bytes no guard was written
/// with.
fn guarded_operation(bytes: &[u8]) -> Option<CredentialOperationId> {
    let named = std::str::from_utf8(bytes)
        .ok()?
        .strip_prefix(STAGING_GUARD_PREFIX)?;
    CredentialOperationId::parse(named).ok()
}

/// The bytes every guard held before guards named their operation (written
/// only by development builds of #553). Such a guard cannot say which
/// staging record it stands for.
const LEGACY_STAGING_GUARD: &[u8] = b"audiograph.credential-v2.staging-guard";

/// What a set's staging guard says, as far as repair is concerned.
enum StagingGuard {
    Absent,
    Names(CredentialOperationId),
    Legacy,
    Unrecognized,
}

impl StagingGuard {
    fn classify(bytes: Option<&[u8]>) -> Self {
        match bytes {
            None => Self::Absent,
            Some(LEGACY_STAGING_GUARD) => Self::Legacy,
            Some(bytes) => guarded_operation(bytes).map_or(Self::Unrecognized, Self::Names),
        }
    }
}

impl NativeKeyringCredentialStore {
    /// Removes `set_id`'s native staging records and staging guard that
    /// `journal` does not track. Run under the interaction lease and the
    /// mutation lock, on the journal read under them. Every cooperating
    /// process records an activation in the journal before it stages and
    /// before it releases that lock, so the only staging a live operation
    /// owns is that of the journal's pending activation for the set; that
    /// staging record, and a guard naming it, are never touched. Nor is the
    /// staging of any activation in `observed`, those the repairing service
    /// saw pending and has not seen settled, while that record exists: a
    /// journal restored at the same epoch can omit an activation that is
    /// still live.
    ///
    /// Candidates are the operation the guard names and every operation the
    /// journal's history records for the set, which also finds staging a
    /// guard in the legacy format stood for when its operation completed.
    /// Staging records go before the guard, so a staging record never
    /// outlives its guard. A legacy guard does not say which staging it
    /// stands for, so it stays while the set has a spared activation. An
    /// unrecognized guard leaves the set untouched.
    fn repair_set_unlocked(
        &self,
        interaction: &mut NativeInteractionLease<'_>,
        journal: &AuthorityJournal,
        observed: &[ObservedActivation],
        set_id: &CredentialSetId,
    ) -> Result<UntrackedRecordOutcome, CredentialStoreFailure> {
        let tracked = journal
            .pending_activation
            .as_ref()
            .filter(|pending| pending.set_id == *set_id)
            .map(|pending| &pending.operation_id);
        let mut live_observed = Vec::new();
        for observed in observed {
            if observed.set_id == *set_id
                && tracked != Some(&observed.operation_id)
                && self
                    .entries
                    .read(
                        interaction,
                        &EntryLocator::staging(&observed.operation_id, set_id),
                    )?
                    .is_some()
            {
                live_observed.push(&observed.operation_id);
            }
        }
        let spared = |operation_id: &CredentialOperationId| {
            tracked == Some(operation_id) || live_observed.contains(&operation_id)
        };
        let any_spared = tracked.is_some() || !live_observed.is_empty();
        let guard_locator = EntryLocator::staging_guard(set_id);
        let guard_bytes = self.entries.read(interaction, &guard_locator)?;
        let guard = StagingGuard::classify(guard_bytes.as_deref().map(Vec::as_slice));
        let mut candidates: Vec<CredentialOperationId> = journal
            .idempotency_history
            .iter()
            .filter(|entry| entry.set_id == *set_id)
            .map(|entry| entry.receipt.operation_id.clone())
            .collect();
        let guard_is_spared = match &guard {
            StagingGuard::Unrecognized => return Ok(UntrackedRecordOutcome::Unrecognized),
            StagingGuard::Names(operation_id) => {
                candidates.push(operation_id.clone());
                spared(operation_id)
            }
            StagingGuard::Legacy => any_spared,
            StagingGuard::Absent => false,
        };
        candidates.retain(|operation_id| !spared(operation_id));
        candidates.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        candidates.dedup();

        let mut removed = false;
        for operation_id in &candidates {
            let staging = EntryLocator::staging(operation_id, set_id);
            if self.entries.read(interaction, &staging)?.is_some() {
                self.entries
                    .delete_and_verify_absent(interaction, &staging)?;
                removed = true;
            }
        }
        if !matches!(guard, StagingGuard::Absent) && !guard_is_spared {
            self.entries
                .delete_and_verify_absent(interaction, &guard_locator)?;
            removed = true;
        }
        Ok(
            if matches!(guard, StagingGuard::Legacy) && !guard_is_spared {
                UntrackedRecordOutcome::LegacyGuardRemoved
            } else if removed {
                UntrackedRecordOutcome::Removed
            } else if any_spared {
                UntrackedRecordOutcome::Tracked
            } else {
                UntrackedRecordOutcome::Clean
            },
        )
    }
}

impl CredentialEntryStore for NativeKeyringCredentialStore {
    fn read_active(
        &self,
        set_id: &CredentialSetId,
        may_read: &dyn Fn(&AuthorityJournal) -> bool,
    ) -> Result<AuthoritativeActiveRead, CredentialStoreFailure> {
        let mut interaction = self.interaction_gate.acquire()?;
        let _mutation_lock = self.journal.acquire_mutation_lock()?;
        let journal = *self.require_bound_authority_unlocked(&mut interaction)?;
        let record = if may_read(&journal) {
            self.read_record_unlocked(&mut interaction, &EntryLocator::active(set_id))?
        } else {
            None
        };
        Ok(AuthoritativeActiveRead { journal, record })
    }

    fn begin_mutation(
        &self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<Box<dyn CredentialMutationSession + '_>, CredentialStoreFailure> {
        let interaction = self.interaction_gate.acquire()?;
        let mutation_lock = self.journal.acquire_mutation_lock()?;
        Ok(Box::new(NativeCredentialMutationSession {
            store: self,
            operation_id: operation_id.clone(),
            set_id: set_id.clone(),
            journal_loaded: false,
            interaction,
            expected_active_readback: None,
            expected_staging_readback: None,
            _mutation_lock: mutation_lock,
        }))
    }

    /// Decided on the bound authority's journal read under the lock; a
    /// journal `admit` refuses, or one that is not the bound authority's,
    /// touches no record.
    fn repair_untracked_records(
        &self,
        observed: &[ObservedActivation],
        admit: &dyn Fn(&AuthorityJournal) -> bool,
    ) -> Result<Option<Vec<UntrackedRecordRepair>>, CredentialStoreFailure> {
        let mut interaction = self.interaction_gate.acquire()?;
        let _mutation_lock = self.journal.acquire_mutation_lock()?;
        let journal = *self.require_bound_authority_unlocked(&mut interaction)?;
        if !admit(&journal) {
            return Ok(None);
        }
        let mut sets = Vec::with_capacity(BUILT_IN_CREDENTIAL_SET_IDS.len());
        for built_in in BUILT_IN_CREDENTIAL_SET_IDS {
            let set_id = CredentialSetId::BuiltIn(*built_in);
            let outcome =
                self.repair_set_unlocked(&mut interaction, &journal, observed, &set_id)?;
            sets.push(UntrackedRecordRepair { set_id, outcome });
        }
        Ok(Some(sets))
    }

    fn authority_instance(&self) -> &str {
        &self.bound_authority
    }
}

impl NativeCredentialMutationSession<'_> {
    fn require_set_scope(&self, set_id: &CredentialSetId) -> Result<(), CredentialStoreFailure> {
        if self.set_id == *set_id {
            Ok(())
        } else {
            Err(CredentialStoreFailure::Internal)
        }
    }

    fn require_staging_scope(
        &self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<(), CredentialStoreFailure> {
        if self.operation_id == *operation_id && self.set_id == *set_id {
            Ok(())
        } else {
            Err(CredentialStoreFailure::Internal)
        }
    }

    fn require_no_pending_readback(&self) -> Result<(), CredentialStoreFailure> {
        if self.expected_active_readback.is_none() && self.expected_staging_readback.is_none() {
            Ok(())
        } else {
            Err(CredentialStoreFailure::OperationInProgress)
        }
    }

    /// Every entry operation needs the authority binding `load_journal`
    /// validated under this session's lock; without it the session would
    /// act for an authority it never checked.
    fn require_loaded_journal(&self) -> Result<(), CredentialStoreFailure> {
        if self.journal_loaded {
            Ok(())
        } else {
            Err(CredentialStoreFailure::Internal)
        }
    }

    /// Publishes `journal` for the bound authority. The boundary's `Ok`
    /// already means it read these exact bytes back through the held root it
    /// published into; reading the journal again by path could reach another
    /// directory and latch uncertainty over a verified publication.
    ///
    /// Another program holding the journal open refuses the publication
    /// with `OperationInProgress` before any effect. Before this session
    /// changed anything native, that is retryable as it stands; after a
    /// native change (`after_native_change`) it is the store-level
    /// `Unavailable`, since retrying the request no longer means repeating
    /// nothing.
    fn persist_journal(
        &mut self,
        journal: &AuthorityJournal,
        after_native_change: bool,
    ) -> Result<(), CredentialStoreFailure> {
        self.interaction.ensure_healthy()?;
        self.require_loaded_journal()?;
        self.require_no_pending_readback()?;
        journal.validate_persisted_for_backend(CredentialBackendKind::Native)?;

        let bytes = encode_authority_journal(&self.store.bound_authority, journal)?;
        match self.store.journal.replace(&bytes) {
            Ok(()) => Ok(()),
            Err(CredentialStoreFailure::CommitUnknown) => {
                Err(self.interaction.latch_commit_unknown())
            }
            Err(CredentialStoreFailure::OperationInProgress) if after_native_change => {
                Err(CredentialStoreFailure::Unavailable)
            }
            Err(failure) => Err(failure),
        }
    }

    /// Requires that `set_id`'s staging guard is absent, already names
    /// `operation_id`, or names an operation whose staging record is gone,
    /// and returns whether it already names `operation_id`. A guard whose
    /// staging record remains stops another operation from staging, so at
    /// most one staging record exists per set.
    fn claim_staging_guard(
        &mut self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<bool, CredentialStoreFailure> {
        let guard = EntryLocator::staging_guard(set_id);
        let Some(bytes) = self.store.entries.read(&mut self.interaction, &guard)? else {
            return Ok(false);
        };
        let owner =
            guarded_operation(bytes.as_slice()).ok_or(CredentialStoreFailure::CorruptRecord)?;
        if owner == *operation_id {
            return Ok(true);
        }
        match self.store.entries.read(
            &mut self.interaction,
            &EntryLocator::staging(&owner, set_id),
        )? {
            None => Ok(false),
            Some(_) => Err(CredentialStoreFailure::CorruptRecord),
        }
    }
}

impl CredentialMutationSession for NativeCredentialMutationSession<'_> {
    fn load_journal(&mut self) -> Result<AuthorityJournal, CredentialStoreFailure> {
        self.interaction.ensure_healthy()?;
        self.require_no_pending_readback()?;
        let journal = self
            .store
            .require_bound_authority_unlocked(&mut self.interaction)?;
        self.journal_loaded = true;
        Ok(*journal)
    }

    fn read_active(
        &mut self,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
        self.require_loaded_journal()?;
        self.require_set_scope(set_id)?;
        self.require_no_pending_readback()?;
        self.store
            .read_record_unlocked(&mut self.interaction, &EntryLocator::active(set_id))
    }

    /// An intent is persisted before any native change of its operation.
    fn persist_intent(&mut self, journal: &AuthorityJournal) -> Result<(), CredentialStoreFailure> {
        self.persist_journal(journal, false)
    }

    fn replace_active(
        &mut self,
        set_id: &CredentialSetId,
        record: EncodedCredentialRecord,
    ) -> Result<(), CredentialStoreFailure> {
        self.require_loaded_journal()?;
        self.require_set_scope(set_id)?;
        self.require_no_pending_readback()?;
        self.store.entries.write(
            &mut self.interaction,
            &EntryLocator::active(set_id),
            record.as_bytes(),
        )?;
        self.expected_active_readback = Some((set_id.clone(), record));
        Ok(())
    }

    fn readback_active(
        &mut self,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
        self.require_loaded_journal()?;
        self.require_set_scope(set_id)?;
        if self.expected_staging_readback.is_some() {
            return Err(CredentialStoreFailure::OperationInProgress);
        }
        let readback = self
            .store
            .read_record_unlocked(&mut self.interaction, &EntryLocator::active(set_id));
        let Some((expected_set_id, expected)) = self.expected_active_readback.take() else {
            return readback;
        };
        if expected_set_id != *set_id {
            self.expected_active_readback = Some((expected_set_id, expected));
            return readback;
        }
        match readback {
            Ok(Some(record)) if record.as_bytes() == expected.as_bytes() => Ok(Some(record)),
            Ok(None) | Ok(Some(_)) | Err(_) => Err(self.interaction.latch_commit_unknown()),
        }
    }

    fn write_staging(
        &mut self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
        record: EncodedCredentialRecord,
    ) -> Result<(), CredentialStoreFailure> {
        self.require_loaded_journal()?;
        self.require_staging_scope(operation_id, set_id)?;
        self.require_no_pending_readback()?;
        // The guard goes first and is read back before any staging byte
        // exists, so a staging record never exists without a guard naming
        // its operation.
        if !self.claim_staging_guard(operation_id, set_id)? {
            let guard = EntryLocator::staging_guard(set_id);
            let bytes = staging_guard_bytes(operation_id);
            self.store
                .entries
                .write(&mut self.interaction, &guard, &bytes)?;
            match self.store.entries.read(&mut self.interaction, &guard) {
                Ok(Some(readback)) if readback.as_slice() == bytes.as_slice() => {}
                Ok(_) | Err(_) => return Err(self.interaction.latch_commit_unknown()),
            }
        }
        self.store.entries.write(
            &mut self.interaction,
            &EntryLocator::staging(operation_id, set_id),
            record.as_bytes(),
        )?;
        self.expected_staging_readback = Some((operation_id.clone(), set_id.clone(), record));
        Ok(())
    }

    fn read_staging(
        &mut self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
        self.require_loaded_journal()?;
        self.require_staging_scope(operation_id, set_id)?;
        if self.expected_active_readback.is_some() {
            return Err(CredentialStoreFailure::OperationInProgress);
        }
        let readback = self.store.read_record_unlocked(
            &mut self.interaction,
            &EntryLocator::staging(operation_id, set_id),
        );
        let Some((expected_operation_id, expected_set_id, expected)) =
            self.expected_staging_readback.take()
        else {
            return readback;
        };
        if expected_operation_id != *operation_id || expected_set_id != *set_id {
            self.expected_staging_readback =
                Some((expected_operation_id, expected_set_id, expected));
            return readback;
        }
        match readback {
            Ok(Some(record)) if record.as_bytes() == expected.as_bytes() => Ok(Some(record)),
            Ok(None) | Ok(Some(_)) | Err(_) => Err(self.interaction.latch_commit_unknown()),
        }
    }

    fn delete_staging(
        &mut self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<(), CredentialStoreFailure> {
        self.require_loaded_journal()?;
        self.require_staging_scope(operation_id, set_id)?;
        self.require_no_pending_readback()?;
        self.store.entries.delete_and_verify_absent(
            &mut self.interaction,
            &EntryLocator::staging(operation_id, set_id),
        )?;
        // Only once the staging record is gone does its guard go, and only
        // a guard naming this operation: another operation's guard still
        // stands for that operation's staging record.
        let guard = EntryLocator::staging_guard(set_id);
        match self.store.entries.read(&mut self.interaction, &guard)? {
            Some(bytes) if guarded_operation(bytes.as_slice()).as_ref() == Some(operation_id) => {
                self.store
                    .entries
                    .delete_and_verify_absent(&mut self.interaction, &guard)
            }
            Some(_) | None => Ok(()),
        }
    }

    fn commit_journal(&mut self, journal: &AuthorityJournal) -> Result<(), CredentialStoreFailure> {
        self.persist_journal(journal, true)
    }
}

impl Drop for NativeCredentialMutationSession<'_> {
    fn drop(&mut self) {
        if self.expected_active_readback.is_some() || self.expected_staging_readback.is_some() {
            let _ = self.interaction.latch_commit_unknown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NativeCredentialStoreOpen, NativeKeyringCredentialStore, staging_guard_bytes};
    use crate::credentials::adapters::authority_journal::{
        AuthorityJournalBoundary, AuthorityMutationLock, FileAuthorityJournalBoundary,
        encode_authority_journal, encode_authority_marker,
    };
    use crate::credentials::adapters::keyring_entry::{
        EntryLocator, KeyringBoundary, KeyringEntryAdapter,
    };
    use crate::credentials::adapters::native_interaction::{
        ForbidPrompt, MutationInvocation, NativeInteractionGate,
    };
    use crate::credentials::domain::{
        AuthorityJournal, CredentialMutationKind, CredentialRecordEnvelope, CredentialStoreFailure,
        EncodedCredentialRecord, PendingCredentialIntent, PendingSettingsActivation,
        StoredSecretBundle,
    };
    use crate::credentials::fake::FakeSettingsActivationPort;
    use crate::credentials::service::{
        AuthoritativeActiveRead, CredentialEntryStore, CredentialService, CredentialUseRequest,
        DeleteCredentialSet, PrepareCredentialActivation, ReplaceCredentialSet,
        UntrackedRecordOutcome, UntrackedRecordRepair,
    };
    use crate::credentials::test_support::DeterministicTokenSource;
    use audio_graph_ipc_contract::credential_contract::{
        AuthMethodId, BUILT_IN_CREDENTIAL_SET_IDS, BuiltInCredentialSetId,
        CredentialActivationStage, CredentialAudience, CredentialBackendKind, CredentialErrorCode,
        CredentialIdempotencyToken, CredentialMutationResultCode, CredentialOperationId,
        CredentialPurpose, CredentialRevision, CredentialSafeRecoveryAction, CredentialSetId,
        CredentialSetRecoveryState, PORTABLE_ENCODED_RECORD_MAX_BYTES, SecureTransportScheme,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    #[derive(Clone, Copy)]
    enum AuthorityWriteCut {
        BeforeInvocation,
        AfterPersist,
    }

    #[derive(Clone, Copy)]
    enum JournalReplaceCut {
        BeforePublish,
        AfterPublish,
        /// Another program holds the journal open without sharing delete.
        Contended,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum PolicyTraceTarget {
        Authority,
        Staging,
        StagingGuard(BuiltInCredentialSetId),
        Active(BuiltInCredentialSetId),
        OtherActive,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct PolicyTraceStep {
        operation: &'static str,
        prompt_id: usize,
        target: PolicyTraceTarget,
    }

    #[derive(Default)]
    struct MemoryKeyringBoundary {
        entries: Mutex<Vec<(String, Vec<u8>)>>,
        reads: AtomicUsize,
        writes: AtomicUsize,
        deletes: AtomicUsize,
        authority_reads: AtomicUsize,
        fail_authority_read_at: AtomicUsize,
        authority_write_cut: Mutex<Option<AuthorityWriteCut>>,
        fail_active_reads: AtomicBool,
        corrupt_active_readback: AtomicBool,
        corrupt_staging_readback: AtomicBool,
        policy_trace: Mutex<Vec<PolicyTraceStep>>,
    }

    impl MemoryKeyringBoundary {
        fn trace_policy(&self, prompt: &ForbidPrompt<'_>, operation: &str, locator: &EntryLocator) {
            let built_in = |account: &str| {
                BUILT_IN_CREDENTIAL_SET_IDS
                    .iter()
                    .copied()
                    .find(|candidate| candidate.as_str() == account)
            };
            let kind = if locator.account() == "v2/_authority" {
                "authority"
            } else if locator.account().starts_with("v2-staging/") {
                "staging"
            } else if locator.account().starts_with("v2-staging-guard/") {
                "guard"
            } else {
                "active"
            };
            let step = match (operation, kind) {
                ("get", "authority") => "forbid:get:authority",
                ("get", "staging") => "forbid:get:staging",
                ("get", "guard") => "forbid:get:guard",
                ("get", "active") => "forbid:get:active",
                ("set", "authority") => "forbid:set:authority",
                ("set", "staging") => "forbid:set:staging",
                ("set", "guard") => "forbid:set:guard",
                ("set", "active") => "forbid:set:active",
                ("delete", "staging") => "forbid:delete:staging",
                ("delete", "guard") => "forbid:delete:guard",
                ("delete", "authority") => "forbid:delete:authority",
                ("delete", "active") => "forbid:delete:active",
                _ => unreachable!("closed policy trace operation"),
            };
            let target = if locator.account() == "v2/_authority" {
                PolicyTraceTarget::Authority
            } else if locator.account().starts_with("v2-staging/") {
                PolicyTraceTarget::Staging
            } else if let Some(account) = locator.account().strip_prefix("v2-staging-guard/") {
                built_in(account).map_or(PolicyTraceTarget::OtherActive, |set_id| {
                    PolicyTraceTarget::StagingGuard(set_id)
                })
            } else {
                locator
                    .account()
                    .strip_prefix("v2/")
                    .and_then(built_in)
                    .map_or(PolicyTraceTarget::OtherActive, PolicyTraceTarget::Active)
            };
            self.policy_trace
                .lock()
                .expect("memory keyring policy trace lock")
                .push(PolicyTraceStep {
                    operation: step,
                    prompt_id: prompt as *const ForbidPrompt<'_> as usize,
                    target,
                });
        }
    }

    impl KeyringBoundary for MemoryKeyringBoundary {
        fn get_secret(
            &self,
            prompt: &ForbidPrompt<'_>,
            locator: &EntryLocator,
        ) -> Result<zeroize::Zeroizing<Vec<u8>>, CredentialStoreFailure> {
            self.trace_policy(prompt, "get", locator);
            self.reads.fetch_add(1, Ordering::SeqCst);
            if locator.account() == "v2/_authority" {
                let authority_read = self.authority_reads.fetch_add(1, Ordering::SeqCst) + 1;
                if self.fail_authority_read_at.load(Ordering::SeqCst) == authority_read {
                    return Err(CredentialStoreFailure::Unavailable);
                }
            } else if locator.account().starts_with("v2/")
                && self.fail_active_reads.load(Ordering::SeqCst)
            {
                return Err(CredentialStoreFailure::Unavailable);
            }
            let mut value = self
                .entries
                .lock()
                .expect("memory keyring lock")
                .iter()
                .find(|(account, _)| account == locator.account())
                .map(|(_, value)| value.clone())
                .ok_or(CredentialStoreFailure::Missing)?;
            if (locator.account().starts_with("v2/")
                && locator.account() != "v2/_authority"
                && self.corrupt_active_readback.load(Ordering::SeqCst))
                || (locator.account().starts_with("v2-staging/")
                    && self.corrupt_staging_readback.load(Ordering::SeqCst))
            {
                value.push(0xff);
            }
            Ok(zeroize::Zeroizing::new(value))
        }

        fn set_secret(
            &self,
            prompt: &ForbidPrompt<'_>,
            invocation: &mut MutationInvocation<'_>,
            locator: &EntryLocator,
            secret: &[u8],
        ) -> Result<(), CredentialStoreFailure> {
            self.trace_policy(prompt, "set", locator);
            self.writes.fetch_add(1, Ordering::SeqCst);
            let authority_cut = if locator.account() == "v2/_authority" {
                self.authority_write_cut
                    .lock()
                    .expect("authority write cut lock")
                    .take()
            } else {
                None
            };
            if matches!(authority_cut, Some(AuthorityWriteCut::BeforeInvocation)) {
                return Err(CredentialStoreFailure::Unavailable);
            }
            invocation.mark_started();
            let mut entries = self.entries.lock().expect("memory keyring lock");
            entries.retain(|(account, _)| account != locator.account());
            entries.push((locator.account().to_owned(), secret.to_vec()));
            if matches!(authority_cut, Some(AuthorityWriteCut::AfterPersist)) {
                return Err(CredentialStoreFailure::Unavailable);
            }
            Ok(())
        }

        fn delete_credential(
            &self,
            prompt: &ForbidPrompt<'_>,
            invocation: &mut MutationInvocation<'_>,
            locator: &EntryLocator,
        ) -> Result<(), CredentialStoreFailure> {
            self.trace_policy(prompt, "delete", locator);
            self.deletes.fetch_add(1, Ordering::SeqCst);
            invocation.mark_started();
            let mut entries = self.entries.lock().expect("memory keyring lock");
            let before = entries.len();
            entries.retain(|(account, _)| account != locator.account());
            if entries.len() == before {
                Err(CredentialStoreFailure::Missing)
            } else {
                Ok(())
            }
        }
    }

    struct CorruptMarkerBoundary;

    impl KeyringBoundary for CorruptMarkerBoundary {
        fn get_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            locator: &EntryLocator,
        ) -> Result<zeroize::Zeroizing<Vec<u8>>, CredentialStoreFailure> {
            if locator.account() == "v2/_authority" {
                Err(CredentialStoreFailure::CorruptRecord)
            } else {
                Err(CredentialStoreFailure::Missing)
            }
        }

        fn set_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
            _secret: &[u8],
        ) -> Result<(), CredentialStoreFailure> {
            unreachable!("read-only corrupt marker boundary")
        }

        fn delete_credential(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
        ) -> Result<(), CredentialStoreFailure> {
            unreachable!("read-only corrupt marker boundary")
        }
    }

    #[derive(Default)]
    struct MemoryJournalBoundary {
        bytes: Mutex<Option<Vec<u8>>>,
        reads: AtomicUsize,
        writes: AtomicUsize,
        corrupt_replacements: AtomicBool,
        replace_cut: Mutex<Option<JournalReplaceCut>>,
        lock_contended: AtomicBool,
    }

    struct MemoryMutationLock;
    impl AuthorityMutationLock for MemoryMutationLock {}

    impl AuthorityJournalBoundary for MemoryJournalBoundary {
        fn read(&self) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(self.bytes.lock().expect("memory journal lock").clone())
        }

        /// Honors the boundary contract: a publication whose bytes do not
        /// read back is `CommitUnknown`, never `Ok`.
        fn replace(&self, bytes: &[u8]) -> Result<(), CredentialStoreFailure> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            let cut = self
                .replace_cut
                .lock()
                .expect("journal replace cut lock")
                .take();
            match cut {
                Some(JournalReplaceCut::BeforePublish) => {
                    return Err(CredentialStoreFailure::Unavailable);
                }
                Some(JournalReplaceCut::Contended) => {
                    return Err(CredentialStoreFailure::OperationInProgress);
                }
                Some(JournalReplaceCut::AfterPublish) | None => {}
            }
            let corrupt = self.corrupt_replacements.load(Ordering::SeqCst);
            *self.bytes.lock().expect("memory journal lock") = Some(if corrupt {
                b"{corrupt-authority-journal".to_vec()
            } else {
                bytes.to_vec()
            });
            if corrupt || matches!(cut, Some(JournalReplaceCut::AfterPublish)) {
                return Err(CredentialStoreFailure::CommitUnknown);
            }
            Ok(())
        }

        fn remove_stray_staged(&self) -> Result<(), CredentialStoreFailure> {
            Ok(())
        }

        fn acquire_mutation_lock(
            &self,
        ) -> Result<Box<dyn AuthorityMutationLock>, CredentialStoreFailure> {
            if self.lock_contended.load(Ordering::SeqCst) {
                return Err(CredentialStoreFailure::OperationInProgress);
            }
            Ok(Box::new(MemoryMutationLock))
        }
    }

    fn ready_store_with_gate(
        keyring: Arc<MemoryKeyringBoundary>,
        journal: Arc<MemoryJournalBoundary>,
        gate: &'static NativeInteractionGate,
    ) -> NativeKeyringCredentialStore {
        let opened =
            NativeKeyringCredentialStore::open_with_boundaries_and_gate(keyring, journal, gate)
                .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        match store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority")
        {
            NativeCredentialStoreOpen::Ready { store, .. } => store,
            _ => panic!("expected ready authority"),
        }
    }

    #[test]
    fn absent_journal_and_marker_open_uninitialized_without_authority_state_writes() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());

        let opened =
            NativeKeyringCredentialStore::open_with_boundaries(keyring.clone(), journal.clone())
                .expect("open absent authority");

        assert!(matches!(
            opened,
            NativeCredentialStoreOpen::Uninitialized(_)
        ));
        // The authority marker, then the active record and staging guard of
        // every built-in set: the evidence that no authority ever existed.
        assert_eq!(
            keyring.reads.load(Ordering::SeqCst),
            1 + 2 * BUILT_IN_CREDENTIAL_SET_IDS.len()
        );
        assert_eq!(keyring.writes.load(Ordering::SeqCst), 0);
        assert_eq!(journal.reads.load(Ordering::SeqCst), 1);
        assert_eq!(journal.writes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn poisoned_process_entry_gate_fails_closed_before_any_keyring_call() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let gate = NativeInteractionGate::isolated_for_test();
        let poisoner = std::thread::spawn(move || {
            gate.poison_for_test();
        });
        assert!(poisoner.join().is_err());

        let failure = match NativeKeyringCredentialStore::open_with_boundaries_and_gate(
            keyring.clone(),
            journal.clone(),
            gate,
        ) {
            Err(failure) => failure,
            Ok(_) => panic!("poisoned entry gate must fail closed"),
        };
        assert_eq!(failure, CredentialStoreFailure::StalledWorker);
        let public = failure.into_public(None);
        assert_eq!(public.code, CredentialErrorCode::StalledWorker);
        assert_eq!(
            public.recovery_action,
            CredentialSafeRecoveryAction::RestartApplication
        );
        assert!(!public.retryable);
        assert_eq!(keyring.reads.load(Ordering::SeqCst), 0);
        assert_eq!(keyring.writes.load(Ordering::SeqCst), 0);
        assert_eq!(journal.reads.load(Ordering::SeqCst), 0);
        assert_eq!(journal.writes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn invalid_initial_journal_is_rejected_before_any_authority_write() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };

        assert!(matches!(
            store.initialize(AuthorityJournal::new(CredentialBackendKind::InMemory)),
            Err(CredentialStoreFailure::CorruptRecord)
        ));
        assert_eq!(keyring.writes.load(Ordering::SeqCst), 0);
        assert_eq!(journal_boundary.writes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn orphan_built_in_entries_make_an_absent_authority_recovery_required() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring
            .entries
            .lock()
            .expect("memory keyring lock")
            .push(("v2/openai".to_owned(), b"orphan-secret-canary".to_vec()));
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());

        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(
                keyring.clone(),
                journal_boundary.clone(),
            ),
            Ok(NativeCredentialStoreOpen::RecoveryRequired)
        ));
        assert_eq!(keyring.writes.load(Ordering::SeqCst), 0);
        assert_eq!(journal_boundary.writes.load(Ordering::SeqCst), 0);
        assert!(
            keyring
                .entries
                .lock()
                .expect("memory keyring lock")
                .iter()
                .all(|(account, _)| account != "v2/_authority")
        );
    }

    #[test]
    fn orphan_preflight_read_failures_stay_typed_without_writing() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.fail_active_reads.store(true, Ordering::SeqCst);
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());

        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(
                keyring.clone(),
                journal_boundary.clone(),
            ),
            Err(CredentialStoreFailure::Unavailable)
        ));
        assert_eq!(keyring.writes.load(Ordering::SeqCst), 0);
        assert_eq!(journal_boundary.writes.load(Ordering::SeqCst), 0);
        assert!(
            keyring
                .entries
                .lock()
                .expect("memory keyring lock")
                .iter()
                .all(|(account, _)| account != "v2/_authority")
        );
    }

    #[test]
    fn initialization_cut_points_preserve_definite_prewrite_errors_and_normalize_uncertainty() {
        #[derive(Clone, Copy)]
        enum ReopenState {
            Uninitialized,
            Ready,
            RecoveryRequired,
        }

        fn run_case(
            keyring: Arc<MemoryKeyringBoundary>,
            journal_boundary: Arc<MemoryJournalBoundary>,
            expected_error: CredentialStoreFailure,
            expected_reopen: ReopenState,
        ) {
            let opened = NativeKeyringCredentialStore::open_with_boundaries(
                keyring.clone(),
                journal_boundary.clone(),
            )
            .expect("open absent authority");
            let store = match opened {
                NativeCredentialStoreOpen::Uninitialized(store) => store,
                _ => panic!("expected uninitialized authority"),
            };
            let error = match store.initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            {
                Ok(_) => panic!("scripted initialization cut must fail"),
                Err(error) => error,
            };
            assert_eq!(error, expected_error);

            let reopened =
                NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                    .expect("reopen after scripted cut");
            assert!(match expected_reopen {
                ReopenState::Uninitialized => {
                    matches!(reopened, NativeCredentialStoreOpen::Uninitialized(_))
                }
                ReopenState::Ready => {
                    matches!(reopened, NativeCredentialStoreOpen::Ready { .. })
                }
                ReopenState::RecoveryRequired => {
                    matches!(reopened, NativeCredentialStoreOpen::RecoveryRequired)
                }
            });
        }

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        *keyring
            .authority_write_cut
            .lock()
            .expect("authority write cut lock") = Some(AuthorityWriteCut::BeforeInvocation);
        run_case(
            keyring,
            Arc::new(MemoryJournalBoundary::default()),
            CredentialStoreFailure::Unavailable,
            ReopenState::Uninitialized,
        );

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        *keyring
            .authority_write_cut
            .lock()
            .expect("authority write cut lock") = Some(AuthorityWriteCut::AfterPersist);
        run_case(
            keyring,
            Arc::new(MemoryJournalBoundary::default()),
            CredentialStoreFailure::CommitUnknown,
            ReopenState::RecoveryRequired,
        );

        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        *journal_boundary
            .replace_cut
            .lock()
            .expect("journal replace cut lock") = Some(JournalReplaceCut::BeforePublish);
        run_case(
            Arc::new(MemoryKeyringBoundary::default()),
            journal_boundary,
            CredentialStoreFailure::CommitUnknown,
            ReopenState::RecoveryRequired,
        );

        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        *journal_boundary
            .replace_cut
            .lock()
            .expect("journal replace cut lock") = Some(JournalReplaceCut::AfterPublish);
        run_case(
            Arc::new(MemoryKeyringBoundary::default()),
            journal_boundary,
            CredentialStoreFailure::CommitUnknown,
            ReopenState::Ready,
        );

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.fail_authority_read_at.store(3, Ordering::SeqCst);
        run_case(
            keyring,
            Arc::new(MemoryJournalBoundary::default()),
            CredentialStoreFailure::CommitUnknown,
            ReopenState::Ready,
        );
    }

    /// Another program holding the journal open refuses a publication before
    /// any effect. For an intent, persisted before any native change, that
    /// is the retryable `OperationInProgress`; for the commit after a native
    /// change it is the store-level `Unavailable`. Neither latches the store.
    #[test]
    fn a_contended_journal_is_retryable_only_before_a_native_change() {
        let journal = Arc::new(MemoryJournalBoundary::default());
        let store = ready_store_with_gate(
            Arc::new(MemoryKeyringBoundary::default()),
            journal.clone(),
            NativeInteractionGate::isolated_for_test(),
        );
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let operation_id = CredentialOperationId::parse("7b7b7b7b-7b7b-4b7b-8b7b-7b7b7b7b7b7b")
            .expect("canonical operation id");
        let contend = || {
            *journal
                .replace_cut
                .lock()
                .expect("journal replace cut lock") = Some(JournalReplaceCut::Contended);
        };
        let mut session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin mutation");
        let mut next = session.load_journal().expect("load journal");
        next.global_epoch += 1;

        contend();
        assert_eq!(
            session.persist_intent(&next),
            Err(CredentialStoreFailure::OperationInProgress)
        );
        session
            .persist_intent(&next)
            .expect("the intent persists once the reader lets go");
        session
            .replace_active(&set_id, remnant_record(&set_id, &operation_id))
            .expect("replace the active record");
        session
            .readback_active(&set_id)
            .expect("read the active record back");
        contend();
        assert_eq!(
            session.commit_journal(&next),
            Err(CredentialStoreFailure::Unavailable)
        );
        session
            .commit_journal(&next)
            .expect("nothing was latched: the commit goes through once the reader lets go");
    }

    #[test]
    fn explicit_initialization_pairs_marker_and_secret_free_journal_then_reopens_ready() {
        const SECRET_CANARY: &[u8] = b"must-not-enter-authority-metadata";
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.entries.lock().expect("memory keyring lock").push((
            "v2-staging/11111111-2222-4333-8444-555555555555/openai".to_owned(),
            SECRET_CANARY.to_vec(),
        ));
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let journal = AuthorityJournal::new(CredentialBackendKind::Native);

        let initialized = store
            .initialize(journal.clone())
            .expect("initialize native authority");
        match initialized {
            NativeCredentialStoreOpen::Ready {
                journal: initialized,
                ..
            } => assert_eq!(*initialized, journal),
            _ => panic!("expected ready authority"),
        }

        let marker_bytes = keyring
            .entries
            .lock()
            .expect("memory keyring lock")
            .iter()
            .find(|(account, _)| account == "v2/_authority")
            .expect("authority marker")
            .1
            .clone();
        let journal_bytes = journal_boundary
            .bytes
            .lock()
            .expect("memory journal lock")
            .clone()
            .expect("authority journal");
        assert!(
            !marker_bytes
                .windows(SECRET_CANARY.len())
                .any(|bytes| bytes == SECRET_CANARY)
        );
        assert!(
            !journal_bytes
                .windows(SECRET_CANARY.len())
                .any(|bytes| bytes == SECRET_CANARY)
        );

        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                .expect("reopen paired authority"),
            NativeCredentialStoreOpen::Ready { .. }
        ));
    }

    #[test]
    fn matching_authority_ids_do_not_override_unsupported_inner_journal() {
        fn pending_activation_journal(auth_method_id: AuthMethodId) -> AuthorityJournal {
            let mut journal = AuthorityJournal::new(CredentialBackendKind::Native);
            let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
            let operation_id = CredentialOperationId::parse("abababab-cdcd-4efe-8010-121212121212")
                .expect("operation id");
            let idempotency_token =
                CredentialIdempotencyToken::parse("bcbcbcbc-dede-4afa-8121-232323232323")
                    .expect("idempotency token");
            let proposed_revision =
                CredentialRevision::parse("cdcdcdcd-efef-4b0b-8232-343434343434")
                    .expect("revision");
            journal.pending_intents.push(PendingCredentialIntent {
                operation_id: operation_id.clone(),
                idempotency_token: idempotency_token.clone(),
                set_id: set_id.clone(),
                mutation_kind: CredentialMutationKind::Activate,
                expected_revision: None,
                proposed_revision: proposed_revision.clone(),
                recovery_state: CredentialSetRecoveryState::PendingIntent,
            });
            journal.pending_activation = Some(PendingSettingsActivation {
                operation_id,
                idempotency_token,
                set_id: set_id.clone(),
                auth_method_id,
                expected_revision: None,
                proposed_revision,
                expected_settings_revision: 52,
                proposed_settings_revision: 53,
                expected_global_epoch: 0,
                proposed_global_epoch: 1,
                stage: CredentialActivationStage::Staged,
            });
            let set = journal.set_state_mut(&set_id).expect("built-in set state");
            set.pending_activation = true;
            set.recovery_state = CredentialSetRecoveryState::PendingIntent;
            journal
        }

        fn duplicate_pending_token_journal() -> AuthorityJournal {
            let mut journal = AuthorityJournal::new(CredentialBackendKind::Native);
            let shared_token =
                CredentialIdempotencyToken::parse("dededede-fafa-4c1c-8343-454545454545")
                    .expect("idempotency token");
            for (set_id, operation_id, proposed_revision) in [
                (
                    BuiltInCredentialSetId::Openai,
                    "efefefef-0b0b-4d2d-8454-565656565656",
                    "f0f0f0f0-1c1c-4e3e-8565-676767676767",
                ),
                (
                    BuiltInCredentialSetId::Deepgram,
                    "01010101-2d2d-4f4f-8676-787878787878",
                    "12121212-3e3e-4050-8787-898989898989",
                ),
            ] {
                let set_id = CredentialSetId::from(set_id);
                journal.pending_intents.push(PendingCredentialIntent {
                    operation_id: CredentialOperationId::parse(operation_id).expect("operation id"),
                    idempotency_token: shared_token.clone(),
                    set_id: set_id.clone(),
                    mutation_kind: CredentialMutationKind::Replace,
                    expected_revision: None,
                    proposed_revision: CredentialRevision::parse(proposed_revision)
                        .expect("revision"),
                    recovery_state: CredentialSetRecoveryState::PendingIntent,
                });
                journal
                    .set_state_mut(&set_id)
                    .expect("built-in set state")
                    .recovery_state = CredentialSetRecoveryState::PendingIntent;
            }
            journal
        }

        fn assert_matching_invalid_journal_requires_recovery(journal: &AuthorityJournal) {
            let authority_instance_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
            let keyring = Arc::new(MemoryKeyringBoundary::default());
            keyring.entries.lock().expect("memory keyring lock").push((
                "v2/_authority".to_owned(),
                encode_authority_marker(authority_instance_id).expect("marker envelope"),
            ));
            let journal_boundary = Arc::new(MemoryJournalBoundary {
                bytes: Mutex::new(Some(
                    encode_authority_journal(authority_instance_id, journal)
                        .expect("journal envelope"),
                )),
                ..MemoryJournalBoundary::default()
            });

            assert!(matches!(
                NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                    .expect("open exact but invalid authority pair"),
                NativeCredentialStoreOpen::RecoveryRequired
            ));
        }

        let authority_instance_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.entries.lock().expect("memory keyring lock").push((
            "v2/_authority".to_owned(),
            encode_authority_marker(authority_instance_id).expect("marker envelope"),
        ));
        let mut unsupported = AuthorityJournal::new(CredentialBackendKind::Native);
        unsupported.schema_version = u32::MAX;
        let journal_boundary = Arc::new(MemoryJournalBoundary {
            bytes: Mutex::new(Some(
                encode_authority_journal(authority_instance_id, &unsupported)
                    .expect("journal envelope"),
            )),
            ..MemoryJournalBoundary::default()
        });

        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                .expect("open invalid inner journal"),
            NativeCredentialStoreOpen::RecoveryRequired
        ));

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.entries.lock().expect("memory keyring lock").push((
            "v2/_authority".to_owned(),
            encode_authority_marker(authority_instance_id).expect("marker envelope"),
        ));
        let mut duplicated_set = AuthorityJournal::new(CredentialBackendKind::Native);
        duplicated_set.sets.push(duplicated_set.sets[0].clone());
        let journal_boundary = Arc::new(MemoryJournalBoundary {
            bytes: Mutex::new(Some(
                encode_authority_journal(authority_instance_id, &duplicated_set)
                    .expect("journal envelope"),
            )),
            ..MemoryJournalBoundary::default()
        });

        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                .expect("open duplicate-set journal"),
            NativeCredentialStoreOpen::RecoveryRequired
        ));

        let wrong_auth_method = pending_activation_journal(AuthMethodId::AwsStatic);
        assert_matching_invalid_journal_requires_recovery(&wrong_auth_method);

        let mut drifted_epoch = pending_activation_journal(AuthMethodId::ApiKey);
        drifted_epoch.global_epoch = 1;
        assert_matching_invalid_journal_requires_recovery(&drifted_epoch);

        let duplicate_pending_token = duplicate_pending_token_journal();
        assert_matching_invalid_journal_requires_recovery(&duplicate_pending_token);
    }

    #[test]
    fn matching_authority_pending_activation_excludes_unrelated_pending_mutation() {
        let authority_instance_id = "89898989-9a9a-4b1b-8c2c-d3d3d3d3d3d3";
        let activation_set = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let activation_operation =
            CredentialOperationId::parse("8a8a8a8a-9b9b-4c2c-8d3d-e4e4e4e4e4e4")
                .expect("operation id");
        let activation_token =
            CredentialIdempotencyToken::parse("8b8b8b8b-9c9c-4d3d-8e4e-f5f5f5f5f5f5")
                .expect("idempotency token");
        let activation_revision =
            CredentialRevision::parse("8c8c8c8c-9d9d-4e4e-8f5f-a6a6a6a6a6a6").expect("revision");
        let mut journal = AuthorityJournal::new(CredentialBackendKind::Native);
        journal.pending_intents.push(PendingCredentialIntent {
            operation_id: activation_operation.clone(),
            idempotency_token: activation_token.clone(),
            set_id: activation_set.clone(),
            mutation_kind: CredentialMutationKind::Activate,
            expected_revision: None,
            proposed_revision: activation_revision.clone(),
            recovery_state: CredentialSetRecoveryState::PendingIntent,
        });
        journal.pending_activation = Some(PendingSettingsActivation {
            operation_id: activation_operation,
            idempotency_token: activation_token,
            set_id: activation_set.clone(),
            auth_method_id: AuthMethodId::ApiKey,
            expected_revision: None,
            proposed_revision: activation_revision,
            expected_settings_revision: 61,
            proposed_settings_revision: 62,
            expected_global_epoch: 0,
            proposed_global_epoch: 1,
            stage: CredentialActivationStage::Staged,
        });
        let activation_state = journal
            .set_state_mut(&activation_set)
            .expect("built-in activation set state");
        activation_state.pending_activation = true;
        activation_state.recovery_state = CredentialSetRecoveryState::PendingIntent;

        let unrelated_set = CredentialSetId::from(BuiltInCredentialSetId::Openai);
        journal.pending_intents.push(PendingCredentialIntent {
            operation_id: CredentialOperationId::parse("8d8d8d8d-9e9e-4f5f-8060-b7b7b7b7b7b7")
                .expect("operation id"),
            idempotency_token: CredentialIdempotencyToken::parse(
                "8e8e8e8e-9f9f-4060-8171-c8c8c8c8c8c8",
            )
            .expect("idempotency token"),
            set_id: unrelated_set.clone(),
            mutation_kind: CredentialMutationKind::Replace,
            expected_revision: None,
            proposed_revision: CredentialRevision::parse("8f8f8f8f-a0a0-4171-8282-d9d9d9d9d9d9")
                .expect("revision"),
            recovery_state: CredentialSetRecoveryState::PendingIntent,
        });
        journal
            .set_state_mut(&unrelated_set)
            .expect("built-in unrelated set state")
            .recovery_state = CredentialSetRecoveryState::PendingIntent;

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.entries.lock().expect("memory keyring lock").push((
            "v2/_authority".to_owned(),
            encode_authority_marker(authority_instance_id).expect("marker envelope"),
        ));
        let journal_boundary = Arc::new(MemoryJournalBoundary {
            bytes: Mutex::new(Some(
                encode_authority_journal(authority_instance_id, &journal)
                    .expect("journal envelope"),
            )),
            ..MemoryJournalBoundary::default()
        });

        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                .expect("open exact matching authority pair"),
            NativeCredentialStoreOpen::RecoveryRequired
        ));
    }

    #[test]
    fn matching_terminal_epoch_journal_reopens_ready() {
        let authority_instance_id = "23232323-4545-4676-8789-010101010101";
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.entries.lock().expect("memory keyring lock").push((
            "v2/_authority".to_owned(),
            encode_authority_marker(authority_instance_id).expect("marker envelope"),
        ));
        let mut terminal = AuthorityJournal::new(CredentialBackendKind::Native);
        terminal.global_epoch = u64::MAX;
        let journal_boundary = Arc::new(MemoryJournalBoundary {
            bytes: Mutex::new(Some(
                encode_authority_journal(authority_instance_id, &terminal)
                    .expect("terminal journal envelope"),
            )),
            ..MemoryJournalBoundary::default()
        });

        match NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
            .expect("terminal journal remains readable")
        {
            NativeCredentialStoreOpen::Ready { journal, .. } => {
                assert_eq!(journal.global_epoch, u64::MAX);
                assert!(journal.pending_activation.is_none());
                assert!(journal.pending_intents.is_empty());
            }
            _ => panic!("matching terminal authority should reopen ready"),
        }
    }

    #[test]
    fn one_sided_malformed_mismatched_and_wrong_backend_authority_require_recovery() {
        fn assert_recovery(marker: Option<Vec<u8>>, journal: Option<Vec<u8>>) {
            let keyring = Arc::new(MemoryKeyringBoundary::default());
            if let Some(marker) = marker {
                keyring
                    .entries
                    .lock()
                    .expect("memory keyring lock")
                    .push(("v2/_authority".to_owned(), marker));
            }
            let journal_boundary = Arc::new(MemoryJournalBoundary {
                bytes: Mutex::new(journal),
                ..MemoryJournalBoundary::default()
            });
            assert!(matches!(
                NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                    .expect("invalid authority opens as a closed state"),
                NativeCredentialStoreOpen::RecoveryRequired
            ));
        }

        let first_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let second_id = "11111111-2222-4333-8444-555555555555";
        let native_journal = AuthorityJournal::new(CredentialBackendKind::Native);
        let first_marker = encode_authority_marker(first_id).expect("first marker");
        let second_marker = encode_authority_marker(second_id).expect("second marker");
        let first_journal =
            encode_authority_journal(first_id, &native_journal).expect("first journal");

        assert_recovery(Some(first_marker.clone()), None);
        assert_recovery(None, Some(first_journal.clone()));
        assert_recovery(
            Some(b"{malformed-marker".to_vec()),
            Some(first_journal.clone()),
        );
        assert_recovery(
            Some(first_marker.clone()),
            Some(b"{malformed-journal".to_vec()),
        );
        assert_recovery(Some(second_marker), Some(first_journal));

        let wrong_backend = AuthorityJournal::new(CredentialBackendKind::InMemory);
        assert_recovery(
            Some(first_marker.clone()),
            Some(
                encode_authority_journal(first_id, &wrong_backend).expect("wrong-backend journal"),
            ),
        );

        let mut unsupported_marker: serde_json::Value =
            serde_json::from_slice(&first_marker).expect("marker JSON");
        unsupported_marker["schema_version"] = serde_json::json!(u32::MAX);
        assert_recovery(
            Some(serde_json::to_vec(&unsupported_marker).expect("unsupported marker")),
            Some(encode_authority_journal(first_id, &native_journal).expect("supported journal")),
        );
    }

    #[test]
    fn byte_malformed_authority_marker_is_recovery_required_not_parser_error() {
        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(
                Arc::new(CorruptMarkerBoundary),
                Arc::new(MemoryJournalBoundary::default()),
            )
            .expect("malformed marker is a closed composition state"),
            NativeCredentialStoreOpen::RecoveryRequired
        ));
    }

    #[test]
    fn ready_adapter_executes_one_service_replace_and_commits_secret_free_journal() {
        const SECRET_CANARY: &str = "native-entry-only-secret-canary";
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let (store, journal) = match initialized {
            NativeCredentialStoreOpen::Ready { store, journal } => (store, *journal),
            _ => panic!("expected ready authority"),
        };
        let service = CredentialService::new(
            Arc::new(store),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::from(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key(SECRET_CANARY).expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                )
                .expect("canonical idempotency token"),
            })
            .expect("replace credential set");

        assert!(
            keyring
                .entries
                .lock()
                .expect("memory keyring lock")
                .iter()
                .any(|(account, _)| account == "v2/deepgram")
        );
        let persisted_journal = journal_boundary
            .bytes
            .lock()
            .expect("memory journal lock")
            .clone()
            .expect("persisted journal");
        assert!(
            !persisted_journal
                .windows(SECRET_CANARY.len())
                .any(|bytes| bytes == SECRET_CANARY.as_bytes())
        );

        match NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
            .expect("reopen committed authority")
        {
            NativeCredentialStoreOpen::Ready { journal, .. } => {
                assert_eq!(journal.global_epoch, 1)
            }
            _ => panic!("expected ready committed authority"),
        }
    }

    #[test]
    fn native_mutation_session_round_trips_and_scrubs_staging_before_release() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened =
            NativeKeyringCredentialStore::open_with_boundaries(keyring.clone(), journal_boundary)
                .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let store = match initialized {
            NativeCredentialStoreOpen::Ready { store, .. } => store,
            _ => panic!("expected ready authority"),
        };
        let operation_id = CredentialOperationId::parse("10101010-2020-4030-8040-505050505050")
            .expect("canonical operation id");
        let revision = CredentialRevision::parse("60606060-7070-4080-8090-a0a0a0a0a0a0")
            .expect("canonical revision");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let encoded = CredentialRecordEnvelope::present(
            set_id.clone(),
            AuthMethodId::ApiKey,
            revision,
            operation_id.clone(),
            StoredSecretBundle::api_key("native-staging-session-canary").expect("API key"),
        )
        .expect("valid staging record")
        .encode()
        .expect("encode staging record");
        let expected = encoded.as_bytes().to_vec();

        let mut session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin native mutation session");
        session.load_journal().expect("load paired native journal");
        session
            .write_staging(&operation_id, &set_id, encoded)
            .expect("write staging record");
        let readback = session
            .read_staging(&operation_id, &set_id)
            .expect("read staging record")
            .expect("staging record exists");
        assert_eq!(readback.as_bytes(), expected.as_slice());
        session
            .delete_staging(&operation_id, &set_id)
            .expect("delete and verify staging record");
        assert!(
            session
                .read_staging(&operation_id, &set_id)
                .expect("read deleted staging record")
                .is_none()
        );
        drop(session);

        assert!(
            keyring
                .entries
                .lock()
                .expect("memory keyring lock")
                .iter()
                .all(|(account, _)| !account.starts_with("v2-staging/"))
        );
    }

    #[test]
    fn mutation_session_rejects_scope_drift_and_unverified_overwrite_before_io() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let gate = NativeInteractionGate::isolated_for_test();
        let store = ready_store_with_gate(keyring.clone(), journal.clone(), gate);
        let operation_id = CredentialOperationId::parse("78787878-9a9a-4bcd-8f01-232323232323")
            .expect("canonical scoped operation id");
        let other_operation = CredentialOperationId::parse("89898989-abab-4cde-8012-343434343434")
            .expect("canonical other operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let other_set = CredentialSetId::from(BuiltInCredentialSetId::Assemblyai);
        let staging_record = CredentialRecordEnvelope::present(
            set_id.clone(),
            AuthMethodId::ApiKey,
            CredentialRevision::parse("dededede-f0f0-4123-a456-789abcdef012")
                .expect("canonical scoped revision"),
            operation_id.clone(),
            StoredSecretBundle::api_key("scope-drift-canary").expect("API key"),
        )
        .expect("valid scoped record")
        .encode()
        .expect("encode scoped record");
        let active_record = staging_record.copy_for_boundary();
        let replacement_record = active_record.copy_for_boundary();
        let mut session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin scoped mutation");
        let paired = session.load_journal().expect("load scoped journal");

        let reads_before_scope_drift = keyring.reads.load(Ordering::SeqCst);
        assert!(matches!(
            session.read_active(&other_set),
            Err(CredentialStoreFailure::Internal)
        ));
        assert_eq!(
            keyring.reads.load(Ordering::SeqCst),
            reads_before_scope_drift
        );
        session
            .write_staging(&operation_id, &set_id, staging_record)
            .expect("write scoped staging record");
        let reads_before_wrong_staging = keyring.reads.load(Ordering::SeqCst);
        assert!(matches!(
            session.read_staging(&other_operation, &set_id),
            Err(CredentialStoreFailure::Internal)
        ));
        assert_eq!(
            keyring.reads.load(Ordering::SeqCst),
            reads_before_wrong_staging
        );
        assert!(matches!(
            session.read_active(&set_id),
            Err(CredentialStoreFailure::OperationInProgress)
        ));
        assert!(matches!(
            session.readback_active(&set_id),
            Err(CredentialStoreFailure::OperationInProgress)
        ));
        assert_eq!(
            keyring.reads.load(Ordering::SeqCst),
            reads_before_wrong_staging
        );
        let journal_reads_before_pending = journal.reads.load(Ordering::SeqCst);
        assert_eq!(
            session.load_journal(),
            Err(CredentialStoreFailure::OperationInProgress)
        );
        assert_eq!(
            journal.reads.load(Ordering::SeqCst),
            journal_reads_before_pending
        );
        let journal_writes_before_pending = journal.writes.load(Ordering::SeqCst);
        assert_eq!(
            session.commit_journal(&paired),
            Err(CredentialStoreFailure::OperationInProgress)
        );
        assert_eq!(
            journal.writes.load(Ordering::SeqCst),
            journal_writes_before_pending
        );
        session
            .read_staging(&operation_id, &set_id)
            .expect("verify scoped staging record")
            .expect("scoped staging exists");
        session
            .delete_staging(&operation_id, &set_id)
            .expect("delete verified staging record");

        session
            .replace_active(&set_id, active_record)
            .expect("write scoped active record");
        let writes_before_overwrite = keyring.writes.load(Ordering::SeqCst);
        assert_eq!(
            session.replace_active(&set_id, replacement_record),
            Err(CredentialStoreFailure::OperationInProgress)
        );
        assert_eq!(
            keyring.writes.load(Ordering::SeqCst),
            writes_before_overwrite
        );
        let reads_before_wrong_active = keyring.reads.load(Ordering::SeqCst);
        assert!(matches!(
            session.readback_active(&other_set),
            Err(CredentialStoreFailure::Internal)
        ));
        assert_eq!(
            keyring.reads.load(Ordering::SeqCst),
            reads_before_wrong_active
        );
        assert!(matches!(
            session.read_active(&set_id),
            Err(CredentialStoreFailure::OperationInProgress)
        ));
        assert!(matches!(
            session.read_staging(&operation_id, &set_id),
            Err(CredentialStoreFailure::OperationInProgress)
        ));
        assert_eq!(
            keyring.reads.load(Ordering::SeqCst),
            reads_before_wrong_active
        );
        let journal_reads_before_active_pending = journal.reads.load(Ordering::SeqCst);
        assert_eq!(
            session.load_journal(),
            Err(CredentialStoreFailure::OperationInProgress)
        );
        assert_eq!(
            journal.reads.load(Ordering::SeqCst),
            journal_reads_before_active_pending
        );
        assert_eq!(
            session.commit_journal(&paired),
            Err(CredentialStoreFailure::OperationInProgress)
        );
        session
            .readback_active(&set_id)
            .expect("verify scoped active record")
            .expect("scoped active exists");
        session
            .commit_journal(&paired)
            .expect("commit after all native readbacks");
    }

    #[test]
    fn dropping_pending_readback_latches_active_and_staging_but_verified_drop_remains_healthy() {
        fn record(
            set_id: &CredentialSetId,
            operation_id: &CredentialOperationId,
            revision: &str,
            secret: &str,
        ) -> EncodedCredentialRecord {
            CredentialRecordEnvelope::present(
                set_id.clone(),
                AuthMethodId::ApiKey,
                CredentialRevision::parse(revision).expect("canonical drop-test revision"),
                operation_id.clone(),
                StoredSecretBundle::api_key(secret).expect("API key"),
            )
            .expect("valid drop-test record")
            .encode()
            .expect("encode drop-test record")
        }

        let active_keyring = Arc::new(MemoryKeyringBoundary::default());
        let active_journal = Arc::new(MemoryJournalBoundary::default());
        let active_store = ready_store_with_gate(
            active_keyring.clone(),
            active_journal.clone(),
            NativeInteractionGate::isolated_for_test(),
        );
        let active_operation = CredentialOperationId::parse("90909090-bcbc-4def-8123-454545454545")
            .expect("canonical pending active operation id");
        let active_set = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let mut active_session = active_store
            .begin_mutation(&active_operation, &active_set)
            .expect("begin pending active mutation");
        active_session
            .load_journal()
            .expect("load pending active journal");
        active_session
            .replace_active(
                &active_set,
                record(
                    &active_set,
                    &active_operation,
                    "efefefef-0101-4234-b567-89abcdef0123",
                    "pending-active-drop-canary",
                ),
            )
            .expect("write pending active record");
        let active_reads = active_keyring.reads.load(Ordering::SeqCst);
        let active_journal_reads = active_journal.reads.load(Ordering::SeqCst);
        let active_journal_writes = active_journal.writes.load(Ordering::SeqCst);
        drop(active_session);
        assert!(matches!(
            active_store.read_active(&active_set, &|_| true),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(active_keyring.reads.load(Ordering::SeqCst), active_reads);
        assert_eq!(
            active_journal.reads.load(Ordering::SeqCst),
            active_journal_reads
        );
        assert_eq!(
            active_journal.writes.load(Ordering::SeqCst),
            active_journal_writes
        );

        let staging_keyring = Arc::new(MemoryKeyringBoundary::default());
        let staging_journal = Arc::new(MemoryJournalBoundary::default());
        let staging_store = ready_store_with_gate(
            staging_keyring.clone(),
            staging_journal.clone(),
            NativeInteractionGate::isolated_for_test(),
        );
        let staging_operation =
            CredentialOperationId::parse("a1a1a1a1-cdcd-4ef0-8234-565656565656")
                .expect("canonical pending staging operation id");
        let staging_set = CredentialSetId::from(BuiltInCredentialSetId::Assemblyai);
        let mut staging_session = staging_store
            .begin_mutation(&staging_operation, &staging_set)
            .expect("begin pending staging mutation");
        staging_session
            .load_journal()
            .expect("load pending staging journal");
        staging_session
            .write_staging(
                &staging_operation,
                &staging_set,
                record(
                    &staging_set,
                    &staging_operation,
                    "f0f0f0f0-1212-4345-8678-9abcdef01234",
                    "pending-staging-drop-canary",
                ),
            )
            .expect("write pending staging record");
        let staging_reads = staging_keyring.reads.load(Ordering::SeqCst);
        let staging_journal_reads = staging_journal.reads.load(Ordering::SeqCst);
        let staging_journal_writes = staging_journal.writes.load(Ordering::SeqCst);
        drop(staging_session);
        assert!(matches!(
            staging_store.read_active(&staging_set, &|_| true),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(staging_keyring.reads.load(Ordering::SeqCst), staging_reads);
        assert_eq!(
            staging_journal.reads.load(Ordering::SeqCst),
            staging_journal_reads
        );
        assert_eq!(
            staging_journal.writes.load(Ordering::SeqCst),
            staging_journal_writes
        );

        let verified_keyring = Arc::new(MemoryKeyringBoundary::default());
        let verified_store = ready_store_with_gate(
            verified_keyring.clone(),
            Arc::new(MemoryJournalBoundary::default()),
            NativeInteractionGate::isolated_for_test(),
        );
        let verified_operation =
            CredentialOperationId::parse("b2b2b2b2-dede-4f01-8345-676767676767")
                .expect("canonical verified operation id");
        let verified_set = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let staging = record(
            &verified_set,
            &verified_operation,
            "01010101-2323-4456-8789-abcdef012345",
            "verified-drop-canary",
        );
        let active = staging.copy_for_boundary();
        let mut verified_session = verified_store
            .begin_mutation(&verified_operation, &verified_set)
            .expect("begin verified mutation");
        verified_session
            .load_journal()
            .expect("load verified journal");
        verified_session
            .write_staging(&verified_operation, &verified_set, staging)
            .expect("write verified staging");
        verified_session
            .read_staging(&verified_operation, &verified_set)
            .expect("verify staging")
            .expect("verified staging exists");
        verified_session
            .delete_staging(&verified_operation, &verified_set)
            .expect("delete verified staging");
        verified_session
            .replace_active(&verified_set, active)
            .expect("write verified active");
        verified_session
            .readback_active(&verified_set)
            .expect("verify active")
            .expect("verified active exists");
        drop(verified_session);
        let verified_reads = verified_keyring.reads.load(Ordering::SeqCst);
        assert!(matches!(
            verified_store.read_active(&verified_set, &|_| true),
            Ok(AuthoritativeActiveRead {
                record: Some(_),
                ..
            })
        ));
        // One authority-marker read plus the record read.
        assert_eq!(
            verified_keyring.reads.load(Ordering::SeqCst),
            verified_reads + 2
        );
    }

    #[test]
    fn wrong_active_and_staging_readback_latch_before_later_native_io() {
        fn ready_store(
            keyring: Arc<MemoryKeyringBoundary>,
            journal: Arc<MemoryJournalBoundary>,
            gate: &'static NativeInteractionGate,
        ) -> NativeKeyringCredentialStore {
            let opened =
                NativeKeyringCredentialStore::open_with_boundaries_and_gate(keyring, journal, gate)
                    .expect("open absent authority");
            let store = match opened {
                NativeCredentialStoreOpen::Uninitialized(store) => store,
                _ => panic!("expected uninitialized authority"),
            };
            match store
                .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
                .expect("initialize native authority")
            {
                NativeCredentialStoreOpen::Ready { store, .. } => store,
                _ => panic!("expected ready authority"),
            }
        }

        let active_keyring = Arc::new(MemoryKeyringBoundary::default());
        let active_journal = Arc::new(MemoryJournalBoundary::default());
        let active_gate = NativeInteractionGate::isolated_for_test();
        let active_store = ready_store(active_keyring.clone(), active_journal.clone(), active_gate);
        let active_operation = CredentialOperationId::parse("12121212-3434-4567-89ab-cdcdcdcdcdcd")
            .expect("canonical active operation id");
        let active_set = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let active_record = CredentialRecordEnvelope::present(
            active_set.clone(),
            AuthMethodId::ApiKey,
            CredentialRevision::parse("abababab-cdcd-4efe-8123-456789abcdef")
                .expect("canonical active revision"),
            active_operation.clone(),
            StoredSecretBundle::api_key("wrong-active-readback-canary").expect("API key"),
        )
        .expect("valid active record")
        .encode()
        .expect("encode active record");
        let mut active_session = active_store
            .begin_mutation(&active_operation, &active_set)
            .expect("begin active mutation");
        let active_paired = active_session.load_journal().expect("load active journal");
        active_session
            .replace_active(&active_set, active_record)
            .expect("write active record");
        active_keyring
            .corrupt_active_readback
            .store(true, Ordering::SeqCst);
        assert!(matches!(
            active_session.readback_active(&active_set),
            Err(CredentialStoreFailure::CommitUnknown)
        ));
        let active_reads = active_keyring.reads.load(Ordering::SeqCst);
        let active_writes = active_keyring.writes.load(Ordering::SeqCst);
        let active_deletes = active_keyring.deletes.load(Ordering::SeqCst);
        let active_journal_reads = active_journal.reads.load(Ordering::SeqCst);
        let active_journal_writes = active_journal.writes.load(Ordering::SeqCst);
        assert_eq!(
            active_session.load_journal(),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert_eq!(
            active_session.commit_journal(&active_paired),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert!(matches!(
            active_session.read_active(&active_set),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(active_keyring.reads.load(Ordering::SeqCst), active_reads);
        assert_eq!(active_keyring.writes.load(Ordering::SeqCst), active_writes);
        assert_eq!(
            active_keyring.deletes.load(Ordering::SeqCst),
            active_deletes
        );
        assert_eq!(
            active_journal.reads.load(Ordering::SeqCst),
            active_journal_reads
        );
        assert_eq!(
            active_journal.writes.load(Ordering::SeqCst),
            active_journal_writes
        );
        drop(active_session);
        assert!(matches!(
            active_store.read_active(&active_set, &|_| true),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(active_keyring.reads.load(Ordering::SeqCst), active_reads);

        let staging_keyring = Arc::new(MemoryKeyringBoundary::default());
        let staging_journal = Arc::new(MemoryJournalBoundary::default());
        let staging_gate = NativeInteractionGate::isolated_for_test();
        let staging_store = ready_store(
            staging_keyring.clone(),
            staging_journal.clone(),
            staging_gate,
        );
        let staging_operation =
            CredentialOperationId::parse("23232323-4545-4678-8abc-dededededede")
                .expect("canonical staging operation id");
        let staging_set = CredentialSetId::from(BuiltInCredentialSetId::Assemblyai);
        let staging_record = CredentialRecordEnvelope::present(
            staging_set.clone(),
            AuthMethodId::ApiKey,
            CredentialRevision::parse("bcbcbcbc-dede-4f0f-9234-56789abcdef0")
                .expect("canonical staging revision"),
            staging_operation.clone(),
            StoredSecretBundle::api_key("wrong-staging-readback-canary").expect("API key"),
        )
        .expect("valid staging record")
        .encode()
        .expect("encode staging record");
        let mut staging_session = staging_store
            .begin_mutation(&staging_operation, &staging_set)
            .expect("begin staging mutation");
        let staging_paired = staging_session
            .load_journal()
            .expect("load staging journal");
        staging_session
            .write_staging(&staging_operation, &staging_set, staging_record)
            .expect("write staging record");
        staging_keyring
            .corrupt_staging_readback
            .store(true, Ordering::SeqCst);
        assert!(matches!(
            staging_session.read_staging(&staging_operation, &staging_set),
            Err(CredentialStoreFailure::CommitUnknown)
        ));
        let staging_reads = staging_keyring.reads.load(Ordering::SeqCst);
        let staging_writes = staging_keyring.writes.load(Ordering::SeqCst);
        let staging_deletes = staging_keyring.deletes.load(Ordering::SeqCst);
        let staging_journal_reads = staging_journal.reads.load(Ordering::SeqCst);
        let staging_journal_writes = staging_journal.writes.load(Ordering::SeqCst);
        assert_eq!(
            staging_session.load_journal(),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert_eq!(
            staging_session.commit_journal(&staging_paired),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert!(matches!(
            staging_session.read_active(&staging_set),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(staging_keyring.reads.load(Ordering::SeqCst), staging_reads);
        assert_eq!(
            staging_keyring.writes.load(Ordering::SeqCst),
            staging_writes
        );
        assert_eq!(
            staging_keyring.deletes.load(Ordering::SeqCst),
            staging_deletes
        );
        assert_eq!(
            staging_journal.reads.load(Ordering::SeqCst),
            staging_journal_reads
        );
        assert_eq!(
            staging_journal.writes.load(Ordering::SeqCst),
            staging_journal_writes
        );
        drop(staging_session);
        assert!(matches!(
            staging_store.read_active(&staging_set, &|_| true),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(staging_keyring.reads.load(Ordering::SeqCst), staging_reads);
    }

    #[test]
    fn complete_ordinary_policy_trace_is_forbid_prompt_only() {
        fn take_trace(keyring: &MemoryKeyringBoundary) -> Vec<PolicyTraceStep> {
            std::mem::take(&mut *keyring.policy_trace.lock().expect("policy trace lock"))
        }

        fn assert_single_lease_trace(
            actual: &[PolicyTraceStep],
            expected_steps: &[(&'static str, PolicyTraceTarget)],
        ) {
            assert_eq!(
                actual
                    .iter()
                    .map(|step| (step.operation, step.target))
                    .collect::<Vec<_>>(),
                expected_steps
            );
            let prompt_id = actual.first().expect("non-empty policy trace").prompt_id;
            assert!(
                actual
                    .iter()
                    .all(|candidate| candidate.prompt_id == prompt_id),
                "one full interaction lease must supply one opaque prompt identity"
            );
        }

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let gate = NativeInteractionGate::isolated_for_test();
        let journal = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries_and_gate(
            keyring.clone(),
            journal,
            gate,
        )
        .expect("open absent authority");
        // No authority pair, so open gathers the evidence that none ever
        // existed: every built-in active record and staging guard.
        let mut no_remnants = Vec::new();
        for set_id in BUILT_IN_CREDENTIAL_SET_IDS.iter().copied() {
            no_remnants.push(("forbid:get:active", PolicyTraceTarget::Active(set_id)));
            no_remnants.push(("forbid:get:guard", PolicyTraceTarget::StagingGuard(set_id)));
        }
        let mut expected_open = vec![("forbid:get:authority", PolicyTraceTarget::Authority)];
        expected_open.extend(no_remnants.iter().copied());
        assert_single_lease_trace(&take_trace(&keyring), &expected_open);
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        assert_eq!(BUILT_IN_CREDENTIAL_SET_IDS.len(), 17);
        let mut expected_initialize = vec![("forbid:get:authority", PolicyTraceTarget::Authority)];
        expected_initialize.extend(no_remnants.iter().copied());
        expected_initialize.extend([
            ("forbid:set:authority", PolicyTraceTarget::Authority),
            ("forbid:get:authority", PolicyTraceTarget::Authority),
        ]);
        assert_single_lease_trace(&take_trace(&keyring), &expected_initialize);
        let store = match initialized {
            NativeCredentialStoreOpen::Ready { store, .. } => store,
            _ => panic!("expected ready authority"),
        };
        let operation_id = CredentialOperationId::parse("34343434-5656-4789-8bcd-efefefefefef")
            .expect("canonical trace operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        assert!(
            store
                .read_active(&set_id, &|_| true)
                .expect("provider-shaped active read")
                .record
                .is_none()
        );
        // The provider read revalidates the bound authority under the same
        // lease before it touches the record.
        assert_single_lease_trace(
            &take_trace(&keyring),
            &[
                ("forbid:get:authority", PolicyTraceTarget::Authority),
                (
                    "forbid:get:active",
                    PolicyTraceTarget::Active(BuiltInCredentialSetId::Deepgram),
                ),
            ],
        );
        let staging_record = CredentialRecordEnvelope::present(
            set_id.clone(),
            AuthMethodId::ApiKey,
            CredentialRevision::parse("cdcdcdcd-efef-4012-a345-6789abcdef01")
                .expect("canonical trace revision"),
            operation_id.clone(),
            StoredSecretBundle::api_key("policy-trace-canary").expect("API key"),
        )
        .expect("valid trace record")
        .encode()
        .expect("encode trace record");
        let active_record = staging_record.copy_for_boundary();
        let mut session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin trace mutation");
        let paired = session.load_journal().expect("load trace journal");
        session
            .write_staging(&operation_id, &set_id, staging_record)
            .expect("write trace staging");
        session
            .read_staging(&operation_id, &set_id)
            .expect("read trace staging")
            .expect("trace staging exists");
        session
            .delete_staging(&operation_id, &set_id)
            .expect("cleanup trace staging");
        session
            .replace_active(&set_id, active_record)
            .expect("replace trace active");
        session
            .readback_active(&set_id)
            .expect("read back trace active")
            .expect("trace active exists");
        session
            .commit_journal(&paired)
            .expect("commit trace journal readback");
        drop(session);
        assert_single_lease_trace(
            &take_trace(&keyring),
            &[
                ("forbid:get:authority", PolicyTraceTarget::Authority),
                (
                    "forbid:get:guard",
                    PolicyTraceTarget::StagingGuard(BuiltInCredentialSetId::Deepgram),
                ),
                (
                    "forbid:set:guard",
                    PolicyTraceTarget::StagingGuard(BuiltInCredentialSetId::Deepgram),
                ),
                (
                    "forbid:get:guard",
                    PolicyTraceTarget::StagingGuard(BuiltInCredentialSetId::Deepgram),
                ),
                ("forbid:set:staging", PolicyTraceTarget::Staging),
                ("forbid:get:staging", PolicyTraceTarget::Staging),
                ("forbid:delete:staging", PolicyTraceTarget::Staging),
                ("forbid:get:staging", PolicyTraceTarget::Staging),
                (
                    "forbid:get:guard",
                    PolicyTraceTarget::StagingGuard(BuiltInCredentialSetId::Deepgram),
                ),
                (
                    "forbid:delete:guard",
                    PolicyTraceTarget::StagingGuard(BuiltInCredentialSetId::Deepgram),
                ),
                (
                    "forbid:get:guard",
                    PolicyTraceTarget::StagingGuard(BuiltInCredentialSetId::Deepgram),
                ),
                (
                    "forbid:set:active",
                    PolicyTraceTarget::Active(BuiltInCredentialSetId::Deepgram),
                ),
                (
                    "forbid:get:active",
                    PolicyTraceTarget::Active(BuiltInCredentialSetId::Deepgram),
                ),
            ],
        );
    }

    #[test]
    fn an_unverifiable_journal_publication_is_commit_unknown_and_reopens_recovery_required() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let (store, journal) = match initialized {
            NativeCredentialStoreOpen::Ready { store, journal } => (store, *journal),
            _ => panic!("expected ready authority"),
        };
        journal_boundary
            .corrupt_replacements
            .store(true, Ordering::SeqCst);
        let service = CredentialService::new(
            Arc::new(store),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        let error = service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::from(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("journal-readback-canary").expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
                )
                .expect("canonical idempotency token"),
            })
            .expect_err("a publication its boundary cannot verify cannot commit");

        assert_eq!(
            error.code,
            audio_graph_ipc_contract::credential_contract::CredentialErrorCode::CommitUnknown
        );
        assert!(
            !keyring
                .entries
                .lock()
                .expect("memory keyring lock")
                .iter()
                .any(|(account, _)| account == "v2/deepgram")
        );
        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                .expect("reopen corrupt authority"),
            NativeCredentialStoreOpen::RecoveryRequired
        ));
    }

    /// A lease the journal forbids is refused on the journal alone, read
    /// under the lock: the record's secret bytes are never requested, and no
    /// failure reading them can mask the refusal.
    #[test]
    fn a_lease_the_journal_forbids_requests_no_record_bytes() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let (store, journal) = match NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            journal_boundary,
        )
        .expect("open absent authority")
        {
            NativeCredentialStoreOpen::Uninitialized(store) => match store
                .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
                .expect("initialize native authority")
            {
                NativeCredentialStoreOpen::Ready { store, journal } => (Arc::new(store), *journal),
                _ => panic!("expected ready authority"),
            },
            _ => panic!("expected uninitialized authority"),
        };
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("forbidden-lease-canary").expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "6a6a6a6a-6a6a-4a6a-8a6a-6a6a6a6a6a6a",
                )
                .expect("canonical idempotency token"),
            })
            .expect("configure Deepgram");
        // Another process marks the set for reconciliation.
        let operation_id = CredentialOperationId::parse("6b6b6b6b-6b6b-4b6b-8b6b-6b6b6b6b6b6b")
            .expect("canonical operation id");
        let mut session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin mutation");
        let mut marked = session.load_journal().expect("load journal");
        marked
            .set_state_mut(&set_id)
            .expect("Deepgram state")
            .recovery_state = CredentialSetRecoveryState::RecordJournalMismatch;
        session
            .persist_intent(&marked)
            .expect("persist marked journal");
        drop(session);
        let native_reads = keyring.reads.load(Ordering::SeqCst);

        let error = service
            .resolve_for_use(CredentialUseRequest {
                set_id,
                consumer_id: "asr.deepgram",
                auth_method_id: AuthMethodId::ApiKey,
                purpose: CredentialPurpose::Asr,
                audience: CredentialAudience::SecureNetworkOrigin {
                    scheme: SecureTransportScheme::Wss,
                    canonical_host: "api.deepgram.com".to_string(),
                    effective_port: 443,
                },
            })
            .expect_err("the journal forbids a lease");

        assert_eq!(error.code, CredentialErrorCode::RecoveryRequired);
        assert_eq!(
            keyring.reads.load(Ordering::SeqCst),
            native_reads + 1,
            "only the authority marker is read"
        );
    }

    #[test]
    fn opaque_mutation_session_owns_the_file_lock_until_drop() {
        let root = std::env::temp_dir().join(format!(
            "audio-graph-fb2b-session-lock-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(
            FileAuthorityJournalBoundary::open_for_test(
                root.join("journal"),
                root.join("namespace"),
            )
            .expect("open journal boundary"),
        );
        let opened = NativeKeyringCredentialStore::open_with_adapters(
            KeyringEntryAdapter::new(keyring),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let store = match initialized {
            NativeCredentialStoreOpen::Ready { store, .. } => store,
            _ => panic!("expected ready authority"),
        };
        let operation_id = CredentialOperationId::parse("11111111-2222-3333-4444-555555555555")
            .expect("canonical operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);

        let session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin mutation session");
        assert!(matches!(
            journal_boundary.acquire_mutation_lock(),
            Err(CredentialStoreFailure::OperationInProgress)
        ));

        drop(session);
        let reacquired = journal_boundary
            .acquire_mutation_lock()
            .expect("session drop releases file lock");
        drop(reacquired);
        drop(store);
        drop(journal_boundary);
        std::fs::remove_dir_all(&root).expect("remove isolated session-lock test directory");
    }

    #[test]
    fn mutation_session_serializes_all_in_process_entry_access_until_drop() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
            .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let store = match initialized {
            NativeCredentialStoreOpen::Ready { store, .. } => Arc::new(store),
            _ => panic!("expected ready authority"),
        };
        let operation_id = CredentialOperationId::parse("11111111-2222-3333-4444-555555555555")
            .expect("canonical operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin mutation session");
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let reader_store = store.clone();
        let reader_set_id = set_id.clone();
        let reader = std::thread::spawn(move || {
            started_tx.send(()).expect("announce blocked read");
            let result = reader_store.read_active(&reader_set_id, &|_| true);
            finished_tx
                .send(result.is_ok())
                .expect("report read result");
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("reader started");

        assert!(
            finished_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        drop(session);
        assert!(
            finished_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("reader completes after session drop")
        );
        reader.join().expect("reader thread");
    }

    #[test]
    fn ready_session_journal_cuts_preserve_prepublication_error_and_latch_uncertainty() {
        let before_keyring = Arc::new(MemoryKeyringBoundary::default());
        let before_journal = Arc::new(MemoryJournalBoundary::default());
        let before_gate = NativeInteractionGate::isolated_for_test();
        let before_store =
            ready_store_with_gate(before_keyring.clone(), before_journal.clone(), before_gate);
        let before_operation = CredentialOperationId::parse("45454545-6767-489a-8cde-f0f0f0f0f0f0")
            .expect("canonical before-publish operation id");
        let before_set = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let mut before_session = before_store
            .begin_mutation(&before_operation, &before_set)
            .expect("begin before-publish mutation");
        let before_paired = before_session
            .load_journal()
            .expect("load before-publish journal");
        *before_journal
            .replace_cut
            .lock()
            .expect("journal replace cut lock") = Some(JournalReplaceCut::BeforePublish);
        assert_eq!(
            before_session.commit_journal(&before_paired),
            Err(CredentialStoreFailure::Unavailable)
        );
        drop(before_session);
        let before_reads = before_keyring.reads.load(Ordering::SeqCst);
        assert!(matches!(
            before_store.read_active(&before_set, &|_| true),
            Ok(AuthoritativeActiveRead { record: None, .. })
        ));
        assert_eq!(
            before_keyring.reads.load(Ordering::SeqCst),
            before_reads + 2
        );

        let after_keyring = Arc::new(MemoryKeyringBoundary::default());
        let after_journal = Arc::new(MemoryJournalBoundary::default());
        let after_gate = NativeInteractionGate::isolated_for_test();
        let after_store =
            ready_store_with_gate(after_keyring.clone(), after_journal.clone(), after_gate);
        let after_operation = CredentialOperationId::parse("56565656-7878-49ab-8def-010101010101")
            .expect("canonical after-publish operation id");
        let after_set = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let mut after_session = after_store
            .begin_mutation(&after_operation, &after_set)
            .expect("begin after-publish mutation");
        let after_paired = after_session
            .load_journal()
            .expect("load after-publish journal");
        *after_journal
            .replace_cut
            .lock()
            .expect("journal replace cut lock") = Some(JournalReplaceCut::AfterPublish);
        assert_eq!(
            after_session.commit_journal(&after_paired),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        let latched_keyring_reads = after_keyring.reads.load(Ordering::SeqCst);
        let latched_keyring_writes = after_keyring.writes.load(Ordering::SeqCst);
        let latched_journal_reads = after_journal.reads.load(Ordering::SeqCst);
        let latched_journal_writes = after_journal.writes.load(Ordering::SeqCst);
        assert_eq!(
            after_session.load_journal(),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert_eq!(
            after_session.commit_journal(&after_paired),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert!(matches!(
            after_session.read_active(&after_set),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(
            after_keyring.reads.load(Ordering::SeqCst),
            latched_keyring_reads
        );
        assert_eq!(
            after_keyring.writes.load(Ordering::SeqCst),
            latched_keyring_writes
        );
        assert_eq!(
            after_journal.reads.load(Ordering::SeqCst),
            latched_journal_reads
        );
        assert_eq!(
            after_journal.writes.load(Ordering::SeqCst),
            latched_journal_writes
        );
        drop(after_session);
        let after_keyring_reads = after_keyring.reads.load(Ordering::SeqCst);
        let after_journal_reads = after_journal.reads.load(Ordering::SeqCst);
        assert!(matches!(
            after_store.read_active(&after_set, &|_| true),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(
            after_keyring.reads.load(Ordering::SeqCst),
            after_keyring_reads
        );
        assert_eq!(
            after_journal.reads.load(Ordering::SeqCst),
            after_journal_reads
        );
    }

    #[test]
    fn shared_injected_gate_serializes_distinct_store_instances() {
        let gate = NativeInteractionGate::isolated_for_test();
        let first_keyring = Arc::new(MemoryKeyringBoundary::default());
        let first_store = Arc::new(ready_store_with_gate(
            first_keyring,
            Arc::new(MemoryJournalBoundary::default()),
            gate,
        ));
        let second_keyring = Arc::new(MemoryKeyringBoundary::default());
        let second_store = Arc::new(ready_store_with_gate(
            second_keyring.clone(),
            Arc::new(MemoryJournalBoundary::default()),
            gate,
        ));
        let operation_id = CredentialOperationId::parse("67676767-8989-4abc-8ef0-121212121212")
            .expect("canonical shared-gate operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let session = first_store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin first-store mutation");
        let second_reads_before = second_keyring.reads.load(Ordering::SeqCst);
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let reader_store = second_store.clone();
        let reader_set_id = set_id.clone();
        let reader = std::thread::spawn(move || {
            started_tx.send(()).expect("announce second-store read");
            let result = reader_store.read_active(&reader_set_id, &|_| true);
            finished_tx
                .send(result)
                .expect("report second-store read result");
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second-store reader started");
        let deadline = Instant::now() + Duration::from_secs(1);
        while gate.queued_waiters_for_test() == 0 {
            assert!(
                Instant::now() < deadline,
                "second store did not queue on shared native interaction gate"
            );
            std::thread::yield_now();
        }

        assert_eq!(
            second_keyring.reads.load(Ordering::SeqCst),
            second_reads_before
        );
        assert!(finished_rx.try_recv().is_err());
        drop(session);
        assert!(matches!(
            finished_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("second-store read completes after lease drop"),
            Ok(AuthoritativeActiveRead { record: None, .. })
        ));
        reader.join().expect("second-store reader thread");
        assert_eq!(
            second_keyring.reads.load(Ordering::SeqCst),
            second_reads_before + 2
        );
    }

    /// A provider read takes the cross-process mutation lock before any
    /// native or journal I/O, so it never observes another process's
    /// half-committed pair.
    #[test]
    fn open_store_reads_only_under_the_mutation_lock() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let store = ready_store_with_gate(
            keyring.clone(),
            journal.clone(),
            NativeInteractionGate::isolated_for_test(),
        );
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let native_reads = keyring.reads.load(Ordering::SeqCst);
        let journal_reads = journal.reads.load(Ordering::SeqCst);
        journal.lock_contended.store(true, Ordering::SeqCst);

        assert!(matches!(
            store.read_active(&set_id, &|_| true),
            Err(CredentialStoreFailure::OperationInProgress)
        ));
        assert_eq!(keyring.reads.load(Ordering::SeqCst), native_reads);
        assert_eq!(journal.reads.load(Ordering::SeqCst), journal_reads);

        journal.lock_contended.store(false, Ordering::SeqCst);
        assert!(matches!(
            store.read_active(&set_id, &|_| true),
            Ok(AuthoritativeActiveRead { record: None, .. })
        ));
    }

    /// An open store is bound to the authority it opened. Losing a half of
    /// that pair, or a fresh authority taking its place, refuses the read
    /// before the record is touched.
    #[test]
    fn open_store_refuses_reads_once_its_bound_authority_is_lost_or_replaced() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let gate = NativeInteractionGate::isolated_for_test();
        let original = ready_store_with_gate(keyring.clone(), journal.clone(), gate);
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        assert!(matches!(
            original.read_active(&set_id, &|_| true),
            Ok(AuthoritativeActiveRead { record: None, .. })
        ));

        journal.bytes.lock().expect("memory journal lock").take();
        let native_reads = keyring.reads.load(Ordering::SeqCst);
        assert!(matches!(
            original.read_active(&set_id, &|_| true),
            Err(CredentialStoreFailure::CorruptRecord)
        ));
        // Only the authority marker was read; the record never was.
        assert_eq!(keyring.reads.load(Ordering::SeqCst), native_reads + 1);

        keyring
            .entries
            .lock()
            .expect("memory keyring lock")
            .retain(|(account, _)| account != "v2/_authority");
        let replacement = ready_store_with_gate(keyring.clone(), journal.clone(), gate);
        assert!(matches!(
            replacement.read_active(&set_id, &|_| true),
            Ok(AuthoritativeActiveRead { record: None, .. })
        ));
        assert!(matches!(
            original.read_active(&set_id, &|_| true),
            Err(CredentialStoreFailure::CorruptRecord)
        ));
    }

    /// Stages a record for `operation` and leaves it, as a cleanup that
    /// never ran would.
    fn stage_and_leave(
        store: &NativeKeyringCredentialStore,
        operation: &str,
        set_id: &CredentialSetId,
    ) -> CredentialOperationId {
        let operation_id = CredentialOperationId::parse(operation).expect("canonical operation id");
        let mut session = store
            .begin_mutation(&operation_id, set_id)
            .expect("begin mutation");
        session.load_journal().expect("load journal");
        session
            .write_staging(&operation_id, set_id, remnant_record(set_id, &operation_id))
            .expect("write staging record");
        session
            .read_staging(&operation_id, set_id)
            .expect("read back staging record")
            .expect("staging record exists");
        operation_id
    }

    fn native_entry(keyring: &MemoryKeyringBoundary, account: &str) -> Option<Vec<u8>> {
        keyring
            .entries
            .lock()
            .expect("memory keyring lock")
            .iter()
            .find(|(candidate, _)| candidate == account)
            .map(|(_, bytes)| bytes.clone())
    }

    /// Another operation's cleanup verifies its own staging record absent
    /// but leaves the guard naming the operation whose staging record
    /// remains, so that record stays discoverable after the authority is
    /// lost.
    #[test]
    fn another_operations_cleanup_leaves_the_guard_of_a_remaining_staging_record() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let gate = NativeInteractionGate::isolated_for_test();
        let store = ready_store_with_gate(keyring.clone(), journal.clone(), gate);
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let staged = stage_and_leave(&store, "65656565-6565-4565-8565-656565656565", &set_id);
        let other = CredentialOperationId::parse("66666666-6666-4666-8666-666666666666")
            .expect("canonical operation id");
        let mut session = store
            .begin_mutation(&other, &set_id)
            .expect("begin mutation");
        session.load_journal().expect("load journal");
        session
            .delete_staging(&other, &set_id)
            .expect("nothing of its own to clean up");
        drop(session);
        drop(store);

        assert_eq!(
            native_entry(&keyring, EntryLocator::staging_guard(&set_id).account()),
            Some(staging_guard_bytes(&staged))
        );
        lose_both_authority_halves(&keyring, &journal);
        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries_and_gate(keyring, journal, gate),
            Ok(NativeCredentialStoreOpen::RecoveryRequired)
        ));
    }

    /// While one operation's staging record remains, another cannot stage
    /// for the same set, so a set never has two staging records. Once that
    /// record is gone, its leftover guard is taken over.
    #[test]
    fn a_remaining_staging_record_holds_its_guard_against_another_operation() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let store = ready_store_with_gate(
            keyring.clone(),
            journal,
            NativeInteractionGate::isolated_for_test(),
        );
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let staged = stage_and_leave(&store, "67676767-6767-4767-8767-676767676767", &set_id);
        let other = CredentialOperationId::parse("68686868-6868-4868-8868-686868686868")
            .expect("canonical operation id");
        let mut session = store
            .begin_mutation(&other, &set_id)
            .expect("begin mutation");
        session.load_journal().expect("load journal");

        assert!(matches!(
            session.write_staging(&other, &set_id, remnant_record(&set_id, &other)),
            Err(CredentialStoreFailure::CorruptRecord)
        ));
        assert_eq!(
            native_entry(&keyring, EntryLocator::staging(&other, &set_id).account()),
            None
        );
        drop(session);

        let staged_account = EntryLocator::staging(&staged, &set_id).account().to_owned();
        keyring
            .entries
            .lock()
            .expect("memory keyring lock")
            .retain(|(account, _)| *account != staged_account);
        let mut session = store
            .begin_mutation(&other, &set_id)
            .expect("begin mutation");
        session.load_journal().expect("load journal");
        session
            .write_staging(&other, &set_id, remnant_record(&set_id, &other))
            .expect("a guard whose staging record is gone is taken over");
        assert_eq!(
            native_entry(&keyring, EntryLocator::staging_guard(&set_id).account()),
            Some(staging_guard_bytes(&other))
        );
    }

    /// A staged journal file that a failed discard left behind is removed by
    /// the next open, under the lock; one that cannot be removed fails that
    /// open closed instead of being ignored.
    #[test]
    fn opening_a_store_removes_a_stray_staged_journal_file_or_fails_closed() {
        let root = std::env::temp_dir().join(format!(
            "audio-graph-521-open-sweep-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let gate = NativeInteractionGate::isolated_for_test();
        let open = || {
            NativeKeyringCredentialStore::open_with_boundaries_and_gate(
                keyring.clone(),
                Arc::new(
                    FileAuthorityJournalBoundary::open_for_test(
                        root.join("journal"),
                        root.join("namespace"),
                    )
                    .expect("open journal boundary"),
                ),
                gate,
            )
        };
        match open().expect("open absent authority") {
            NativeCredentialStoreOpen::Uninitialized(store) => {
                store
                    .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
                    .expect("initialize native authority");
            }
            _ => panic!("expected uninitialized authority"),
        }
        let stray = root
            .join("journal")
            .join(format!(".state.json.{}.tmp", uuid::Uuid::new_v4().simple()));
        std::fs::write(&stray, b"stray-journal").expect("plant stray staged file");

        assert!(matches!(
            open(),
            Ok(NativeCredentialStoreOpen::Ready { .. })
        ));
        assert!(!stray.exists(), "stray staged file survived an open");

        std::fs::create_dir(&stray).expect("plant unremovable stray");
        assert!(matches!(open(), Err(CredentialStoreFailure::Unavailable)));
        std::fs::remove_dir_all(&root).expect("remove isolated open-sweep directory");
    }

    /// A publication the journal boundary verified through its held root
    /// stays successful when an ancestor of the journal path points
    /// elsewhere by the time it returns: the session does not read the
    /// journal again by path.
    #[cfg(windows)]
    #[test]
    fn windows_a_verified_publication_through_a_swapped_ancestor_is_not_commit_unknown() {
        let (base, held, armed, boundary) =
            super::super::authority_journal::tests::armed_ancestor_swap_boundary(
                "native-swap-publish",
            );
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let store = match NativeKeyringCredentialStore::open_with_boundaries_and_gate(
            keyring,
            Arc::new(boundary),
            NativeInteractionGate::isolated_for_test(),
        )
        .expect("open absent authority")
        {
            NativeCredentialStoreOpen::Uninitialized(store) => match store
                .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
                .expect("initialize native authority")
            {
                NativeCredentialStoreOpen::Ready { store, .. } => store,
                _ => panic!("expected ready authority"),
            },
            _ => panic!("expected uninitialized authority"),
        };
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let operation_id = CredentialOperationId::parse("69696969-6969-4969-8969-696969696969")
            .expect("canonical operation id");
        let mut session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin mutation");
        let mut journal = session.load_journal().expect("load journal");
        journal.global_epoch += 1;
        armed.store(true, Ordering::SeqCst);

        assert_eq!(session.persist_intent(&journal), Ok(()));
        let published = super::decode_authority_journal(
            &std::fs::read(held.join("state.json")).expect("read held journal"),
        )
        .expect("held journal decodes");
        assert_eq!(published.journal, journal);
        // Nothing was latched: the session still writes.
        session
            .replace_active(&set_id, remnant_record(&set_id, &operation_id))
            .expect("a verified publication leaves the session healthy");

        drop(session);
        drop(store);
        super::super::authority_journal::tests::remove_ancestor_swap_fixture(&base);
    }

    fn lose_both_authority_halves(
        keyring: &MemoryKeyringBoundary,
        journal: &MemoryJournalBoundary,
    ) {
        journal.bytes.lock().expect("memory journal lock").take();
        keyring
            .entries
            .lock()
            .expect("memory keyring lock")
            .retain(|(account, _)| account != "v2/_authority");
    }

    fn remnant_record(
        set_id: &CredentialSetId,
        operation_id: &CredentialOperationId,
    ) -> EncodedCredentialRecord {
        CredentialRecordEnvelope::present(
            set_id.clone(),
            AuthMethodId::ApiKey,
            CredentialRevision::parse("61616161-6161-4161-8161-616161616161")
                .expect("canonical remnant revision"),
            operation_id.clone(),
            StoredSecretBundle::api_key("remnant-canary").expect("API key"),
        )
        .expect("valid remnant record")
        .encode()
        .expect("encode remnant record")
    }

    /// Losing both halves of the pair erases the authority itself, but a
    /// native record that survived proves there was one. Reopening must not
    /// offer first initialization over it.
    #[test]
    fn reopen_with_both_halves_missing_and_a_surviving_record_requires_recovery() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let gate = NativeInteractionGate::isolated_for_test();
        let store = ready_store_with_gate(keyring.clone(), journal.clone(), gate);
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let operation_id = CredentialOperationId::parse("62626262-6262-4262-8262-626262626262")
            .expect("canonical operation id");
        let mut session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin mutation");
        session.load_journal().expect("load journal");
        session
            .replace_active(&set_id, remnant_record(&set_id, &operation_id))
            .expect("write active record");
        session
            .readback_active(&set_id)
            .expect("read back active record")
            .expect("active record exists");
        drop(session);
        drop(store);
        lose_both_authority_halves(&keyring, &journal);

        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries_and_gate(keyring, journal, gate),
            Ok(NativeCredentialStoreOpen::RecoveryRequired)
        ));
    }

    /// A staging record is named by its operation, which only the lost journal
    /// knew, so nothing can find it by name. Its set's staging guard still
    /// proves an authority existed.
    #[test]
    fn a_staging_only_remnant_blocks_first_initialization() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let gate = NativeInteractionGate::isolated_for_test();
        let store = ready_store_with_gate(keyring.clone(), journal.clone(), gate);
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let operation_id = CredentialOperationId::parse("63636363-6363-4363-8363-636363636363")
            .expect("canonical operation id");
        let mut session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin mutation");
        session.load_journal().expect("load journal");
        session
            .write_staging(
                &operation_id,
                &set_id,
                remnant_record(&set_id, &operation_id),
            )
            .expect("write staging record");
        session
            .read_staging(&operation_id, &set_id)
            .expect("read back staging record")
            .expect("staging record exists");
        drop(session);
        drop(store);
        lose_both_authority_halves(&keyring, &journal);

        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries_and_gate(
                keyring.clone(),
                journal.clone(),
                gate,
            ),
            Ok(NativeCredentialStoreOpen::RecoveryRequired)
        ));
    }

    /// A remnant that appears after a store opened fresh still stops its
    /// first initialization, because initialization re-checks the evidence
    /// under the lock.
    #[test]
    fn initialization_rechecks_the_evidence_of_a_fresh_authority_under_the_lock() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let gate = NativeInteractionGate::isolated_for_test();
        let fresh = match NativeKeyringCredentialStore::open_with_boundaries_and_gate(
            keyring.clone(),
            journal.clone(),
            gate,
        )
        .expect("open absent authority")
        {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        keyring.entries.lock().expect("memory keyring lock").push((
            EntryLocator::staging_guard(&set_id).account().to_owned(),
            b"guard".to_vec(),
        ));

        assert!(matches!(
            fresh.initialize(AuthorityJournal::new(CredentialBackendKind::Native)),
            Err(CredentialStoreFailure::CorruptRecord)
        ));
        assert!(journal.bytes.lock().expect("memory journal lock").is_none());
    }

    /// Every session operation other than `load_journal` needs the binding
    /// that `load_journal` validates, so nothing is read or written for an
    /// authority the session has not checked.
    #[test]
    fn a_session_refuses_entry_access_until_its_journal_is_loaded() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let store = ready_store_with_gate(
            keyring.clone(),
            journal.clone(),
            NativeInteractionGate::isolated_for_test(),
        );
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let operation_id = CredentialOperationId::parse("64646464-6464-4464-8464-646464646464")
            .expect("canonical operation id");
        let native_reads = keyring.reads.load(Ordering::SeqCst);
        let native_writes = keyring.writes.load(Ordering::SeqCst);
        let mut session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin mutation");

        assert!(matches!(
            session.read_active(&set_id),
            Err(CredentialStoreFailure::Internal)
        ));
        assert!(matches!(
            session.replace_active(&set_id, remnant_record(&set_id, &operation_id)),
            Err(CredentialStoreFailure::Internal)
        ));
        assert!(matches!(
            session.readback_active(&set_id),
            Err(CredentialStoreFailure::Internal)
        ));
        assert!(matches!(
            session.write_staging(
                &operation_id,
                &set_id,
                remnant_record(&set_id, &operation_id)
            ),
            Err(CredentialStoreFailure::Internal)
        ));
        assert!(matches!(
            session.read_staging(&operation_id, &set_id),
            Err(CredentialStoreFailure::Internal)
        ));
        assert!(matches!(
            session.delete_staging(&operation_id, &set_id),
            Err(CredentialStoreFailure::Internal)
        ));
        assert!(matches!(
            session.persist_intent(&AuthorityJournal::new(CredentialBackendKind::Native)),
            Err(CredentialStoreFailure::Internal)
        ));
        assert_eq!(keyring.reads.load(Ordering::SeqCst), native_reads);
        assert_eq!(keyring.writes.load(Ordering::SeqCst), native_writes);

        session.load_journal().expect("load journal");
        assert!(matches!(session.read_active(&set_id), Ok(None)));
    }

    #[test]
    fn portable_ceiling_accepts_2560_and_rejects_2561_before_native_invocation() {
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let revision = CredentialRevision::parse("77777777-7777-7777-7777-777777777777")
            .expect("canonical revision");
        let operation_id = CredentialOperationId::parse("88888888-8888-8888-8888-888888888888")
            .expect("canonical operation id");
        let mut largest_fitting_secret = None;
        for secret_length in 1..=PORTABLE_ENCODED_RECORD_MAX_BYTES + 1 {
            let record = CredentialRecordEnvelope::present(
                set_id.clone(),
                AuthMethodId::ApiKey,
                revision.clone(),
                operation_id.clone(),
                StoredSecretBundle::api_key("x".repeat(secret_length)).expect("API key"),
            )
            .expect("valid record shape");
            match record.encode() {
                Ok(encoded) => {
                    if encoded.as_bytes().len() == PORTABLE_ENCODED_RECORD_MAX_BYTES {
                        largest_fitting_secret = Some(secret_length);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let largest_fitting_secret =
            largest_fitting_secret.expect("one exact 2,560-byte record exists");

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let (store, journal) = match initialized {
            NativeCredentialStoreOpen::Ready { store, journal } => (store, *journal),
            _ => panic!("expected ready authority"),
        };
        let service = CredentialService::new(
            Arc::new(store),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("x".repeat(largest_fitting_secret))
                    .expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "12121212-3434-5656-7878-909090909090",
                )
                .expect("canonical idempotency token"),
            })
            .expect("exact portable ceiling");
        let writes_after_exact_ceiling = keyring.writes.load(Ordering::SeqCst);
        let journal_writes_after_exact_ceiling = journal_boundary.writes.load(Ordering::SeqCst);

        let error = service
            .replace_set(ReplaceCredentialSet {
                set_id,
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("x".repeat(largest_fitting_secret + 1))
                    .expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "23232323-4545-6767-8989-101010101010",
                )
                .expect("canonical idempotency token"),
            })
            .expect_err("2,561-byte encoded record is rejected");

        assert_eq!(error.code, CredentialErrorCode::PayloadTooLarge);
        assert_eq!(
            keyring.writes.load(Ordering::SeqCst),
            writes_after_exact_ceiling
        );
        assert_eq!(
            journal_boundary.writes.load(Ordering::SeqCst),
            journal_writes_after_exact_ceiling
        );
    }

    #[test]
    fn active_delete_replaces_the_entry_with_a_tombstone_without_native_deletion() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened =
            NativeKeyringCredentialStore::open_with_boundaries(keyring.clone(), journal_boundary)
                .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let (store, journal) = match initialized {
            NativeCredentialStoreOpen::Ready { store, journal } => (store, *journal),
            _ => panic!("expected ready authority"),
        };
        let service = CredentialService::new(
            Arc::new(store),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let created = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("delete-native-canary").expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "34343434-5656-7878-9090-121212121212",
                )
                .expect("canonical idempotency token"),
            })
            .expect("create active entry");

        service
            .delete_set(DeleteCredentialSet {
                set_id: set_id.clone(),
                expected_revision: created.new_revision,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "45454545-6767-8989-1010-232323232323",
                )
                .expect("canonical idempotency token"),
            })
            .expect("tombstone active entry");

        let active_bytes = keyring
            .entries
            .lock()
            .expect("memory keyring lock")
            .iter()
            .find(|(account, _)| account == "v2/deepgram")
            .expect("retained active entry")
            .1
            .clone();
        let active = CredentialRecordEnvelope::decode(
            &EncodedCredentialRecord::from_boundary_bytes(active_bytes),
        )
        .expect("decode active tombstone");
        assert!(active.is_tombstone());
        assert_eq!(keyring.deletes.load(Ordering::SeqCst), 0);
    }

    fn plant(keyring: &MemoryKeyringBoundary, account: &str, bytes: &[u8]) {
        let mut entries = keyring.entries.lock().expect("memory keyring lock");
        entries.retain(|(candidate, _)| candidate != account);
        entries.push((account.to_owned(), bytes.to_vec()));
    }

    /// Stages for `operation` in a session of its own and reads the staging
    /// record back, as an activation does; `Err` when staging is refused.
    fn try_stage(
        store: &NativeKeyringCredentialStore,
        operation: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<(), CredentialStoreFailure> {
        let mut session = store.begin_mutation(operation, set_id)?;
        session.load_journal()?;
        session.write_staging(operation, set_id, remnant_record(set_id, operation))?;
        session.read_staging(operation, set_id).map(|_| ())
    }

    /// Every built-in set is reported exactly once, with the expected outcome
    /// or `Clean`.
    fn repaired_outcomes(
        sets: &[UntrackedRecordRepair],
        expected: &[(BuiltInCredentialSetId, UntrackedRecordOutcome)],
    ) {
        assert_eq!(sets.len(), BUILT_IN_CREDENTIAL_SET_IDS.len());
        for built_in in BUILT_IN_CREDENTIAL_SET_IDS {
            let set_id = CredentialSetId::from(*built_in);
            let mut reported = sets.iter().filter(|repaired| repaired.set_id == set_id);
            let repaired = reported.next().expect("every built-in set is reported");
            assert!(reported.next().is_none(), "{set_id:?} is reported once");
            let outcome = expected
                .iter()
                .find(|(expected_set, _)| expected_set == built_in)
                .map_or(UntrackedRecordOutcome::Clean, |(_, outcome)| *outcome);
            assert_eq!(repaired.outcome, outcome, "{set_id:?}");
        }
    }

    /// A ready native store over `keyring` and `journal`, and a service over
    /// it whose settings port lets activations be recovered.
    fn activation_service(
        keyring: &Arc<MemoryKeyringBoundary>,
        journal: &Arc<MemoryJournalBoundary>,
    ) -> (Arc<NativeKeyringCredentialStore>, CredentialService) {
        let opened =
            NativeKeyringCredentialStore::open_with_boundaries(keyring.clone(), journal.clone())
                .expect("open absent authority");
        let NativeCredentialStoreOpen::Uninitialized(store) = opened else {
            panic!("expected uninitialized authority");
        };
        let NativeCredentialStoreOpen::Ready { store, journal } = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority")
        else {
            panic!("expected ready authority");
        };
        let store = Arc::new(store);
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            *journal,
            Arc::new(DeterministicTokenSource::default()),
            Arc::new(FakeSettingsActivationPort::new(1)),
        );
        (store, service)
    }

    fn deepgram_activation(token: &str) -> PrepareCredentialActivation {
        PrepareCredentialActivation {
            set_id: BuiltInCredentialSetId::Deepgram.into(),
            auth_method_id: AuthMethodId::ApiKey,
            material: StoredSecretBundle::api_key("guarded-activation-canary").expect("API key"),
            expected_revision: None,
            expected_settings_revision: 1,
            proposed_settings_revision: 2,
            idempotency_token: CredentialIdempotencyToken::parse(token)
                .expect("canonical idempotency token"),
        }
    }

    /// Through the service: the journal records an activation before its
    /// staging, so an orphan's guard refuses an activation that stays
    /// pending. The repair removes the orphan and keeps the refused
    /// activation tracked; activations stay in progress until that one is
    /// recovered, which rolls it back, and then the next one stages.
    #[test]
    fn an_activation_an_orphan_guard_refused_is_recovered_after_the_explicit_repair() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let (store, service) =
            activation_service(&keyring, &Arc::new(MemoryJournalBoundary::default()));
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let orphan = stage_and_leave(&store, "75757575-7575-4575-8575-757575757575", &set_id);

        let refused = service
            .prepare_settings_activation(deepgram_activation(
                "76767676-7676-4676-8676-767676767676",
            ))
            .expect_err("the orphan's guard refuses staging");
        assert_eq!(refused.code, CredentialErrorCode::CorruptRecord);
        let pending = service
            .snapshot_status()
            .pending_activation
            .expect("the journal records the refused activation");

        let sets = service
            .repair_untracked_records()
            .expect("repair untracked records");
        repaired_outcomes(
            &sets,
            &[(
                BuiltInCredentialSetId::Deepgram,
                UntrackedRecordOutcome::Removed,
            )],
        );
        assert_eq!(
            native_entry(&keyring, EntryLocator::staging(&orphan, &set_id).account()),
            None
        );
        assert_eq!(
            service
                .prepare_settings_activation(deepgram_activation(
                    "77777777-7777-4777-8777-777777777777",
                ))
                .expect_err("the refused activation is still pending")
                .code,
            CredentialErrorCode::OperationInProgress
        );

        let receipt = service
            .recover_settings_activation(&pending.operation_id)
            .expect("recover the refused activation");
        assert_eq!(receipt.result_code, CredentialMutationResultCode::NoChange);
        assert!(service.snapshot_status().pending_activation.is_none());
        service
            .prepare_settings_activation(deepgram_activation(
                "78787878-7878-4878-8878-787878787878",
            ))
            .expect("the next activation stages");
    }

    /// A guard in the pre-fix format may stand for the staging of the set's
    /// pending activation, so the repair keeps it while that activation is
    /// pending and removes it once the activation is recovered.
    #[test]
    fn a_pre_fix_guard_stays_while_its_set_has_a_pending_activation() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let (_store, service) =
            activation_service(&keyring, &Arc::new(MemoryJournalBoundary::default()));
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let guard = EntryLocator::staging_guard(&set_id).account().to_owned();
        plant(&keyring, &guard, super::LEGACY_STAGING_GUARD);
        service
            .prepare_settings_activation(deepgram_activation(
                "79797979-7979-4979-8979-797979797979",
            ))
            .expect_err("the pre-fix guard refuses staging");
        let pending = service
            .snapshot_status()
            .pending_activation
            .expect("the journal records the refused activation");

        let sets = service
            .repair_untracked_records()
            .expect("repair untracked records");
        repaired_outcomes(
            &sets,
            &[(
                BuiltInCredentialSetId::Deepgram,
                UntrackedRecordOutcome::Tracked,
            )],
        );
        assert_eq!(
            native_entry(&keyring, &guard).as_deref(),
            Some(super::LEGACY_STAGING_GUARD)
        );

        service
            .recover_settings_activation(&pending.operation_id)
            .expect("recover the refused activation");
        let sets = service
            .repair_untracked_records()
            .expect("repair untracked records");
        repaired_outcomes(
            &sets,
            &[(
                BuiltInCredentialSetId::Deepgram,
                UntrackedRecordOutcome::LegacyGuardRemoved,
            )],
        );
        assert_eq!(native_entry(&keyring, &guard), None);
    }

    /// A journal restored at the same epoch can omit an activation this
    /// service observed pending; its staging survives such a restore. The
    /// service keeps remembering that activation until it is reconciled,
    /// so every repair keeps that staging record and the guard naming it and
    /// reports the set as tracked: a second repair, and one after an
    /// ordinary commit on another set, as much as the first.
    #[test]
    fn the_repair_keeps_the_staging_of_an_activation_the_service_observed() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let (_store, service) = activation_service(&keyring, &journal);
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let before = journal.bytes.lock().expect("memory journal lock").clone();
        service
            .prepare_settings_activation(deepgram_activation(
                "7a7a7a7a-7a7a-4a7a-8a7a-7a7a7a7a7a7a",
            ))
            .expect("prepare an activation");
        let pending = service
            .snapshot_status()
            .pending_activation
            .expect("the service observed the activation");
        *journal.bytes.lock().expect("memory journal lock") = before;
        let staging = EntryLocator::staging(&pending.operation_id, &set_id)
            .account()
            .to_owned();
        let guard = EntryLocator::staging_guard(&set_id).account().to_owned();
        let repair_keeps_the_staging = |when: &str| {
            let sets = service
                .repair_untracked_records()
                .expect("repair untracked records");
            repaired_outcomes(
                &sets,
                &[(
                    BuiltInCredentialSetId::Deepgram,
                    UntrackedRecordOutcome::Tracked,
                )],
            );
            assert!(native_entry(&keyring, &staging).is_some(), "{when}");
            assert!(native_entry(&keyring, &guard).is_some(), "{when}");
        };

        repair_keeps_the_staging("first repair");
        repair_keeps_the_staging("second repair");
        service
            .replace_set(ReplaceCredentialSet {
                set_id: BuiltInCredentialSetId::Openai.into(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("other-set-canary").expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "7c7c7c7c-7c7c-4c7c-8c7c-7c7c7c7c7c7c",
                )
                .expect("canonical idempotency token"),
            })
            .expect("commit on another set");
        repair_keeps_the_staging("repair after an ordinary commit");
    }

    /// An activation stays remembered when another one is observed after
    /// it: activation A is observed, a same-epoch restore drops it from the
    /// journal, and activation B is then prepared on another set. Neither
    /// overwrites the other, so every repair keeps both stagings and A's
    /// guard, and reports both sets as tracked.
    #[test]
    fn every_observed_activation_keeps_its_staging_after_another_is_prepared() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let (_store, service) = activation_service(&keyring, &journal);
        let deepgram = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let openai = CredentialSetId::from(BuiltInCredentialSetId::Openai);
        let before = journal.bytes.lock().expect("memory journal lock").clone();
        service
            .prepare_settings_activation(deepgram_activation(
                "7d7d7d7d-7d7d-4d7d-8d7d-7d7d7d7d7d7d",
            ))
            .expect("prepare activation A");
        let first = service
            .snapshot_status()
            .pending_activation
            .expect("the service observed A")
            .operation_id;
        *journal.bytes.lock().expect("memory journal lock") = before;
        service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: openai.clone(),
                ..deepgram_activation("7e7e7e7e-7e7e-4e7e-8e7e-7e7e7e7e7e7e")
            })
            .expect("prepare activation B on another set");
        let second = service
            .snapshot_status()
            .pending_activation
            .expect("the service observed B")
            .operation_id;
        let kept = [
            EntryLocator::staging(&first, &deepgram),
            EntryLocator::staging_guard(&deepgram),
            EntryLocator::staging(&second, &openai),
        ]
        .map(|locator| locator.account().to_owned());

        for repair in ["first repair", "second repair"] {
            let sets = service
                .repair_untracked_records()
                .expect("repair untracked records");
            repaired_outcomes(
                &sets,
                &[
                    (
                        BuiltInCredentialSetId::Deepgram,
                        UntrackedRecordOutcome::Tracked,
                    ),
                    (
                        BuiltInCredentialSetId::Openai,
                        UntrackedRecordOutcome::Tracked,
                    ),
                ],
            );
            for account in &kept {
                assert!(native_entry(&keyring, account).is_some(), "{repair}");
            }
        }
    }

    /// The Windows power-loss case: the journal publication that recorded an
    /// activation was lost while its guard and staging record survived. No
    /// journal tracks that operation, yet its guard refuses every later
    /// staging for the set. Ordinary operations leave it in place; the
    /// explicit repair removes it, for that set only, and reports nothing
    /// but closed outcomes.
    #[test]
    fn an_untracked_staging_record_blocks_staging_until_the_explicit_repair_removes_it() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let store = ready_store_with_gate(
            keyring.clone(),
            Arc::new(MemoryJournalBoundary::default()),
            NativeInteractionGate::isolated_for_test(),
        );
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let orphan = stage_and_leave(&store, "71717171-7171-4171-8171-717171717171", &set_id);
        let next = CredentialOperationId::parse("72727272-7272-4272-8272-727272727272")
            .expect("canonical operation id");
        let orphan_account = EntryLocator::staging(&orphan, &set_id).account().to_owned();

        for _ in 0..2 {
            assert_eq!(
                try_stage(&store, &next, &set_id),
                Err(CredentialStoreFailure::CorruptRecord)
            );
        }
        assert!(native_entry(&keyring, &orphan_account).is_some());

        let sets = store
            .repair_untracked_records(&[], &|_| true)
            .expect("repair untracked records")
            .expect("the journal is admitted");
        repaired_outcomes(
            &sets,
            &[(
                BuiltInCredentialSetId::Deepgram,
                UntrackedRecordOutcome::Removed,
            )],
        );
        let report = format!("{sets:?}");
        assert!(!report.contains(orphan.as_str()) && !report.contains("remnant-canary"));
        assert_eq!(native_entry(&keyring, &orphan_account), None);
        assert_eq!(
            native_entry(&keyring, EntryLocator::staging_guard(&set_id).account()),
            None
        );
        assert_eq!(try_stage(&store, &next, &set_id), Ok(()));
    }

    /// A guard in the format written before guards named their operation
    /// cannot be claimed, so staging for its set is refused. Through the
    /// service, the explicit repair removes it, with the staging record the
    /// journal's history names for the set, and staging works again. The
    /// report says a legacy guard went: a staging record of an operation the
    /// history does not record could not be found.
    #[test]
    fn a_pre_fix_guard_is_removed_by_the_explicit_repair_with_the_staging_its_history_names() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            Arc::new(MemoryJournalBoundary::default()),
        )
        .expect("open absent authority");
        let NativeCredentialStoreOpen::Uninitialized(store) = opened else {
            panic!("expected uninitialized authority");
        };
        let NativeCredentialStoreOpen::Ready { store, journal } = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority")
        else {
            panic!("expected ready authority");
        };
        let store = Arc::new(store);
        let service = CredentialService::new(
            store.clone(),
            *journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let receipt = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("history-canary").expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "73737373-7373-4373-8373-737373737373",
                )
                .expect("canonical idempotency token"),
            })
            .expect("replace credential set");
        let pre_guard_staging = EntryLocator::staging(&receipt.operation_id, &set_id)
            .account()
            .to_owned();
        plant(
            &keyring,
            &pre_guard_staging,
            remnant_record(&set_id, &receipt.operation_id).as_bytes(),
        );
        plant(
            &keyring,
            EntryLocator::staging_guard(&set_id).account(),
            super::LEGACY_STAGING_GUARD,
        );
        let next = CredentialOperationId::parse("74747474-7474-4474-8474-747474747474")
            .expect("canonical operation id");
        assert_eq!(
            try_stage(&store, &next, &set_id),
            Err(CredentialStoreFailure::CorruptRecord)
        );

        let sets = service
            .repair_untracked_records()
            .expect("repair untracked records");

        repaired_outcomes(
            &sets,
            &[(
                BuiltInCredentialSetId::Deepgram,
                UntrackedRecordOutcome::LegacyGuardRemoved,
            )],
        );
        assert_eq!(native_entry(&keyring, &pre_guard_staging), None);
        assert_eq!(
            native_entry(&keyring, EntryLocator::staging_guard(&set_id).account()),
            None
        );
        assert!(native_entry(&keyring, "v2/deepgram").is_some());
        assert_eq!(try_stage(&store, &next, &set_id), Ok(()));
    }

    /// The repair never touches the staging record of the journal's pending
    /// activation or the guard naming it, leaves a guard it cannot read in
    /// place, and touches nothing at all when the caller refuses the journal
    /// it read under the lock.
    #[test]
    fn repair_spares_tracked_staging_unreadable_guards_and_refused_journals() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            Arc::new(MemoryJournalBoundary::default()),
        )
        .expect("open absent authority");
        let NativeCredentialStoreOpen::Uninitialized(store) = opened else {
            panic!("expected uninitialized authority");
        };
        let NativeCredentialStoreOpen::Ready { store, journal } = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority")
        else {
            panic!("expected ready authority");
        };
        let store = Arc::new(store);
        let service = CredentialService::new(
            store.clone(),
            *journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::from(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("tracked-canary").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 1,
                proposed_settings_revision: 2,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "75757575-7575-4575-8575-757575757575",
                )
                .expect("canonical idempotency token"),
            })
            .expect("prepare activation");
        let openai = CredentialSetId::from(BuiltInCredentialSetId::Openai);
        plant(
            &keyring,
            EntryLocator::staging_guard(&openai).account(),
            b"not-a-guard",
        );
        let snapshot = || keyring.entries.lock().expect("memory keyring lock").clone();
        let before = snapshot();

        let refused = store
            .repair_untracked_records(&[], &|_| false)
            .expect("a refused journal is not a failure");
        assert!(refused.is_none());
        assert_eq!(snapshot(), before);

        let sets = service
            .repair_untracked_records()
            .expect("repair untracked records");
        repaired_outcomes(
            &sets,
            &[
                (
                    BuiltInCredentialSetId::Deepgram,
                    UntrackedRecordOutcome::Tracked,
                ),
                (
                    BuiltInCredentialSetId::Openai,
                    UntrackedRecordOutcome::Unrecognized,
                ),
            ],
        );
        assert_eq!(snapshot(), before);
        assert!(
            before
                .iter()
                .any(|(account, _)| account.starts_with("v2-staging/")
                    && account.ends_with("/deepgram")),
            "the pending activation staged nothing"
        );
    }

    /// Another program holding the journal open no longer stalls the store.
    /// A reader that shares delete does not block a replacement at all. One
    /// that refuses to share delete makes the replacement a definite store
    /// failure before any native write, and nothing is latched: once it lets
    /// go, the same service replaces without a restart. Only the scratch
    /// journal directory and the in-memory keyring are touched.
    #[cfg(windows)]
    #[test]
    fn windows_a_journal_another_program_holds_open_never_latches_the_store() {
        use ::windows::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        use std::os::windows::fs::OpenOptionsExt;

        let root = std::env::temp_dir().join(format!(
            "audio-graph-521-open-target-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(
            FileAuthorityJournalBoundary::open_for_test(
                root.join("journal"),
                root.join("namespace"),
            )
            .expect("open journal boundary"),
        );
        let NativeCredentialStoreOpen::Uninitialized(store) =
            NativeKeyringCredentialStore::open_with_boundaries_and_gate(
                keyring.clone(),
                journal_boundary.clone(),
                NativeInteractionGate::isolated_for_test(),
            )
            .expect("open absent authority")
        else {
            panic!("expected uninitialized authority");
        };
        let NativeCredentialStoreOpen::Ready { store, journal } = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority")
        else {
            panic!("expected ready authority");
        };
        let service = CredentialService::new(
            Arc::new(store),
            *journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let replace = |expected_revision: Option<CredentialRevision>, token: &str| {
            service.replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("open-target-canary").expect("API key"),
                expected_revision,
                idempotency_token: CredentialIdempotencyToken::parse(token)
                    .expect("canonical idempotency token"),
            })
        };
        let open_journal = |share: u32| {
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(share)
                .open(root.join("journal").join("state.json"))
                .expect("open the journal as another program")
        };

        let sharing = open_journal(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
        let first = replace(None, "76767676-7676-4676-8676-767676767676")
            .expect("a reader that shares delete does not block");
        drop(sharing);

        let refusing = open_journal(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0);
        let writes = keyring.writes.load(Ordering::SeqCst);
        let refused = replace(
            first.new_revision.clone(),
            "77777777-7777-4777-8777-777777777777",
        )
        .expect_err("a reader that refuses to share delete blocks the intent");
        assert_eq!(refused.code, CredentialErrorCode::OperationInProgress);
        assert!(refused.retryable);
        assert_eq!(keyring.writes.load(Ordering::SeqCst), writes);
        assert_eq!(
            service.snapshot_status().global_epoch,
            1,
            "the refused intent changed the observed journal"
        );
        drop(refusing);

        let second = replace(
            first.new_revision.clone(),
            "77777777-7777-4777-8777-777777777777",
        )
        .expect("the same service replaces once the reader lets go");
        assert_eq!(second.previous_revision, first.new_revision);
        assert_eq!(service.snapshot_status().global_epoch, 2);

        drop(service);
        drop(journal_boundary);
        std::fs::remove_dir_all(&root).expect("remove isolated open-target directory");
    }

    /// The same refusals through the runtime. A program holding the journal
    /// open without sharing delete refuses the intent's rename, and one that
    /// does not share reading refuses the journal read before it, both
    /// before any native write. Each time the runtime stays ready with no
    /// cached closure, and the same request succeeds once that program lets
    /// go. Only the scratch journal directory and the in-memory keyring are
    /// touched.
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_a_runtime_stays_ready_while_another_program_holds_the_journal_open() {
        use crate::credentials::runtime::{
            CredentialEventSinkFailure, CredentialReplaceDraft, CredentialRuntime,
            CredentialRuntimeBackend, CredentialRuntimeEventSink, CredentialRuntimeLifecycle,
            CredentialRuntimeOpen, CredentialRuntimeRecovery,
        };
        use crate::credentials::service::{
            CredentialChangeEvent, CredentialTokenSource, StoreOwnershipRepair,
        };
        use ::windows::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        use std::os::windows::fs::OpenOptionsExt;
        use std::time::Duration;

        /// Opens the native store over the shared keyring and journal
        /// boundary, as the adapter assembly will.
        struct FileJournalBackend {
            keyring: Arc<MemoryKeyringBoundary>,
            journal: Arc<FileAuthorityJournalBoundary>,
            gate: &'static NativeInteractionGate,
        }

        impl FileJournalBackend {
            fn ready(
                opened: NativeCredentialStoreOpen,
                token_source: Arc<dyn CredentialTokenSource>,
            ) -> CredentialRuntimeOpen {
                match opened {
                    NativeCredentialStoreOpen::Ready { store, journal } => {
                        CredentialRuntimeOpen::Ready(Box::new(CredentialService::new(
                            Arc::new(store),
                            *journal,
                            token_source,
                        )))
                    }
                    NativeCredentialStoreOpen::Uninitialized(_) => {
                        CredentialRuntimeOpen::Uninitialized
                    }
                    NativeCredentialStoreOpen::RecoveryRequired => {
                        CredentialRuntimeOpen::RecoveryRequired
                    }
                }
            }

            fn open_store(&self) -> Result<NativeCredentialStoreOpen, CredentialStoreFailure> {
                NativeKeyringCredentialStore::open_with_boundaries_and_gate(
                    self.keyring.clone(),
                    self.journal.clone(),
                    self.gate,
                )
            }
        }

        impl CredentialRuntimeBackend for FileJournalBackend {
            fn open(
                &mut self,
                token_source: Arc<dyn CredentialTokenSource>,
            ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
                Ok(Self::ready(self.open_store()?, token_source))
            }

            fn initialize(
                &mut self,
                token_source: Arc<dyn CredentialTokenSource>,
            ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
                let NativeCredentialStoreOpen::Uninitialized(store) = self.open_store()? else {
                    return Err(CredentialStoreFailure::RevisionConflict);
                };
                Ok(Self::ready(
                    store.initialize(AuthorityJournal::new(CredentialBackendKind::Native))?,
                    token_source,
                ))
            }

            fn diagnose_or_unlock(
                &mut self,
            ) -> Result<CredentialRuntimeRecovery, CredentialStoreFailure> {
                Ok(CredentialRuntimeRecovery::Ready)
            }

            fn repair_ownership(&mut self) -> Result<StoreOwnershipRepair, CredentialStoreFailure> {
                Ok(StoreOwnershipRepair::Unchanged)
            }
        }

        struct DiscardingSink;

        impl CredentialRuntimeEventSink for DiscardingSink {
            fn emit(
                &self,
                _event: CredentialChangeEvent,
            ) -> Result<(), CredentialEventSinkFailure> {
                Ok(())
            }
        }

        let root = std::env::temp_dir().join(format!(
            "audio-graph-521-runtime-open-target-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(
            FileAuthorityJournalBoundary::open_for_test(
                root.join("journal"),
                root.join("namespace"),
            )
            .expect("open journal boundary"),
        );
        let runtime = CredentialRuntime::dormant(CredentialBackendKind::Native);
        runtime
            .configure(
                Box::new(FileJournalBackend {
                    keyring: keyring.clone(),
                    journal: journal.clone(),
                    gate: NativeInteractionGate::isolated_for_test(),
                }),
                Arc::new(DiscardingSink),
            )
            .expect("configure runtime");
        runtime
            .initialize(Duration::from_secs(5))
            .await
            .expect("initialize the native authority");
        let draft = || {
            CredentialReplaceDraft::api_key(
                BuiltInCredentialSetId::Deepgram.into(),
                AuthMethodId::ApiKey,
                "runtime-open-target-canary".to_owned(),
                None,
                CredentialIdempotencyToken::parse("78787878-7878-4878-8878-787878787878")
                    .expect("canonical idempotency token"),
            )
        };

        for (share, refusal) in [
            (
                FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0,
                "a reader that refuses to share delete blocks the intent",
            ),
            (
                FILE_SHARE_WRITE.0,
                "a holder that refuses to share reading blocks the journal read",
            ),
        ] {
            let holding = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(share)
                .open(root.join("journal").join("state.json"))
                .expect("open the journal as another program");
            let writes = keyring.writes.load(Ordering::SeqCst);
            let refused = runtime
                .replace_set(draft(), Duration::from_secs(5))
                .await
                .expect_err(refusal);
            assert_eq!(
                refused.code,
                CredentialErrorCode::OperationInProgress,
                "{refusal}"
            );
            assert!(refused.retryable);
            assert_eq!(keyring.writes.load(Ordering::SeqCst), writes);
            assert_eq!(
                runtime.status().lifecycle,
                CredentialRuntimeLifecycle::Ready,
                "{refusal}"
            );
            drop(holding);
        }

        let receipt = runtime
            .replace_set(draft(), Duration::from_secs(5))
            .await
            .expect("the same request succeeds once the reader lets go");
        assert_eq!(receipt.previous_revision, None);
        assert_eq!(runtime.status().service.global_epoch, 1);

        drop(runtime);
        for _ in 0..500 {
            if Arc::strong_count(&journal) == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(journal);
        std::fs::remove_dir_all(&root).expect("remove isolated runtime open-target directory");
    }
}
