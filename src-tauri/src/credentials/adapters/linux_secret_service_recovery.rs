//! Interactive policy, a private lexical child of the sealed recovery gate.
#![cfg_attr(not(test), allow(dead_code))] // The Linux factory remains dormant.

use super::{
    AllowPrompt, CancellationToken, ForbidPrompt, NativeInteractionGate, NativeRecoveryFacade,
    RecoveryBoundary, RecoveryFailure, RecoveryOutcome, RecoveryTarget,
};
use crate::credentials::adapters::linux_secret_service_wire::{
    LockedTarget, WireError, recovery_error, recovery_until,
};
use crate::credentials::domain::CredentialStoreFailure as Failure;
use futures_util::StreamExt;
use futures_util::future::{Either, select};
use serde::Serialize;
use std::future::Future;
use std::pin::pin;
use std::sync::{Arc, OnceLock};
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
const DISMISS_GRACE: Duration = Duration::from_millis(500);
const RETIRE_GRACE: Duration = Duration::from_millis(125);
const CANCELLATION_POLL: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Dismissal {
    NoPrompt,
    Completed,
    Unconfirmed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Outcome {
    Verified,
    DismissedByUser,
    Cancelled(Dismissal),
    TimedOut(Dismissal),
    StillLocked,
    Unavailable,
    Failed(Failure),
    DismissalUnconfirmed,
}

/// Dedicated worker only. The existing facade holds the lease, issues the
/// AllowPrompt capability, and invokes verification with a separate ForbidPrompt.
fn recover_exact(
    gate: &'static NativeInteractionGate,
    target: LockedTarget,
    window: Option<String>,
    budget: Duration,
    cancellation: &CancellationToken,
) -> Outcome {
    let Some(end) = Instant::now().checked_add(budget) else {
        return Outcome::Failed(Failure::Internal);
    };
    let result = Arc::new(OnceLock::new());
    let boundary = Boundary {
        target,
        window,
        end,
        result: Arc::clone(&result),
    };
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
            return Dismissal::Unconfirmed;
        };
        let settlement = async {
            let mut request = pin!(request);
            match select(request.as_mut(), pin!(completed(&mut completions))).await {
                Either::Left((_, completion)) => completion.await,
                Either::Right((result, _)) => result,
            }
        };
        if recovery_until(end, settlement).await.is_ok() {
            Dismissal::Completed
        } else {
            Dismissal::Unconfirmed
        }
    }
}

async fn interact(
    permission: &AllowPrompt<'_, '_>,
    target: &LockedTarget,
    window: &str,
    end: Instant,
    cancellation: &CancellationToken,
) -> Option<Outcome> {
    let wait = Wait { end, cancellation };
    let (connection, owner) = match wait.run(target.connect_for_recovery()).await {
        Ok(Some(peer)) => peer,
        Ok(None) => return None, // The target unlocked before dispatch.
        Err(stop) => return Some(stop.outcome(Dismissal::NoPrompt)),
    };
    let peer = Peer {
        _permission: permission,
        connection,
        owner,
    };
    let outcome = unlock(&peer, target, window, &wait).await;
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
) -> Option<Outcome> {
    let objects = [target.path()];
    let reply: UnlockReply = match wait
        .run(peer.call(SERVICE_PATH, SERVICE, "Unlock", &(objects.as_slice(),)))
        .await
    {
        Ok(reply) => reply,
        Err(stop) => return Some(stop.outcome(Dismissal::NoPrompt)),
    };
    let (_, path) = reply;
    if path.as_str() == "/" {
        return None;
    }
    let mut completions = match wait.run(peer.subscribe(&path)).await {
        Ok(stream) => stream,
        Err(stop) => return Some(stop.outcome(peer.dismiss(&path, None).await)),
    };
    let result = wait
        .run(async {
            peer.call::<_, ()>(path.as_str(), PROMPT, "Prompt", &(window,))
                .await?;
            completed(&mut completions).await
        })
        .await;
    // Peer (and its stream) are dropped before the facade can call verify.
    match result {
        Ok(false) => None,
        Ok(true) => Some(Outcome::DismissedByUser),
        Err(stop) => Some(stop.outcome(peer.dismiss(&path, Some(completions)).await)),
    }
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
            Self::Failed(_) if dismissal == Dismissal::Unconfirmed => Outcome::DismissalUnconfirmed,
            Self::Failed(code) => failed(code),
        }
    }
}

fn failed(code: Failure) -> Outcome {
    match code {
        Failure::Locked => Outcome::StillLocked,
        Failure::Unavailable => Outcome::Unavailable,
        code => Outcome::Failed(code),
    }
}

#[cfg(test)]
#[path = "linux_secret_service_recovery_tests.rs"]
mod tests;
