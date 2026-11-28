//! Interactive policy, a private lexical child of the sealed recovery gate.
#![cfg_attr(not(test), allow(dead_code))] // The Linux factory remains dormant.

use super::super::process_native_interaction_gate;
use super::{
    AllowPrompt, CancellationToken, ForbidPrompt, NativeRecoveryFacade, RecoveryBoundary,
    RecoveryFailure, RecoveryOutcome, RecoveryTarget,
};
use crate::crash_handler::{RedactedPanicOp, catch_redacted_panic};
use crate::credentials::adapters::linux_secret_service_wire::{
    LockedTarget, WireError, ensure_worker_context, recovery_error, recovery_until,
};
use crate::credentials::domain::CredentialStoreFailure as Failure;
use futures_util::future::{Either, select};
use futures_util::{FutureExt, StreamExt};
use serde::Serialize;
use std::future::{Future, poll_fn};
use std::path::Path;
use std::pin::pin;
use std::sync::{Arc, OnceLock};
use std::task::Poll;
use std::time::{Duration, Instant};
use zbus::message::Type;
use zbus::names::OwnedUniqueName;
use zbus::proxy::{Builder as ProxyBuilder, CacheProperties, MethodFlags};
use zbus::wire::DynamicDeserialize;
use zbus::{
    Connection, DynamicType, MatchRule, MessageStream, OwnedObjectPath, OwnedVariant, Proxy,
};
use zruntime::SharedRuntime;

const SERVICE: &str = "org.freedesktop.Secret.Service";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const PROMPT: &str = "org.freedesktop.Secret.Prompt";
const UNLOCK_METHOD: &str = "Unlock";
const PROMPT_METHOD: &str = "Prompt";
const DISMISS_GRACE: Duration = Duration::from_millis(500);
const RETIRE_GRACE: Duration = Duration::from_millis(125);
const CANCELLATION_POLL: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Dismissal {
    NoPrompt,
    ServiceAcknowledged,
    Unacknowledged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Verified,
    DismissedByUser,
    Cancelled(Dismissal),
    TimedOut(Dismissal),
    StillLocked,
    Unavailable,
    Unsupported,
    Failed(Failure),
    PromptMayStillBeOpen(Dismissal),
}

#[cfg(test)]
#[derive(Default)]
struct TestOptions {
    gate: Option<&'static super::NativeInteractionGate>,
    daemon: Option<&'static str>,
    cancel_with_queued_completion: bool,
    panic_after_prompt: bool,
}

/// Dedicated worker only. The existing facade holds the lease, issues the
/// AllowPrompt capability, and invokes verification with a separate ForbidPrompt.
fn recover_exact(
    target: LockedTarget,
    window: Option<String>,
    budget: Duration,
    cancellation: &CancellationToken,
    #[cfg(test)] test: TestOptions,
) -> Outcome {
    if let Err(WireError(code)) = ensure_worker_context() {
        return failed(code);
    }
    let Some(end) = Instant::now().checked_add(budget) else {
        return Outcome::Failed(Failure::Internal);
    };
    let result = Arc::new(OnceLock::new());
    let boundary = Boundary {
        target,
        window,
        end,
        result: Arc::clone(&result),
        #[cfg(test)]
        test,
    };
    let gate = process_native_interaction_gate();
    #[cfg(test)]
    let gate = boundary.test.gate.unwrap_or(gate);
    let facade = NativeRecoveryFacade::from_boundary(gate, Box::new(boundary));
    let closed = facade.diagnose_or_unlock(RecoveryTarget::CredentialStore, cancellation);
    result.get().copied().unwrap_or_else(|| match closed {
        Err(Failure::Cancelled) => Outcome::Cancelled(Dismissal::NoPrompt),
        Err(code) => failed(code),
        Ok(_) => Outcome::Failed(Failure::Internal),
    })
}

struct Boundary {
    target: LockedTarget,
    window: Option<String>,
    end: Instant,
    // The sealed facade returns a platform-neutral readiness result. Keep the
    // detailed closed Linux outcome in this single-call boundary.
    result: Arc<OnceLock<Outcome>>,
    #[cfg(test)]
    test: TestOptions,
}

impl RecoveryBoundary for Boundary {
    fn recover(
        &self,
        permission: &AllowPrompt<'_, '_>,
        _target: RecoveryTarget,
        cancellation: &CancellationToken,
    ) -> Result<(), RecoveryFailure> {
        let outcome = zbus::block_on(interact(
            permission,
            &self.target,
            self.window.as_deref().unwrap_or_default(),
            self.end,
            cancellation,
            #[cfg(test)]
            &self.test,
        ));
        if let Some(outcome) = outcome {
            let _ = self.result.set(outcome);
        }
        Ok(())
    }

    fn verify(
        &self,
        permission: &ForbidPrompt<'_>,
        _target: RecoveryTarget,
    ) -> Result<RecoveryOutcome, RecoveryFailure> {
        if self.result.get().is_some() {
            return Ok(RecoveryOutcome::RecoveryRequired);
        }
        let outcome = match self.target.verify(permission) {
            Ok(()) => Outcome::Verified,
            Err(WireError(Failure::Locked)) => Outcome::StillLocked,
            Err(WireError(code)) => failed(code),
        };
        let _ = self.result.set(outcome);
        Ok(if outcome == Outcome::Verified {
            RecoveryOutcome::Ready
        } else {
            RecoveryOutcome::RecoveryRequired
        })
    }
}

type WireResult<T> = Result<T, WireError>;
type UnlockReply = (Vec<OwnedObjectPath>, OwnedObjectPath);

struct Peer<'lease, 'gate> {
    _permission: &'lease AllowPrompt<'lease, 'gate>,
    connection: Connection,
    owner: OwnedUniqueName,
}

impl<'lease, 'gate> Peer<'lease, 'gate> {
    // Only the lexical child can own the raw interactive connection, and it
    // takes the existing sealed grant before it can open one.
    async fn connect_for_recovery(
        permission: &'lease AllowPrompt<'lease, 'gate>,
        target: &LockedTarget,
        #[cfg(test)] test: &TestOptions,
    ) -> WireResult<Option<Self>> {
        let connection = zbus::connection::Builder::address(target.address().clone())
            .build()
            .await
            .map_err(recovery_error)?;
        let (owner, pid, locked) = target.check_recovery_connection(&connection).await?;
        #[cfg(test)]
        let override_name = test.daemon;
        #[cfg(not(test))]
        let override_name: Option<&str> = None;
        let executable = match override_name {
            Some(executable) => executable.into(),
            None => {
                std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|_| Failure::Unsupported)?
            }
        };
        if !allows_interactive_recovery(&executable) {
            return Err(Failure::Unsupported.into());
        }
        Ok(locked.then_some(Self {
            _permission: permission,
            connection,
            owner,
        }))
    }
}

fn allows_interactive_recovery(executable: &Path) -> bool {
    executable
        .file_name()
        .is_some_and(|name| name == "gnome-keyring-daemon")
}

impl Peer<'_, '_> {
    async fn call<B, R>(&self, path: &str, interface: &str, method: &str, body: &B) -> WireResult<R>
    where
        B: Serialize + DynamicType,
        R: for<'de> DynamicDeserialize<'de>,
    {
        let proxy: Proxy<'_> = ProxyBuilder::new(&self.connection)
            .destination(self.owner.as_str())
            .path(path)
            .interface(interface)
            .cache_properties(CacheProperties::No)
            .build()
            .await
            .map_err(recovery_error)?;
        proxy
            .call_with_flags(method, MethodFlags::NO_AUTO_START, body)
            .await
            .map_err(recovery_error)?
            .ok_or_else(|| Failure::Internal.into())
    }

    async fn subscribe(&self, path: &OwnedObjectPath) -> WireResult<MessageStream> {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(self.owner.as_str())
            .path(path.as_str())
            .interface(PROMPT)
            .member("Completed")
            .build()
            .map_err(recovery_error)?;
        MessageStream::for_match_rule(rule, &self.connection, None)
            .await
            .map_err(recovery_error)
    }

    async fn dismiss(
        &self,
        path: &OwnedObjectPath,
        completions: Option<MessageStream>,
    ) -> Dismissal {
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
            return Dismissal::Unacknowledged;
        };
        let settlement = async {
            let mut request = pin!(request);
            match select(request.as_mut(), pin!(completed(&mut completions))).await {
                Either::Left((_, completion)) => completion.await,
                Either::Right((result, _)) => result,
            }
        };
        if matches!(recovery_until(end, settlement).await, Ok(true)) {
            Dismissal::ServiceAcknowledged
        } else {
            Dismissal::Unacknowledged
        }
    }
}

async fn interact(
    permission: &AllowPrompt<'_, '_>,
    target: &LockedTarget,
    window: &str,
    end: Instant,
    cancellation: &CancellationToken,
    #[cfg(test)] test: &TestOptions,
) -> Option<Outcome> {
    let wait = Wait { end, cancellation };
    let peer = match wait
        .run(Peer::connect_for_recovery(
            permission,
            target,
            #[cfg(test)]
            test,
        ))
        .await
    {
        Ok(Some(peer)) => peer,
        Ok(None) => return None, // The target unlocked before dispatch.
        Err(stop) => return Some(stop.outcome(Dismissal::NoPrompt)),
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
    let _ = recovery_until(Instant::now() + RETIRE_GRACE, async {
        peer.connection.close().await.map_err(recovery_error)
    })
    .await;
    outcome
}

async fn unlock(
    peer: &Peer<'_, '_>,
    target: &LockedTarget,
    window: &str,
    wait: &Wait<'_>,
    #[cfg(test)] test: &TestOptions,
) -> Option<Outcome> {
    let objects = [target.path()];
    let reply: UnlockReply = match wait
        .run(peer.call(SERVICE_PATH, SERVICE, UNLOCK_METHOD, &(objects.as_slice(),)))
        .await
    {
        Ok(reply) => reply,
        Err(stop) => return Some(stop.outcome(Dismissal::NoPrompt)),
    };
    let (_, path) = reply;
    if path.as_str() == "/" {
        return None;
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
    completions: Option<MessageStream>,
}

impl PromptGuard<'_, '_, '_> {
    async fn settle(
        mut self,
        window: &str,
        wait: &Wait<'_>,
        #[cfg(test)] test: &TestOptions,
    ) -> Option<Outcome> {
        let result = catch_interactive(async {
            let stream = wait.run(self.peer.subscribe(&self.path)).await?;
            let completions = self.completions.insert(stream);
            wait.run(async {
                self.peer
                    .call::<_, ()>(self.path.as_str(), PROMPT, PROMPT_METHOD, &(window,))
                    .await?;
                #[cfg(test)]
                {
                    assert!(!test.panic_after_prompt, "scripted prompt unwind");
                    if test.cancel_with_queued_completion {
                        wait.cancellation.cancel();
                        std::future::pending::<()>().await;
                    }
                }
                completed(completions).await
            })
            .await
        })
        .await;
        match result {
            Ok(Ok(false)) => None,
            Ok(Ok(true)) => Some(Outcome::DismissedByUser),
            Ok(Err(Stop::Cancelled)) if self.approval_is_queued() => None,
            Ok(Err(stop)) => Some(stop.outcome(self.dismiss().await)),
            Err(()) => Some(Outcome::PromptMayStillBeOpen(self.dismiss().await)),
        }
    }

    fn approval_is_queued(&mut self) -> bool {
        self.completions
            .as_mut()
            .is_some_and(|stream| matches!(completed(stream).now_or_never(), Some(Ok(false))))
    }

    async fn dismiss(self) -> Dismissal {
        catch_interactive(self.peer.dismiss(&self.path, self.completions))
            .await
            .unwrap_or(Dismissal::Unacknowledged)
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

async fn completed(completions: &mut MessageStream) -> WireResult<bool> {
    let signal = completions
        .next()
        .await
        .ok_or(Failure::Unavailable)?
        .map_err(recovery_error)?;
    let (dismissed, _): (bool, OwnedVariant) =
        signal.body().deserialize().map_err(recovery_error)?;
    Ok(dismissed)
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
    fn outcome(self, dismissal: Dismissal) -> Outcome {
        match self {
            Self::Cancelled => Outcome::Cancelled(dismissal),
            Self::TimedOut => Outcome::TimedOut(dismissal),
            Self::Failed(_) if dismissal == Dismissal::Unacknowledged => {
                Outcome::PromptMayStillBeOpen(dismissal)
            }
            Self::Failed(code) => failed(code),
        }
    }
}

fn failed(code: Failure) -> Outcome {
    match code {
        Failure::Locked => Outcome::StillLocked,
        Failure::Unavailable => Outcome::Unavailable,
        Failure::Unsupported => Outcome::Unsupported,
        code => Outcome::Failed(code),
    }
}

#[cfg(test)]
#[path = "linux_secret_service_recovery_tests.rs"]
mod tests;
