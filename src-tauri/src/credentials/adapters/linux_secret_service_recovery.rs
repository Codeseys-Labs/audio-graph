//! Interactive policy, a private lexical child of the sealed recovery gate.
#![cfg_attr(not(test), allow(dead_code))] // The Linux factory remains dormant.

use super::super::process_native_interaction_gate;
use super::{
    AllowPrompt, CancellationToken, ForbidPrompt, NativeRecoveryFacade, RecoveryBoundary,
    RecoveryFailure, RecoveryOutcome, RecoveryTarget,
};
use crate::crash_handler::{RedactedPanicOp, catch_redacted_panic};
use crate::credentials::adapters::linux_secret_service_wire::{
    Address, CompletionStream, LockedTarget, OwnedObjectPath, WireError, block_on,
    define_recovery_peer, ensure_worker_context, next_completion, recovery_until, session_address,
};
use crate::credentials::domain::CredentialStoreFailure as Failure;
use futures_util::FutureExt;
use futures_util::future::{Either, select};
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::Mutex;
use std::task::Poll;
use std::time::{Duration, Instant};
use zruntime::SharedRuntime;

const SERVICE: &str = "org.freedesktop.Secret.Service";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const PROMPT: &str = "org.freedesktop.Secret.Prompt";
const UNLOCK_METHOD: &str = "Unlock";
const PROMPT_METHOD: &str = "Prompt";
const DISMISS_GRACE: Duration = Duration::from_millis(500);
const RETIRE_GRACE: Duration = Duration::from_millis(125);
const CANCELLATION_POLL: Duration = Duration::from_millis(25);

#[cfg(test)]
#[derive(Default)]
pub(super) struct TestOptions {
    gate: Option<&'static super::NativeInteractionGate>,
    address: Option<Address>,
    cancel_with_queued_completion: bool,
    timeout_with_queued_completion: bool,
    panic_after_prompt: bool,
}

/// Dedicated worker only. The existing facade holds the lease, issues the
/// AllowPrompt capability, and invokes verification with a separate ForbidPrompt.
pub(super) fn recovery_facade(
    target: LockedTarget,
    window: Option<String>,
    budget: Duration,
    #[cfg(test)] test: TestOptions,
) -> Result<NativeRecoveryFacade, Failure> {
    let end = Instant::now()
        .checked_add(budget)
        .ok_or(Failure::Internal)?;
    let boundary = Boundary {
        state: Mutex::new(State::Pending(target)),
        window,
        end,
        #[cfg(test)]
        test,
    };
    let gate = process_native_interaction_gate();
    #[cfg(test)]
    let gate = boundary.test.gate.unwrap_or(gate);
    Ok(NativeRecoveryFacade::from_boundary(
        gate,
        Box::new(boundary),
    ))
}

enum State {
    Pending(LockedTarget),
    Verify(LockedTarget),
    Used,
}

struct Boundary {
    state: Mutex<State>,
    window: Option<String>,
    end: Instant,
    #[cfg(test)]
    test: TestOptions,
}

impl Boundary {
    fn state(&self) -> Result<std::sync::MutexGuard<'_, State>, RecoveryFailure> {
        self.state
            .lock()
            .map_err(|_| RecoveryFailure::Closed(Failure::RevisionConflict))
    }

    fn take_pending(&self) -> Result<LockedTarget, RecoveryFailure> {
        let mut state = self.state()?;
        if let State::Pending(_) = &*state
            && let State::Pending(target) = std::mem::replace(&mut *state, State::Used)
        {
            return Ok(target);
        }
        Err(RecoveryFailure::Closed(Failure::RevisionConflict))
    }
}

impl RecoveryBoundary for Boundary {
    fn recover(
        &self,
        permission: &AllowPrompt<'_, '_>,
        _target: RecoveryTarget,
        cancellation: &CancellationToken,
    ) -> Result<(), RecoveryFailure> {
        let target = self.take_pending()?;
        ensure_worker_context().map_err(closed_wire)?;
        #[cfg(test)]
        let address = self.test.address.clone().map_or_else(session_address, Ok);
        #[cfg(not(test))]
        let address = session_address();
        block_on(interact(
            permission,
            &target,
            address.map_err(closed_wire)?,
            self.window.as_deref().unwrap_or_default(),
            self.end,
            cancellation,
            #[cfg(test)]
            &self.test,
        ))?;
        *self.state()? = State::Verify(target);
        Ok(())
    }

    fn verify(
        &self,
        permission: &ForbidPrompt<'_>,
        _target: RecoveryTarget,
    ) -> Result<RecoveryOutcome, RecoveryFailure> {
        let target = {
            let mut state = self.state()?;
            let State::Verify(_) = &*state else {
                // The sealed facade invokes this after an ordinary closed error.
                return Ok(RecoveryOutcome::RecoveryRequired);
            };
            std::mem::replace(&mut *state, State::Used)
        };
        let State::Verify(target) = target else {
            unreachable!()
        };
        match target.verify(permission) {
            Ok(()) => Ok(RecoveryOutcome::Ready),
            Err(WireError(Failure::Locked)) => Ok(RecoveryOutcome::RecoveryRequired),
            Err(error) => Err(closed_wire(error)),
        }
    }
}

type WireResult<T> = Result<T, WireError>;
type UnlockReply = (Vec<OwnedObjectPath>, OwnedObjectPath);

define_recovery_peer!();
use interactive_transport::Peer;

impl Peer<'_, '_> {
    async fn dismiss(
        &self,
        path: &OwnedObjectPath,
        completions: Option<CompletionStream>,
    ) -> Option<bool> {
        let end = Instant::now() + DISMISS_GRACE;
        let completions = match completions {
            Some(stream) => Some(stream),
            None => recovery_until(end - DISMISS_GRACE / 2, self.subscribe(path))
                .await
                .ok(),
        };
        let request = self.call::<_, ()>(path.as_str(), PROMPT, "Dismiss", &());
        let Some(mut completions) = completions else {
            let _ = recovery_until(end, request).await;
            return None;
        };
        recovery_until(
            end,
            dismiss_and_settle(request, next_completion(&mut completions)),
        )
        .await
        .ok()
    }
}

async fn dismiss_and_settle(
    request: impl Future<Output = WireResult<()>>,
    completion: impl Future<Output = WireResult<bool>>,
) -> WireResult<bool> {
    match select(pin!(request), pin!(completion)).await {
        Either::Left((_, completion)) => completion.await,
        Either::Right((result, request)) => {
            // A closed or malformed stream does not settle the prompt.
            // Keep polling Dismiss through the same bounded deadline.
            if result.is_err() {
                let _ = request.await;
            }
            result
        }
    }
}

async fn interact(
    permission: &AllowPrompt<'_, '_>,
    target: &LockedTarget,
    address: Address,
    window: &str,
    end: Instant,
    cancellation: &CancellationToken,
    #[cfg(test)] test: &TestOptions,
) -> Result<(), RecoveryFailure> {
    let wait = Wait { end, cancellation };
    let peer = match wait
        .run(Peer::connect_for_recovery(permission, target, address))
        .await
    {
        Ok(Some(peer)) => peer,
        Ok(None) => return Ok(()), // The target unlocked before dispatch.
        Err(stop) => return Err(stop.closed()),
    };
    let outcome = unlock(
        &peer,
        target,
        window,
        &wait,
        #[cfg(test)]
        test,
    )
    .await;
    let _ = recovery_until(Instant::now() + RETIRE_GRACE, peer.retire()).await;
    outcome
}

async fn unlock(
    peer: &Peer<'_, '_>,
    target: &LockedTarget,
    window: &str,
    wait: &Wait<'_>,
    #[cfg(test)] test: &TestOptions,
) -> Result<(), RecoveryFailure> {
    let objects = [target.path()];
    let reply: UnlockReply = match wait
        .run(async {
            peer.require_current().await?;
            peer.call(SERVICE_PATH, SERVICE, UNLOCK_METHOD, &(objects.as_slice(),))
                .await
        })
        .await
    {
        Ok(reply) => reply,
        Err(stop) => return Err(stop.closed()),
    };
    let (_, path) = reply;
    if path.as_str() == "/" {
        return Ok(());
    }
    PromptGuard {
        peer,
        path,
        completions: None,
    }
    .settle(
        window,
        wait,
        #[cfg(test)]
        test,
    )
    .await
}

// Keep the prompt and its original connection alive across an unwind so
// cleanup can still be driven within the same runtime. Drop cannot await.
struct PromptGuard<'peer, 'lease, 'gate> {
    peer: &'peer Peer<'lease, 'gate>,
    path: OwnedObjectPath,
    completions: Option<CompletionStream>,
}

impl PromptGuard<'_, '_, '_> {
    async fn settle(
        mut self,
        window: &str,
        wait: &Wait<'_>,
        #[cfg(test)] test: &TestOptions,
    ) -> Result<(), RecoveryFailure> {
        let result = catch_interactive(async {
            let stream = wait.run(self.peer.subscribe(&self.path)).await?;
            let completions = self.completions.insert(stream);
            wait.run(async {
                self.peer.require_current().await?;
                self.peer
                    .call::<_, ()>(self.path.as_str(), PROMPT, PROMPT_METHOD, &(window,))
                    .await?;
                #[cfg(test)]
                {
                    assert!(!test.panic_after_prompt, "scripted prompt unwind");
                    if test.cancel_with_queued_completion {
                        wait.cancellation.cancel();
                    }
                    if test.cancel_with_queued_completion || test.timeout_with_queued_completion {
                        std::future::pending::<()>().await;
                    }
                }
                next_completion(completions).await
            })
            .await
        })
        .await;
        match result {
            Ok(Ok(dismissed)) => completion_outcome(dismissed),
            Ok(Err(stop)) => {
                if let Some(dismissed) = self.queued_completion() {
                    return completion_outcome(dismissed);
                }
                match self
                    .dismiss()
                    .await
                    .map_err(|()| RecoveryFailure::Uncertain)?
                {
                    Some(false) => Ok(()),
                    Some(true) => Err(stop.closed()),
                    None => {
                        log::warn!(
                            "Secret Service did not acknowledge dismissal. A dialog may still be open."
                        );
                        Err(stop.closed())
                    }
                }
            }
            Err(()) => {
                if !matches!(self.dismiss().await, Ok(Some(_))) {
                    log::warn!("Secret Service recovery panicked. A dialog may still be open.");
                }
                Err(RecoveryFailure::Uncertain)
            }
        }
    }

    fn queued_completion(&mut self) -> Option<bool> {
        self.completions
            .as_mut()
            .and_then(|stream| next_completion(stream).now_or_never()?.ok())
    }

    async fn dismiss(self) -> Result<Option<bool>, ()> {
        catch_interactive(self.peer.dismiss(&self.path, self.completions)).await
    }
}

fn completion_outcome(dismissed: bool) -> Result<(), RecoveryFailure> {
    if dismissed {
        Err(RecoveryFailure::Closed(Failure::Cancelled))
    } else {
        Ok(())
    }
}

async fn catch_interactive<T>(future: impl Future<Output = T>) -> Result<T, ()> {
    let mut future = pin!(future);
    poll_fn(|cx| {
        match catch_redacted_panic(RedactedPanicOp::CredentialBoundary, || {
            future.as_mut().poll(cx)
        }) {
            Ok(result) => result.map(Ok),
            Err(_) => Poll::Ready(Err(())),
        }
    })
    .await
}

struct Wait<'a> {
    end: Instant,
    cancellation: &'a CancellationToken,
}

impl Wait<'_> {
    // Every interactive phase shares one deadline. The sealed token is an
    // atomic flag, so use an executor timer to observe cancellation.
    async fn run<T>(&self, work: impl Future<Output = WireResult<T>>) -> Result<T, Stop> {
        if self.cancellation.is_cancelled() {
            return Err(Stop::Cancelled);
        }
        if Instant::now() >= self.end {
            return Err(Stop::TimedOut);
        }
        let runtime = SharedRuntime::current().map_err(|_| Stop::Failed(Failure::Unavailable))?;
        let cancelled = async {
            while !self.cancellation.is_cancelled() {
                runtime.sleep(CANCELLATION_POLL).await;
            }
        };
        match runtime
            .timeout_at(self.end, select(pin!(cancelled), pin!(work)))
            .await
        {
            Err(_) => Err(Stop::TimedOut),
            Ok(Either::Left(_)) => Err(Stop::Cancelled),
            Ok(Either::Right((result, _))) => result.map_err(|WireError(code)| Stop::Failed(code)),
        }
    }
}

enum Stop {
    Cancelled,
    TimedOut,
    Failed(Failure),
}

impl Stop {
    fn closed(self) -> RecoveryFailure {
        match self {
            Self::Cancelled => RecoveryFailure::Closed(Failure::Cancelled),
            Self::TimedOut => RecoveryFailure::Closed(Failure::Unavailable),
            Self::Failed(code) => closed_wire(WireError(code)),
        }
    }
}

fn closed_wire(WireError(code): WireError) -> RecoveryFailure {
    // A provider result never latches the process. A pre-existing gate latch
    // is enforced by the sealed facade before it calls this boundary.
    let closed = match code {
        Failure::StalledWorker | Failure::CommitUnknown => Failure::Unavailable,
        code => code,
    };
    RecoveryFailure::Closed(closed)
}

#[cfg(test)]
#[path = "linux_secret_service_recovery/tests.rs"]
mod tests;
