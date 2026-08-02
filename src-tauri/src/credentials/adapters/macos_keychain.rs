use super::{AllowPrompt, RecoveryBoundary, RecoveryFailure, RecoveryOutcome, RecoveryTarget};
use crate::credentials::adapters::keyring_entry::{EntryLocator, KeyringBoundary};
#[cfg(target_os = "macos")]
use crate::credentials::adapters::macos_keychain_ffi::{
    KeychainItem, SecurityFrameworkCore, UserKeychain,
};
use crate::credentials::adapters::macos_keychain_ffi::{KeychainStatus, NativeStatus};
use crate::credentials::adapters::native_interaction::{
    CancellationToken, ForbidPrompt, MutationInvocation, ensure_forbid_prompt,
    latch_forbid_uncertainty,
};
use crate::credentials::domain::CredentialStoreFailure;
use std::panic::{AssertUnwindSafe, catch_unwind};
use zeroize::Zeroizing;

trait MacKeychainApi: Send + Sync {
    type Keychain;
    type Item;

    fn interaction_allowed(&self) -> Result<bool, NativeStatus>;
    fn set_interaction_allowed(&self, allowed: bool) -> Result<(), NativeStatus>;
    fn default_user_keychain(&self) -> Result<Self::Keychain, NativeStatus>;
    fn read_secret(
        &self,
        keychain: &Self::Keychain,
        service: &str,
        account: &str,
    ) -> Result<Zeroizing<Vec<u8>>, NativeStatus>;
    fn find_item(
        &self,
        keychain: &Self::Keychain,
        service: &str,
        account: &str,
    ) -> Result<Self::Item, NativeStatus>;
    fn add_secret(
        &self,
        keychain: &Self::Keychain,
        service: &str,
        account: &str,
        secret: &[u8],
    ) -> Result<(), NativeStatus>;
    fn update_secret(&self, item: &Self::Item, secret: &[u8]) -> Result<(), NativeStatus>;
    fn delete_item(&self, item: &Self::Item) -> Result<(), NativeStatus>;
    fn unlock(&self, keychain: &Self::Keychain) -> Result<(), NativeStatus>;
    fn status(&self, keychain: &Self::Keychain) -> Result<KeychainStatus, NativeStatus>;
}

#[cfg(target_os = "macos")]
impl MacKeychainApi for SecurityFrameworkCore {
    type Keychain = UserKeychain;
    type Item = KeychainItem;

    fn interaction_allowed(&self) -> Result<bool, NativeStatus> {
        SecurityFrameworkCore::interaction_allowed(self)
    }

    fn set_interaction_allowed(&self, allowed: bool) -> Result<(), NativeStatus> {
        SecurityFrameworkCore::set_interaction_allowed(self, allowed)
    }

    fn default_user_keychain(&self) -> Result<Self::Keychain, NativeStatus> {
        SecurityFrameworkCore::default_user_keychain(self)
    }

    fn read_secret(
        &self,
        keychain: &Self::Keychain,
        service: &str,
        account: &str,
    ) -> Result<Zeroizing<Vec<u8>>, NativeStatus> {
        SecurityFrameworkCore::read_secret(self, keychain, service, account)
    }

    fn find_item(
        &self,
        keychain: &Self::Keychain,
        service: &str,
        account: &str,
    ) -> Result<Self::Item, NativeStatus> {
        SecurityFrameworkCore::find_item(self, keychain, service, account)
    }

    fn add_secret(
        &self,
        keychain: &Self::Keychain,
        service: &str,
        account: &str,
        secret: &[u8],
    ) -> Result<(), NativeStatus> {
        SecurityFrameworkCore::add_secret(self, keychain, service, account, secret)
    }

    fn update_secret(&self, item: &Self::Item, secret: &[u8]) -> Result<(), NativeStatus> {
        SecurityFrameworkCore::update_secret(self, item, secret)
    }

    fn delete_item(&self, item: &Self::Item) -> Result<(), NativeStatus> {
        SecurityFrameworkCore::delete_item(self, item)
    }

    fn unlock(&self, keychain: &Self::Keychain) -> Result<(), NativeStatus> {
        SecurityFrameworkCore::unlock(self, keychain)
    }

    fn status(&self, keychain: &Self::Keychain) -> Result<KeychainStatus, NativeStatus> {
        SecurityFrameworkCore::status(self, keychain)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OperationDisposition {
    NotRun,
    Succeeded,
    Failed,
}

enum GuardFailure<OperationFailure> {
    Control,
    Operation(OperationFailure),
    Restore { disposition: OperationDisposition },
}

struct ExactInteractionRestore<'api, 'prompt, 'gate, Api: MacKeychainApi + ?Sized> {
    api: &'api Api,
    prompt: &'prompt ForbidPrompt<'gate>,
    prior: bool,
    armed: bool,
}

impl<Api: MacKeychainApi + ?Sized> ExactInteractionRestore<'_, '_, '_, Api> {
    fn new<'api, 'prompt, 'gate>(
        api: &'api Api,
        prompt: &'prompt ForbidPrompt<'gate>,
        prior: bool,
    ) -> ExactInteractionRestore<'api, 'prompt, 'gate, Api> {
        ExactInteractionRestore {
            api,
            prompt,
            prior,
            armed: true,
        }
    }

    fn restore(&mut self) -> Result<(), ()> {
        let restored = catch_unwind(AssertUnwindSafe(|| {
            self.api.set_interaction_allowed(self.prior)
        }));
        self.armed = false;
        if matches!(restored, Ok(Ok(()))) {
            Ok(())
        } else {
            let _ = latch_forbid_uncertainty(self.prompt);
            Err(())
        }
    }
}

impl<Api: MacKeychainApi + ?Sized> Drop for ExactInteractionRestore<'_, '_, '_, Api> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.restore();
        }
    }
}

fn with_interaction_suppressed<Api, Value, OperationFailure>(
    api: &Api,
    prompt: &ForbidPrompt<'_>,
    operation: impl FnOnce(&Api) -> Result<Value, OperationFailure>,
) -> Result<Value, GuardFailure<OperationFailure>>
where
    Api: MacKeychainApi + ?Sized,
{
    let prior = api
        .interaction_allowed()
        .map_err(|_| GuardFailure::Control)?;
    let mut restoration = ExactInteractionRestore::new(api, prompt, prior);

    if api.set_interaction_allowed(false).is_err() {
        return if restoration.restore().is_ok() {
            Err(GuardFailure::Control)
        } else {
            Err(GuardFailure::Restore {
                disposition: OperationDisposition::NotRun,
            })
        };
    }

    let result = operation(api);
    let disposition = if result.is_ok() {
        OperationDisposition::Succeeded
    } else {
        OperationDisposition::Failed
    };
    if restoration.restore().is_err() {
        drop(result);
        return Err(GuardFailure::Restore { disposition });
    }
    result.map_err(GuardFailure::Operation)
}

fn known_credential_status(status: NativeStatus) -> Option<CredentialStoreFailure> {
    if [
        NativeStatus::INTERACTION_NOT_ALLOWED,
        NativeStatus::INTERACTION_REQUIRED,
    ]
    .contains(&status)
    {
        return Some(CredentialStoreFailure::Locked);
    }
    if [
        NativeStatus::WRITE_PERMISSION,
        NativeStatus::READ_ONLY,
        NativeStatus::AUTH_FAILED,
        NativeStatus::READ_ONLY_ATTRIBUTE,
        NativeStatus::DATA_NOT_MODIFIABLE,
        NativeStatus::MISSING_ENTITLEMENT,
        NativeStatus::RESTRICTED_API,
    ]
    .contains(&status)
    {
        return Some(CredentialStoreFailure::AccessDenied);
    }
    if [
        NativeStatus::NOT_AVAILABLE,
        NativeStatus::NO_SUCH_KEYCHAIN,
        NativeStatus::INVALID_KEYCHAIN,
        NativeStatus::NO_DEFAULT_KEYCHAIN,
        NativeStatus::NO_STORAGE_MODULE,
        NativeStatus::IN_DARK_WAKE,
        NativeStatus::SERVICE_NOT_AVAILABLE,
    ]
    .contains(&status)
    {
        return Some(CredentialStoreFailure::Unavailable);
    }
    if [
        NativeStatus::UNIMPLEMENTED,
        NativeStatus::WRONG_SECURITY_VERSION,
    ]
    .contains(&status)
    {
        return Some(CredentialStoreFailure::Unsupported);
    }
    None
}

fn map_open_status(status: NativeStatus) -> CredentialStoreFailure {
    known_credential_status(status).unwrap_or(CredentialStoreFailure::Internal)
}

fn map_read_status(status: NativeStatus) -> CredentialStoreFailure {
    if status == NativeStatus::ITEM_NOT_FOUND {
        CredentialStoreFailure::Missing
    } else if status == NativeStatus::DECODE {
        CredentialStoreFailure::CorruptRecord
    } else {
        known_credential_status(status).unwrap_or(CredentialStoreFailure::Internal)
    }
}

fn map_lookup_status(status: NativeStatus) -> CredentialStoreFailure {
    known_credential_status(status).unwrap_or(CredentialStoreFailure::Internal)
}

fn map_add_status(status: NativeStatus) -> CredentialStoreFailure {
    if status == NativeStatus::DATA_TOO_LARGE {
        CredentialStoreFailure::PayloadTooLarge
    } else if status == NativeStatus::DUPLICATE_ITEM {
        CredentialStoreFailure::RevisionConflict
    } else {
        known_credential_status(status).unwrap_or(CredentialStoreFailure::CommitUnknown)
    }
}

fn map_update_status(status: NativeStatus) -> CredentialStoreFailure {
    if status == NativeStatus::DATA_TOO_LARGE {
        CredentialStoreFailure::PayloadTooLarge
    } else {
        known_credential_status(status).unwrap_or(CredentialStoreFailure::CommitUnknown)
    }
}

fn map_delete_status(status: NativeStatus) -> CredentialStoreFailure {
    known_credential_status(status).unwrap_or(CredentialStoreFailure::CommitUnknown)
}

fn map_unlock_status(status: NativeStatus) -> Option<CredentialStoreFailure> {
    if status == NativeStatus::USER_CANCELLED {
        Some(CredentialStoreFailure::Cancelled)
    } else {
        known_credential_status(status)
    }
}

fn map_verify_status(status: NativeStatus) -> CredentialStoreFailure {
    known_credential_status(status).unwrap_or(CredentialStoreFailure::Internal)
}

struct MacOsKeychainBoundary<Api> {
    api: Api,
}

impl<Api> MacOsKeychainBoundary<Api> {
    fn new(api: Api) -> Self {
        Self { api }
    }
}

impl<Api: MacKeychainApi> KeyringBoundary for MacOsKeychainBoundary<Api> {
    fn get_secret(
        &self,
        prompt: &ForbidPrompt<'_>,
        locator: &EntryLocator,
    ) -> Result<Vec<u8>, CredentialStoreFailure> {
        ensure_forbid_prompt(prompt)?;
        match with_interaction_suppressed(&self.api, prompt, |api| {
            let keychain = api.default_user_keychain().map_err(map_open_status)?;
            api.read_secret(&keychain, locator.service(), locator.account())
                .map_err(map_read_status)
        }) {
            Ok(secret) => Ok(secret.as_slice().to_vec()),
            Err(GuardFailure::Control) => Err(CredentialStoreFailure::Internal),
            Err(GuardFailure::Operation(failure)) => Err(failure),
            Err(GuardFailure::Restore { disposition }) => {
                let _ = disposition;
                Err(latch_forbid_uncertainty(prompt))
            }
        }
    }

    fn set_secret(
        &self,
        prompt: &ForbidPrompt<'_>,
        invocation: &mut MutationInvocation<'_>,
        locator: &EntryLocator,
        secret: &[u8],
    ) -> Result<(), CredentialStoreFailure> {
        ensure_forbid_prompt(prompt)?;
        invocation.prepare()?;
        let result = with_interaction_suppressed(&self.api, prompt, |api| {
            let keychain = api.default_user_keychain().map_err(map_open_status)?;
            match api.find_item(&keychain, locator.service(), locator.account()) {
                Ok(item) => {
                    invocation.mark_started();
                    api.update_secret(&item, secret).map_err(map_update_status)
                }
                Err(status) if status == NativeStatus::ITEM_NOT_FOUND => {
                    invocation.mark_started();
                    api.add_secret(&keychain, locator.service(), locator.account(), secret)
                        .map_err(map_add_status)
                }
                Err(status) => Err(map_lookup_status(status)),
            }
        });
        match result {
            Ok(()) => Ok(()),
            Err(GuardFailure::Control) => Err(CredentialStoreFailure::Internal),
            Err(GuardFailure::Operation(failure)) => Err(failure),
            Err(GuardFailure::Restore { disposition }) => {
                let _ = disposition;
                Err(invocation.latch_uncertainty())
            }
        }
    }

    fn delete_credential(
        &self,
        prompt: &ForbidPrompt<'_>,
        invocation: &mut MutationInvocation<'_>,
        locator: &EntryLocator,
    ) -> Result<(), CredentialStoreFailure> {
        ensure_forbid_prompt(prompt)?;
        invocation.prepare()?;
        let result = with_interaction_suppressed(&self.api, prompt, |api| {
            let keychain = api.default_user_keychain().map_err(map_open_status)?;
            match api.find_item(&keychain, locator.service(), locator.account()) {
                Ok(item) => {
                    invocation.mark_started();
                    api.delete_item(&item).map_err(map_delete_status)
                }
                Err(status) if status == NativeStatus::ITEM_NOT_FOUND => {
                    Err(CredentialStoreFailure::Missing)
                }
                Err(status) => Err(map_lookup_status(status)),
            }
        });
        match result {
            Ok(()) => Ok(()),
            Err(GuardFailure::Control) => Err(CredentialStoreFailure::Internal),
            Err(GuardFailure::Operation(failure)) => Err(failure),
            Err(GuardFailure::Restore { disposition }) => {
                let _ = disposition;
                Err(invocation.latch_uncertainty())
            }
        }
    }
}

impl<Api: MacKeychainApi> RecoveryBoundary for MacOsKeychainBoundary<Api> {
    fn recover(
        &self,
        _prompt: &AllowPrompt<'_, '_>,
        _target: RecoveryTarget,
        cancellation: &CancellationToken,
    ) -> Result<(), RecoveryFailure> {
        if cancellation.is_cancelled() {
            return Err(RecoveryFailure::Closed(CredentialStoreFailure::Cancelled));
        }
        let keychain = self
            .api
            .default_user_keychain()
            .map_err(|status| RecoveryFailure::Closed(map_open_status(status)))?;
        if cancellation.is_cancelled() {
            return Err(RecoveryFailure::Closed(CredentialStoreFailure::Cancelled));
        }

        let unlock = self.api.unlock(&keychain);
        let cancelled_after_unlock = cancellation.is_cancelled();
        match unlock {
            Ok(()) if cancelled_after_unlock => {
                Err(RecoveryFailure::Closed(CredentialStoreFailure::Cancelled))
            }
            Ok(()) => Ok(()),
            Err(status) => match map_unlock_status(status) {
                None => Err(RecoveryFailure::Uncertain),
                Some(_) if cancelled_after_unlock => {
                    Err(RecoveryFailure::Closed(CredentialStoreFailure::Cancelled))
                }
                Some(failure) => Err(RecoveryFailure::Closed(failure)),
            },
        }
    }

    fn verify(
        &self,
        prompt: &ForbidPrompt<'_>,
        _target: RecoveryTarget,
    ) -> Result<RecoveryOutcome, RecoveryFailure> {
        ensure_forbid_prompt(prompt).map_err(RecoveryFailure::Closed)?;
        match with_interaction_suppressed(&self.api, prompt, |api| {
            let keychain = api
                .default_user_keychain()
                .map_err(|status| RecoveryFailure::Closed(map_open_status(status)))?;
            let status = api
                .status(&keychain)
                .map_err(|status| RecoveryFailure::Closed(map_verify_status(status)))?;
            if status.unlocked && status.readable && status.writable {
                Ok(RecoveryOutcome::Ready)
            } else {
                Ok(RecoveryOutcome::RecoveryRequired)
            }
        }) {
            Ok(outcome) => Ok(outcome),
            Err(GuardFailure::Control) => {
                Err(RecoveryFailure::Closed(CredentialStoreFailure::Internal))
            }
            Err(GuardFailure::Operation(failure)) => Err(failure),
            Err(GuardFailure::Restore { disposition }) => {
                let _ = disposition;
                Err(RecoveryFailure::Uncertain)
            }
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub(super) fn production_keyring_boundary() -> std::sync::Arc<dyn KeyringBoundary> {
    std::sync::Arc::new(MacOsKeychainBoundary::new(SecurityFrameworkCore::new()))
}

#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub(super) fn production_recovery_boundary() -> Box<dyn RecoveryBoundary> {
    Box::new(MacOsKeychainBoundary::new(SecurityFrameworkCore::new()))
}

#[cfg(test)]
mod tests {
    use super::super::NativeRecoveryFacade;
    use super::{MacKeychainApi, MacOsKeychainBoundary};
    use crate::credentials::adapters::keyring_entry::{
        EntryLocator, KeyringBoundary, KeyringEntryAdapter,
    };
    use crate::credentials::adapters::macos_keychain_ffi::{KeychainStatus, NativeStatus};
    use crate::credentials::adapters::native_interaction::{
        CancellationToken, NativeInteractionGate, RecoveryOutcome, RecoveryTarget,
    };
    use crate::credentials::domain::CredentialStoreFailure;
    use std::collections::VecDeque;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Arc, Mutex};
    use zeroize::Zeroizing;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Call {
        GetInteraction,
        SetInteraction(bool),
        Read,
        Find,
        Add,
        Update,
        Delete,
        Unlock,
        Status,
    }

    #[derive(Clone, Copy)]
    enum UnitStep {
        Success,
        Failure(NativeStatus),
        Panic,
    }

    #[derive(Clone, Copy)]
    enum BoolStep {
        Value(bool),
        Failure(NativeStatus),
        Panic,
    }

    enum ReadStep {
        Secret(Vec<u8>),
        Failure(NativeStatus),
        Panic,
    }

    #[derive(Clone, Copy)]
    enum FindStep {
        Found,
        Failure(NativeStatus),
        Panic,
    }

    #[derive(Clone, Copy)]
    enum StatusStep {
        Value(KeychainStatus),
        Failure(NativeStatus),
        Panic,
    }

    struct FakePlan {
        get: BoolStep,
        set: VecDeque<UnitStep>,
        open: VecDeque<UnitStep>,
        read: VecDeque<ReadStep>,
        find: VecDeque<FindStep>,
        add: VecDeque<UnitStep>,
        update: VecDeque<UnitStep>,
        delete: VecDeque<UnitStep>,
        unlock: VecDeque<UnitStep>,
        status: VecDeque<StatusStep>,
        cancel_on_open: Option<Arc<CancellationToken>>,
        cancel_on_unlock: Option<Arc<CancellationToken>>,
    }

    impl Default for FakePlan {
        fn default() -> Self {
            Self {
                get: BoolStep::Value(false),
                set: VecDeque::new(),
                open: VecDeque::new(),
                read: VecDeque::new(),
                find: VecDeque::new(),
                add: VecDeque::new(),
                update: VecDeque::new(),
                delete: VecDeque::new(),
                unlock: VecDeque::new(),
                status: VecDeque::new(),
                cancel_on_open: None,
                cancel_on_unlock: None,
            }
        }
    }

    #[derive(Clone)]
    struct Trace(Arc<Mutex<Vec<Call>>>);

    impl Trace {
        fn snapshot(&self) -> Vec<Call> {
            self.0.lock().expect("macOS fake trace").clone()
        }
    }

    struct ScriptedApi {
        trace: Trace,
        plan: Mutex<FakePlan>,
    }

    impl ScriptedApi {
        fn new(plan: FakePlan) -> (Self, Trace) {
            let trace = Trace(Arc::new(Mutex::new(Vec::new())));
            (
                Self {
                    trace: trace.clone(),
                    plan: Mutex::new(plan),
                },
                trace,
            )
        }

        fn record(&self, call: Call) {
            self.trace.0.lock().expect("macOS fake trace").push(call);
        }

        fn run_unit(step: UnitStep) -> Result<(), NativeStatus> {
            match step {
                UnitStep::Success => Ok(()),
                UnitStep::Failure(status) => Err(status),
                UnitStep::Panic => panic!("scripted macOS native panic"),
            }
        }

        fn next_unit(queue: &mut VecDeque<UnitStep>) -> UnitStep {
            queue.pop_front().unwrap_or(UnitStep::Success)
        }
    }

    impl MacKeychainApi for ScriptedApi {
        type Keychain = ();
        type Item = ();

        fn interaction_allowed(&self) -> Result<bool, NativeStatus> {
            self.record(Call::GetInteraction);
            let step = self.plan.lock().expect("macOS fake plan").get;
            match step {
                BoolStep::Value(value) => Ok(value),
                BoolStep::Failure(status) => Err(status),
                BoolStep::Panic => panic!("scripted interaction getter panic"),
            }
        }

        fn set_interaction_allowed(&self, allowed: bool) -> Result<(), NativeStatus> {
            self.record(Call::SetInteraction(allowed));
            let step = {
                let mut plan = self.plan.lock().expect("macOS fake plan");
                Self::next_unit(&mut plan.set)
            };
            Self::run_unit(step)
        }

        fn default_user_keychain(&self) -> Result<Self::Keychain, NativeStatus> {
            let (step, cancellation) = {
                let mut plan = self.plan.lock().expect("macOS fake plan");
                (Self::next_unit(&mut plan.open), plan.cancel_on_open.clone())
            };
            if let Some(cancellation) = cancellation {
                cancellation.cancel();
            }
            Self::run_unit(step)
        }

        fn read_secret(
            &self,
            _keychain: &Self::Keychain,
            _service: &str,
            _account: &str,
        ) -> Result<Zeroizing<Vec<u8>>, NativeStatus> {
            self.record(Call::Read);
            let step = self
                .plan
                .lock()
                .expect("macOS fake plan")
                .read
                .pop_front()
                .unwrap_or(ReadStep::Failure(NativeStatus::for_test(-1)));
            match step {
                ReadStep::Secret(secret) => Ok(Zeroizing::new(secret)),
                ReadStep::Failure(status) => Err(status),
                ReadStep::Panic => panic!("scripted macOS read panic"),
            }
        }

        fn find_item(
            &self,
            _keychain: &Self::Keychain,
            _service: &str,
            _account: &str,
        ) -> Result<Self::Item, NativeStatus> {
            self.record(Call::Find);
            let step = self
                .plan
                .lock()
                .expect("macOS fake plan")
                .find
                .pop_front()
                .unwrap_or(FindStep::Failure(NativeStatus::for_test(-1)));
            match step {
                FindStep::Found => Ok(()),
                FindStep::Failure(status) => Err(status),
                FindStep::Panic => panic!("scripted macOS find panic"),
            }
        }

        fn add_secret(
            &self,
            _keychain: &Self::Keychain,
            _service: &str,
            _account: &str,
            _secret: &[u8],
        ) -> Result<(), NativeStatus> {
            self.record(Call::Add);
            let step = {
                let mut plan = self.plan.lock().expect("macOS fake plan");
                Self::next_unit(&mut plan.add)
            };
            Self::run_unit(step)
        }

        fn update_secret(&self, _item: &Self::Item, _secret: &[u8]) -> Result<(), NativeStatus> {
            self.record(Call::Update);
            let step = {
                let mut plan = self.plan.lock().expect("macOS fake plan");
                Self::next_unit(&mut plan.update)
            };
            Self::run_unit(step)
        }

        fn delete_item(&self, _item: &Self::Item) -> Result<(), NativeStatus> {
            self.record(Call::Delete);
            let step = {
                let mut plan = self.plan.lock().expect("macOS fake plan");
                Self::next_unit(&mut plan.delete)
            };
            Self::run_unit(step)
        }

        fn unlock(&self, _keychain: &Self::Keychain) -> Result<(), NativeStatus> {
            self.record(Call::Unlock);
            let (step, cancellation) = {
                let mut plan = self.plan.lock().expect("macOS fake plan");
                (
                    Self::next_unit(&mut plan.unlock),
                    plan.cancel_on_unlock.clone(),
                )
            };
            let result = Self::run_unit(step);
            if let Some(cancellation) = cancellation {
                cancellation.cancel();
            }
            result
        }

        fn status(&self, _keychain: &Self::Keychain) -> Result<KeychainStatus, NativeStatus> {
            self.record(Call::Status);
            let step = self
                .plan
                .lock()
                .expect("macOS fake plan")
                .status
                .pop_front()
                .unwrap_or(StatusStep::Value(KeychainStatus {
                    unlocked: true,
                    readable: true,
                    writable: true,
                }));
            match step {
                StatusStep::Value(status) => Ok(status),
                StatusStep::Failure(status) => Err(status),
                StatusStep::Panic => panic!("scripted macOS status panic"),
            }
        }
    }

    fn entry_adapter(plan: FakePlan) -> (KeyringEntryAdapter, Trace) {
        let (api, trace) = ScriptedApi::new(plan);
        (
            KeyringEntryAdapter::new(Arc::new(MacOsKeychainBoundary::new(api))),
            trace,
        )
    }

    fn recovery_facade(
        plan: FakePlan,
    ) -> (&'static NativeInteractionGate, NativeRecoveryFacade, Trace) {
        let gate = NativeInteractionGate::isolated_for_test();
        let (api, trace) = ScriptedApi::new(plan);
        let facade =
            NativeRecoveryFacade::from_boundary(gate, Box::new(MacOsKeychainBoundary::new(api)));
        (gate, facade, trace)
    }

    #[test]
    fn ordinary_read_restores_exact_prior_false_after_success() {
        let mut plan = FakePlan::default();
        plan.read
            .push_back(ReadStep::Secret(b"opaque-record".to_vec()));
        let (api, trace) = ScriptedApi::new(plan);
        let boundary = MacOsKeychainBoundary::new(api);
        let gate = NativeInteractionGate::isolated_for_test();
        let lease = gate.acquire().expect("isolated native lease");

        let secret = boundary
            .get_secret(lease.forbid_prompt(), &EntryLocator::authority())
            .expect("guarded read");

        assert_eq!(secret, b"opaque-record");
        assert_eq!(
            trace.snapshot(),
            vec![
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::Read,
                Call::SetInteraction(false),
            ]
        );
    }

    #[test]
    fn disable_failure_attempts_one_exact_restore_without_running_the_operation() {
        let mut plan = FakePlan {
            get: BoolStep::Value(true),
            ..FakePlan::default()
        };
        plan.set
            .push_back(UnitStep::Failure(NativeStatus::for_test(-70001)));
        plan.set.push_back(UnitStep::Success);
        plan.read
            .push_back(ReadStep::Secret(b"must-not-be-read".to_vec()));
        let (api, trace) = ScriptedApi::new(plan);
        let boundary = MacOsKeychainBoundary::new(api);
        let gate = NativeInteractionGate::isolated_for_test();
        let lease = gate.acquire().expect("isolated native lease");

        let result = boundary.get_secret(lease.forbid_prompt(), &EntryLocator::authority());

        assert_eq!(
            result,
            Err(crate::credentials::domain::CredentialStoreFailure::Internal)
        );
        assert_eq!(
            trace.snapshot(),
            vec![
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::SetInteraction(true),
            ]
        );
        drop(lease);
        assert!(gate.acquire().is_ok());
    }

    #[test]
    fn ordinary_read_restores_both_prior_states_after_success_and_native_error() {
        for prior in [false, true] {
            let mut success_plan = FakePlan {
                get: BoolStep::Value(prior),
                ..FakePlan::default()
            };
            success_plan
                .read
                .push_back(ReadStep::Secret(b"exact-record".to_vec()));
            let (success_api, success_trace) = ScriptedApi::new(success_plan);
            let success_boundary = MacOsKeychainBoundary::new(success_api);
            let success_gate = NativeInteractionGate::isolated_for_test();
            let success_lease = success_gate.acquire().expect("success lease");

            assert_eq!(
                success_boundary
                    .get_secret(success_lease.forbid_prompt(), &EntryLocator::authority(),),
                Ok(b"exact-record".to_vec())
            );
            assert_eq!(
                success_trace.snapshot(),
                vec![
                    Call::GetInteraction,
                    Call::SetInteraction(false),
                    Call::Read,
                    Call::SetInteraction(prior),
                ]
            );

            let mut error_plan = FakePlan {
                get: BoolStep::Value(prior),
                ..FakePlan::default()
            };
            error_plan
                .read
                .push_back(ReadStep::Failure(NativeStatus::ITEM_NOT_FOUND));
            let (error_api, error_trace) = ScriptedApi::new(error_plan);
            let error_boundary = MacOsKeychainBoundary::new(error_api);
            let error_gate = NativeInteractionGate::isolated_for_test();
            let error_lease = error_gate.acquire().expect("error lease");

            assert_eq!(
                error_boundary.get_secret(error_lease.forbid_prompt(), &EntryLocator::authority(),),
                Err(CredentialStoreFailure::Missing)
            );
            assert_eq!(
                error_trace.snapshot(),
                vec![
                    Call::GetInteraction,
                    Call::SetInteraction(false),
                    Call::Read,
                    Call::SetInteraction(prior),
                ]
            );
        }
    }

    #[test]
    fn guard_get_and_restore_failure_paths_are_closed_and_exact_once() {
        let mut get_plan = FakePlan {
            get: BoolStep::Failure(NativeStatus::for_test(-70002)),
            ..FakePlan::default()
        };
        get_plan
            .read
            .push_back(ReadStep::Secret(b"must-not-run".to_vec()));
        let (get_api, get_trace) = ScriptedApi::new(get_plan);
        let get_boundary = MacOsKeychainBoundary::new(get_api);
        let get_gate = NativeInteractionGate::isolated_for_test();
        let get_lease = get_gate.acquire().expect("get-failure lease");
        assert_eq!(
            get_boundary.get_secret(get_lease.forbid_prompt(), &EntryLocator::authority()),
            Err(CredentialStoreFailure::Internal)
        );
        assert_eq!(get_trace.snapshot(), vec![Call::GetInteraction]);
        drop(get_lease);
        assert!(get_gate.acquire().is_ok());

        let mut restore_plan = FakePlan::default();
        restore_plan.set.push_back(UnitStep::Success);
        restore_plan
            .set
            .push_back(UnitStep::Failure(NativeStatus::for_test(-70003)));
        restore_plan
            .read
            .push_back(ReadStep::Secret(b"discard-on-restore-failure".to_vec()));
        let (restore_api, restore_trace) = ScriptedApi::new(restore_plan);
        let restore_boundary = MacOsKeychainBoundary::new(restore_api);
        let restore_gate = NativeInteractionGate::isolated_for_test();
        let restore_lease = restore_gate.acquire().expect("restore-failure lease");
        assert_eq!(
            restore_boundary.get_secret(restore_lease.forbid_prompt(), &EntryLocator::authority(),),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert_eq!(
            restore_trace.snapshot(),
            vec![
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::Read,
                Call::SetInteraction(false),
            ]
        );
        drop(restore_lease);
        assert_eq!(
            restore_gate.acquire().err(),
            Some(CredentialStoreFailure::StalledWorker)
        );
    }

    #[test]
    fn restoration_uncertainty_dominates_disable_and_operation_failures() {
        let mut disable_plan = FakePlan::default();
        disable_plan
            .set
            .push_back(UnitStep::Failure(NativeStatus::for_test(-70004)));
        disable_plan
            .set
            .push_back(UnitStep::Failure(NativeStatus::for_test(-70005)));
        disable_plan
            .read
            .push_back(ReadStep::Secret(b"must-not-run".to_vec()));
        let (disable_api, disable_trace) = ScriptedApi::new(disable_plan);
        let disable_boundary = MacOsKeychainBoundary::new(disable_api);
        let disable_gate = NativeInteractionGate::isolated_for_test();
        let disable_lease = disable_gate.acquire().expect("disable uncertainty lease");
        assert_eq!(
            disable_boundary.get_secret(disable_lease.forbid_prompt(), &EntryLocator::authority(),),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert_eq!(
            disable_trace.snapshot(),
            vec![
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::SetInteraction(false),
            ]
        );

        let mut operation_plan = FakePlan::default();
        operation_plan.set.push_back(UnitStep::Success);
        operation_plan
            .set
            .push_back(UnitStep::Failure(NativeStatus::for_test(-70006)));
        operation_plan
            .read
            .push_back(ReadStep::Failure(NativeStatus::ITEM_NOT_FOUND));
        let (operation_api, operation_trace) = ScriptedApi::new(operation_plan);
        let operation_boundary = MacOsKeychainBoundary::new(operation_api);
        let operation_gate = NativeInteractionGate::isolated_for_test();
        let operation_lease = operation_gate
            .acquire()
            .expect("operation uncertainty lease");
        assert_eq!(
            operation_boundary
                .get_secret(operation_lease.forbid_prompt(), &EntryLocator::authority(),),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert_eq!(
            operation_trace.snapshot(),
            vec![
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::Read,
                Call::SetInteraction(false),
            ]
        );
    }

    #[test]
    fn unwind_restores_once_and_outer_boundary_preserves_mutation_uncertainty() {
        let mut read_plan = FakePlan {
            get: BoolStep::Value(true),
            ..FakePlan::default()
        };
        read_plan.read.push_back(ReadStep::Panic);
        let (read_adapter, read_trace) = entry_adapter(read_plan);
        let read_gate = NativeInteractionGate::isolated_for_test();
        let mut read_lease = read_gate.acquire().expect("read panic lease");
        assert_eq!(
            read_adapter.read(&mut read_lease, &EntryLocator::authority()),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert_eq!(
            read_trace.snapshot(),
            vec![
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::Read,
                Call::SetInteraction(true),
            ]
        );

        let mut mutation_plan = FakePlan::default();
        mutation_plan.find.push_back(FindStep::Found);
        mutation_plan.update.push_back(UnitStep::Panic);
        let (mutation_adapter, mutation_trace) = entry_adapter(mutation_plan);
        let mutation_gate = NativeInteractionGate::isolated_for_test();
        let mut mutation_lease = mutation_gate.acquire().expect("mutation panic lease");
        assert_eq!(
            mutation_adapter.write(
                &mut mutation_lease,
                &EntryLocator::authority(),
                b"opaque-update",
            ),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        assert_eq!(
            mutation_trace.snapshot(),
            vec![
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::Find,
                Call::Update,
                Call::SetInteraction(false),
            ]
        );
    }

    #[test]
    fn read_status_mapping_is_numeric_contextual_and_content_free() {
        let cases = [
            (
                NativeStatus::ITEM_NOT_FOUND,
                CredentialStoreFailure::Missing,
            ),
            (
                NativeStatus::INTERACTION_NOT_ALLOWED,
                CredentialStoreFailure::Locked,
            ),
            (
                NativeStatus::INTERACTION_REQUIRED,
                CredentialStoreFailure::Locked,
            ),
            (
                NativeStatus::WRITE_PERMISSION,
                CredentialStoreFailure::AccessDenied,
            ),
            (
                NativeStatus::READ_ONLY,
                CredentialStoreFailure::AccessDenied,
            ),
            (
                NativeStatus::AUTH_FAILED,
                CredentialStoreFailure::AccessDenied,
            ),
            (
                NativeStatus::READ_ONLY_ATTRIBUTE,
                CredentialStoreFailure::AccessDenied,
            ),
            (
                NativeStatus::DATA_NOT_MODIFIABLE,
                CredentialStoreFailure::AccessDenied,
            ),
            (
                NativeStatus::MISSING_ENTITLEMENT,
                CredentialStoreFailure::AccessDenied,
            ),
            (
                NativeStatus::RESTRICTED_API,
                CredentialStoreFailure::AccessDenied,
            ),
            (
                NativeStatus::NOT_AVAILABLE,
                CredentialStoreFailure::Unavailable,
            ),
            (
                NativeStatus::NO_SUCH_KEYCHAIN,
                CredentialStoreFailure::Unavailable,
            ),
            (
                NativeStatus::INVALID_KEYCHAIN,
                CredentialStoreFailure::Unavailable,
            ),
            (
                NativeStatus::NO_DEFAULT_KEYCHAIN,
                CredentialStoreFailure::Unavailable,
            ),
            (
                NativeStatus::NO_STORAGE_MODULE,
                CredentialStoreFailure::Unavailable,
            ),
            (
                NativeStatus::IN_DARK_WAKE,
                CredentialStoreFailure::Unavailable,
            ),
            (
                NativeStatus::SERVICE_NOT_AVAILABLE,
                CredentialStoreFailure::Unavailable,
            ),
            (
                NativeStatus::UNIMPLEMENTED,
                CredentialStoreFailure::Unsupported,
            ),
            (
                NativeStatus::WRONG_SECURITY_VERSION,
                CredentialStoreFailure::Unsupported,
            ),
            (NativeStatus::DECODE, CredentialStoreFailure::CorruptRecord),
            (
                NativeStatus::USER_CANCELLED,
                CredentialStoreFailure::Internal,
            ),
            (
                NativeStatus::for_test(-70999),
                CredentialStoreFailure::Internal,
            ),
        ];

        for (status, expected) in cases {
            let mut plan = FakePlan::default();
            plan.read.push_back(ReadStep::Failure(status));
            let (api, _) = ScriptedApi::new(plan);
            let boundary = MacOsKeychainBoundary::new(api);
            let gate = NativeInteractionGate::isolated_for_test();
            let lease = gate.acquire().expect("mapping lease");
            assert_eq!(
                boundary.get_secret(lease.forbid_prompt(), &EntryLocator::authority()),
                Err(expected)
            );
        }
    }

    #[test]
    fn set_uses_add_only_for_missing_and_marks_started_at_the_native_mutation() {
        let mut missing_plan = FakePlan::default();
        missing_plan
            .find
            .push_back(FindStep::Failure(NativeStatus::ITEM_NOT_FOUND));
        let (missing_adapter, missing_trace) = entry_adapter(missing_plan);
        let missing_gate = NativeInteractionGate::isolated_for_test();
        let mut missing_lease = missing_gate.acquire().expect("missing write lease");
        assert_eq!(
            missing_adapter.write(
                &mut missing_lease,
                &EntryLocator::authority(),
                b"new-record",
            ),
            Ok(())
        );
        assert_eq!(
            missing_trace.snapshot(),
            vec![
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::Find,
                Call::Add,
                Call::SetInteraction(false),
            ]
        );

        let mut found_plan = FakePlan::default();
        found_plan.find.push_back(FindStep::Found);
        let (found_adapter, found_trace) = entry_adapter(found_plan);
        let found_gate = NativeInteractionGate::isolated_for_test();
        let mut found_lease = found_gate.acquire().expect("found write lease");
        assert_eq!(
            found_adapter.write(&mut found_lease, &EntryLocator::authority(), b"replacement",),
            Ok(())
        );
        assert_eq!(
            found_trace.snapshot(),
            vec![
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::Find,
                Call::Update,
                Call::SetInteraction(false),
            ]
        );

        let mut failed_lookup_plan = FakePlan::default();
        failed_lookup_plan
            .find
            .push_back(FindStep::Failure(NativeStatus::INTERACTION_REQUIRED));
        let (failed_adapter, failed_trace) = entry_adapter(failed_lookup_plan);
        let failed_gate = NativeInteractionGate::isolated_for_test();
        let mut failed_lease = failed_gate.acquire().expect("failed lookup lease");
        assert_eq!(
            failed_adapter.write(
                &mut failed_lease,
                &EntryLocator::authority(),
                b"must-not-add",
            ),
            Err(CredentialStoreFailure::Locked)
        );
        assert_eq!(
            failed_trace.snapshot(),
            vec![
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::Find,
                Call::SetInteraction(false),
            ]
        );

        for (status, expected) in [
            (
                NativeStatus::AUTH_FAILED,
                CredentialStoreFailure::AccessDenied,
            ),
            (
                NativeStatus::NOT_AVAILABLE,
                CredentialStoreFailure::Unavailable,
            ),
            (
                NativeStatus::UNIMPLEMENTED,
                CredentialStoreFailure::Unsupported,
            ),
            (NativeStatus::DECODE, CredentialStoreFailure::Internal),
            (
                NativeStatus::USER_CANCELLED,
                CredentialStoreFailure::Internal,
            ),
            (
                NativeStatus::for_test(-71000),
                CredentialStoreFailure::Internal,
            ),
        ] {
            let mut plan = FakePlan::default();
            plan.find.push_back(FindStep::Failure(status));
            let (adapter, trace) = entry_adapter(plan);
            let gate = NativeInteractionGate::isolated_for_test();
            let mut lease = gate.acquire().expect("non-missing lookup lease");
            assert_eq!(
                adapter.write(&mut lease, &EntryLocator::authority(), b"must-not-add"),
                Err(expected)
            );
            assert!(!trace.snapshot().contains(&Call::Add));
            assert!(!trace.snapshot().contains(&Call::Update));
        }
    }

    #[test]
    fn add_and_update_statuses_keep_their_context_after_mutation_start() {
        for (status, expected) in [
            (
                NativeStatus::DUPLICATE_ITEM,
                CredentialStoreFailure::RevisionConflict,
            ),
            (
                NativeStatus::DATA_TOO_LARGE,
                CredentialStoreFailure::PayloadTooLarge,
            ),
        ] {
            let mut add_plan = FakePlan::default();
            add_plan
                .find
                .push_back(FindStep::Failure(NativeStatus::ITEM_NOT_FOUND));
            add_plan.add.push_back(UnitStep::Failure(status));
            let (add_adapter, _) = entry_adapter(add_plan);
            let add_gate = NativeInteractionGate::isolated_for_test();
            let mut add_lease = add_gate.acquire().expect("add status lease");
            assert_eq!(
                add_adapter.write(&mut add_lease, &EntryLocator::authority(), b"record"),
                Err(expected)
            );
        }

        let mut update_plan = FakePlan::default();
        update_plan.find.push_back(FindStep::Found);
        update_plan
            .update
            .push_back(UnitStep::Failure(NativeStatus::DATA_TOO_LARGE));
        let (update_adapter, _) = entry_adapter(update_plan);
        let update_gate = NativeInteractionGate::isolated_for_test();
        let mut update_lease = update_gate.acquire().expect("update status lease");
        assert_eq!(
            update_adapter.write(&mut update_lease, &EntryLocator::authority(), b"record",),
            Err(CredentialStoreFailure::PayloadTooLarge)
        );
    }

    #[test]
    fn mutation_unknown_and_restore_uncertainty_map_commit_unknown_after_start() {
        for post_start_step in [
            UnitStep::Failure(NativeStatus::for_test(-71001)),
            UnitStep::Failure(NativeStatus::ITEM_NOT_FOUND),
        ] {
            let mut plan = FakePlan::default();
            plan.find.push_back(FindStep::Found);
            plan.update.push_back(post_start_step);
            let (adapter, _) = entry_adapter(plan);
            let gate = NativeInteractionGate::isolated_for_test();
            let mut lease = gate.acquire().expect("unknown mutation lease");
            assert_eq!(
                adapter.write(&mut lease, &EntryLocator::authority(), b"replacement"),
                Err(CredentialStoreFailure::CommitUnknown)
            );
        }

        let mut restore_plan = FakePlan::default();
        restore_plan.find.push_back(FindStep::Found);
        restore_plan.set.push_back(UnitStep::Success);
        restore_plan
            .set
            .push_back(UnitStep::Failure(NativeStatus::for_test(-71002)));
        let (restore_adapter, restore_trace) = entry_adapter(restore_plan);
        let restore_gate = NativeInteractionGate::isolated_for_test();
        let mut restore_lease = restore_gate.acquire().expect("mutation restore lease");
        assert_eq!(
            restore_adapter.write(
                &mut restore_lease,
                &EntryLocator::authority(),
                b"replacement",
            ),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        assert_eq!(
            restore_trace.snapshot(),
            vec![
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::Find,
                Call::Update,
                Call::SetInteraction(false),
            ]
        );
    }

    #[test]
    fn delete_checks_raw_result_then_runs_a_separate_absence_readback() {
        let mut plan = FakePlan::default();
        plan.find.push_back(FindStep::Found);
        plan.read
            .push_back(ReadStep::Failure(NativeStatus::ITEM_NOT_FOUND));
        let (adapter, trace) = entry_adapter(plan);
        let gate = NativeInteractionGate::isolated_for_test();
        let mut lease = gate.acquire().expect("delete lease");
        assert_eq!(
            adapter.delete_and_verify_absent(&mut lease, &EntryLocator::authority()),
            Ok(())
        );
        assert_eq!(
            trace.snapshot(),
            vec![
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::Find,
                Call::Delete,
                Call::SetInteraction(false),
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::Read,
                Call::SetInteraction(false),
            ]
        );
    }

    #[test]
    fn delete_lookup_missing_still_confirms_absence_without_starting_delete() {
        let mut plan = FakePlan::default();
        plan.find
            .push_back(FindStep::Failure(NativeStatus::ITEM_NOT_FOUND));
        plan.read
            .push_back(ReadStep::Failure(NativeStatus::ITEM_NOT_FOUND));
        let (adapter, trace) = entry_adapter(plan);
        let gate = NativeInteractionGate::isolated_for_test();
        let mut lease = gate.acquire().expect("missing delete lease");

        assert_eq!(
            adapter.delete_and_verify_absent(&mut lease, &EntryLocator::authority()),
            Ok(())
        );
        assert!(!trace.snapshot().contains(&Call::Delete));
        assert!(trace.snapshot().contains(&Call::Read));
    }

    #[test]
    fn delete_after_found_mismatch_and_present_readback_are_commit_unknown() {
        let mut mismatch_plan = FakePlan::default();
        mismatch_plan.find.push_back(FindStep::Found);
        mismatch_plan
            .delete
            .push_back(UnitStep::Failure(NativeStatus::ITEM_NOT_FOUND));
        let (mismatch_adapter, mismatch_trace) = entry_adapter(mismatch_plan);
        let mismatch_gate = NativeInteractionGate::isolated_for_test();
        let mut mismatch_lease = mismatch_gate.acquire().expect("delete mismatch lease");
        assert_eq!(
            mismatch_adapter
                .delete_and_verify_absent(&mut mismatch_lease, &EntryLocator::authority()),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        assert!(!mismatch_trace.snapshot().contains(&Call::Read));

        let mut readback_plan = FakePlan::default();
        readback_plan.find.push_back(FindStep::Found);
        readback_plan
            .read
            .push_back(ReadStep::Secret(b"still-present".to_vec()));
        let (readback_adapter, readback_trace) = entry_adapter(readback_plan);
        let readback_gate = NativeInteractionGate::isolated_for_test();
        let mut readback_lease = readback_gate.acquire().expect("present readback lease");
        assert_eq!(
            readback_adapter
                .delete_and_verify_absent(&mut readback_lease, &EntryLocator::authority()),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        assert!(readback_trace.snapshot().contains(&Call::Read));
    }

    #[test]
    fn explicit_recovery_unlocks_once_without_forcing_interaction_true_then_verifies() {
        let plan = FakePlan::default();
        let (_gate, facade, trace) = recovery_facade(plan);
        let cancellation = CancellationToken::new();

        assert_eq!(
            facade.diagnose_or_unlock(RecoveryTarget::CredentialStore, &cancellation),
            Ok(RecoveryOutcome::Ready)
        );
        assert_eq!(
            trace.snapshot(),
            vec![
                Call::Unlock,
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::Status,
                Call::SetInteraction(false),
            ]
        );
        assert!(!trace.snapshot().contains(&Call::SetInteraction(true)));
    }

    #[test]
    fn recovery_verifies_unlock_read_write_bits_independently() {
        let cases = [
            (
                KeychainStatus {
                    unlocked: true,
                    readable: true,
                    writable: true,
                },
                RecoveryOutcome::Ready,
            ),
            (
                KeychainStatus {
                    unlocked: false,
                    readable: true,
                    writable: true,
                },
                RecoveryOutcome::RecoveryRequired,
            ),
            (
                KeychainStatus {
                    unlocked: true,
                    readable: false,
                    writable: true,
                },
                RecoveryOutcome::RecoveryRequired,
            ),
            (
                KeychainStatus {
                    unlocked: true,
                    readable: true,
                    writable: false,
                },
                RecoveryOutcome::RecoveryRequired,
            ),
        ];

        for (status, expected) in cases {
            let mut plan = FakePlan::default();
            plan.status.push_back(StatusStep::Value(status));
            let (_gate, facade, _) = recovery_facade(plan);
            assert_eq!(
                facade.diagnose_or_unlock(
                    RecoveryTarget::CredentialStore,
                    &CancellationToken::new(),
                ),
                Ok(expected)
            );
        }
    }

    #[test]
    fn recovery_checks_cancellation_immediately_before_and_after_one_unlock() {
        let before = Arc::new(CancellationToken::new());
        let before_plan = FakePlan {
            cancel_on_open: Some(before.clone()),
            ..FakePlan::default()
        };
        let (_before_gate, before_facade, before_trace) = recovery_facade(before_plan);
        assert_eq!(
            before_facade.diagnose_or_unlock(RecoveryTarget::CredentialStore, before.as_ref()),
            Err(CredentialStoreFailure::Cancelled)
        );
        assert!(!before_trace.snapshot().contains(&Call::Unlock));
        assert!(before_trace.snapshot().contains(&Call::Status));

        let after = Arc::new(CancellationToken::new());
        let after_plan = FakePlan {
            cancel_on_unlock: Some(after.clone()),
            ..FakePlan::default()
        };
        let (_after_gate, after_facade, after_trace) = recovery_facade(after_plan);
        assert_eq!(
            after_facade.diagnose_or_unlock(RecoveryTarget::CredentialStore, after.as_ref()),
            Err(CredentialStoreFailure::Cancelled)
        );
        assert_eq!(
            after_trace
                .snapshot()
                .iter()
                .filter(|call| **call == Call::Unlock)
                .count(),
            1
        );
        assert!(after_trace.snapshot().contains(&Call::Status));
    }

    #[test]
    fn unknown_unlock_is_uncertain_while_native_cancellation_is_closed_and_verified() {
        let mut unknown_plan = FakePlan::default();
        unknown_plan
            .unlock
            .push_back(UnitStep::Failure(NativeStatus::for_test(-72001)));
        let (unknown_gate, unknown_facade, unknown_trace) = recovery_facade(unknown_plan);
        assert_eq!(
            unknown_facade
                .diagnose_or_unlock(RecoveryTarget::CredentialStore, &CancellationToken::new(),),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert_eq!(unknown_trace.snapshot(), vec![Call::Unlock]);
        assert_eq!(
            unknown_gate.acquire().err(),
            Some(CredentialStoreFailure::StalledWorker)
        );

        let mut cancelled_plan = FakePlan::default();
        cancelled_plan
            .unlock
            .push_back(UnitStep::Failure(NativeStatus::USER_CANCELLED));
        let (_cancelled_gate, cancelled_facade, cancelled_trace) = recovery_facade(cancelled_plan);
        assert_eq!(
            cancelled_facade
                .diagnose_or_unlock(RecoveryTarget::CredentialStore, &CancellationToken::new(),),
            Err(CredentialStoreFailure::Cancelled)
        );
        assert!(cancelled_trace.snapshot().contains(&Call::Status));
    }

    #[test]
    fn recovery_verification_restore_failure_stalls_the_gate() {
        let mut plan = FakePlan::default();
        plan.set.push_back(UnitStep::Success);
        plan.set
            .push_back(UnitStep::Failure(NativeStatus::for_test(-72002)));
        let (gate, facade, trace) = recovery_facade(plan);
        assert_eq!(
            facade.diagnose_or_unlock(RecoveryTarget::CredentialStore, &CancellationToken::new(),),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert!(trace.snapshot().contains(&Call::Status));
        assert_eq!(
            gate.acquire().err(),
            Some(CredentialStoreFailure::StalledWorker)
        );
    }

    #[test]
    fn source_inventory_keeps_policy_safe_raw_core_capability_blind_and_content_free() {
        let safe = include_str!("macos_keychain.rs");
        let safe_production = safe
            .split_once("#[cfg(test)]\nmod tests")
            .expect("safe policy test boundary")
            .0;
        let ffi = include_str!("macos_keychain_ffi.rs");
        let other_adapter_sources = concat!(
            include_str!("mod.rs"),
            include_str!("keyring_entry.rs"),
            include_str!("native_interaction.rs"),
            include_str!("native_keyring.rs"),
            include_str!("authority_journal.rs"),
        );

        assert!(!safe_production.contains("unsafe"));
        assert!(ffi.contains("unsafe"));
        for capability in [
            "ForbidPrompt",
            "AllowPrompt",
            "RecoveryBoundary",
            "MutationInvocation",
        ] {
            assert!(!ffi.contains(capability));
        }
        for symbol in [
            "SecKeychainGetUserInteractionAllowed",
            "SecKeychainSetUserInteractionAllowed",
            "SecKeychainCopyDomainDefault",
            "SecKeychainFindGenericPassword",
            "SecKeychainAddGenericPassword",
            "SecKeychainItemModifyAttributesAndData",
            "SecKeychainItemDelete",
            "SecKeychainItemFreeContent",
            "SecKeychainUnlock",
            "SecKeychainGetStatus",
        ] {
            assert!(ffi.contains(symbol), "missing checked raw symbol {symbol}");
        }
        for forbidden in [
            "SecCopyErrorMessageString",
            "disable_user_interaction",
            "set_generic_password",
            "println!",
            "eprintln!",
            "tracing::",
            "log::",
        ] {
            assert!(
                !ffi.contains(forbidden),
                "forbidden raw-core surface {forbidden}"
            );
        }
        for numeric in ["-25300", "-25308", "-25315", "-128", "-26275"] {
            assert!(!safe_production.contains(numeric));
        }
        assert!(safe_production.contains("SecurityFrameworkCore::new"));
        for raw_type in ["SecurityFrameworkCore", "UserKeychain", "KeychainItem"] {
            assert!(!other_adapter_sources.contains(raw_type));
        }
        assert!(ffi.contains("Zeroize"));
        assert!(ffi.contains("SecKeychainItemFreeContent"));
        let native_zeroize = ffi
            .find("native_secret.zeroize();")
            .expect("native buffer zeroization");
        let checked_free = ffi
            .find("SecKeychainItemFreeContent(ptr::null_mut(), self.data)")
            .expect("checked native buffer free");
        assert!(native_zeroize < checked_free);
        assert!(
            ffi[checked_free..].contains("NativeStatus::checked(status)"),
            "native buffer cleanup status must be checked"
        );
        assert!(safe_production.contains("Result<Zeroizing<Vec<u8>>, NativeStatus>"));
    }

    #[test]
    fn restoration_panic_never_escapes_drop_during_operation_unwind() {
        let mut plan = FakePlan::default();
        plan.set.push_back(UnitStep::Success);
        plan.set.push_back(UnitStep::Panic);
        plan.read.push_back(ReadStep::Panic);
        let (adapter, trace) = entry_adapter(plan);
        let gate = NativeInteractionGate::isolated_for_test();
        let mut lease = gate.acquire().expect("double unwind lease");

        let result = catch_unwind(AssertUnwindSafe(|| {
            adapter.read(&mut lease, &EntryLocator::authority())
        }));

        assert!(matches!(
            result,
            Ok(Err(CredentialStoreFailure::StalledWorker))
        ));
        assert_eq!(
            trace.snapshot(),
            vec![
                Call::GetInteraction,
                Call::SetInteraction(false),
                Call::Read,
                Call::SetInteraction(false),
            ]
        );
    }
}
