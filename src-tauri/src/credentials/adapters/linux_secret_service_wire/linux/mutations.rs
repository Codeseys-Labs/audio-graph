//! Noninteractive writes and deletes, with a fresh read-back after dispatch.

use super::{
    Address, Attributes, CLOSE_BUDGET, Client, Failure, ITEM, Method, OwnedObjectPath, SecretBytes,
    Variant, WireError, WireResult, drive, map_crypto_error, session_address, until,
};
use crate::credentials::adapters::native_interaction::{
    ForbidPrompt, MutationInvocation, ensure_forbid_prompt,
};
use std::collections::HashMap;
use std::time::Duration;
use zeroize::Zeroizing;

const COLLECTION: &str = "org.freedesktop.Secret.Collection";

// One ten-second deadline covers preparation, dispatch, dismissal, both
// session closes and fresh verification. Half is reserved for cleanup and
// reconciliation, so timing out after dispatch is never a plain timeout.
#[cfg_attr(test, allow(dead_code))]
const MUTATION_DEADLINE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super::super) enum MutationOutcome {
    Committed,
    NotCommitted(Failure),
    InteractionRequired,
}

pub(super) enum Mutation {
    Write(Zeroizing<Vec<u8>>),
    Delete,
}

/// Only the dedicated credential worker may call this blocking facade.
#[cfg_attr(test, allow(dead_code))] // Tests inject the private-bus address.
pub(in super::super::super) fn write_exact(
    prompt: &ForbidPrompt<'_>,
    invocation: &mut MutationInvocation<'_>,
    attributes: &Attributes,
    secret: Zeroizing<Vec<u8>>,
) -> WireResult<MutationOutcome> {
    mutate_at(
        session_address()?,
        prompt,
        invocation,
        attributes,
        Mutation::Write(secret),
        MUTATION_DEADLINE,
        #[cfg(test)]
        None,
    )
}

/// Only the dedicated credential worker may call this blocking facade.
#[cfg_attr(test, allow(dead_code))] // Tests inject the private-bus address.
pub(in super::super::super) fn delete_exact(
    prompt: &ForbidPrompt<'_>,
    invocation: &mut MutationInvocation<'_>,
    attributes: &Attributes,
) -> WireResult<MutationOutcome> {
    mutate_at(
        session_address()?,
        prompt,
        invocation,
        attributes,
        Mutation::Delete,
        MUTATION_DEADLINE,
        #[cfg(test)]
        None,
    )
}

pub(super) fn mutate_at(
    address: Address,
    prompt: &ForbidPrompt<'_>,
    invocation: &mut MutationInvocation<'_>,
    attributes: &Attributes,
    mutation: Mutation,
    timeout: Duration,
    #[cfg(test)] interruption: Option<DispatchInterruption>,
) -> WireResult<MutationOutcome> {
    ensure_forbid_prompt(prompt)?;
    invocation.prepare()?;
    if attributes.is_empty() {
        return Err(Failure::Internal.into());
    }

    drive(timeout, async |end| {
        let dispatch_by = end - timeout / 2;
        let mut client = until(dispatch_by, Client::connect(address.clone())).await?;
        let plan = until(dispatch_by, prepare(&mut client, attributes, &mutation)).await;
        let plan = match plan {
            Ok(Some(plan)) => plan,
            result => {
                client.close_session(dispatch_by + CLOSE_BUDGET).await;
                return result.map(|_| MutationOutcome::Committed);
            }
        };

        let mut started = false;
        let send = send(&client, &plan, attributes, invocation, &mut started);
        #[cfg(test)]
        let send = interrupt_send(&client, interruption, send);
        let reply = until(dispatch_by, send).await;
        let evidence = match reply {
            Ok(None) => Dispatch::Acknowledged,
            Ok(Some(prompt_path)) => Dispatch::Prompt(
                until(dispatch_by, client.dismiss_mutation_prompt(&prompt_path))
                    .await
                    .ok(),
            ),
            Err(error) if !started => {
                client.close_session(dispatch_by + CLOSE_BUDGET).await;
                return Err(error);
            }
            Err(WireError(
                reason @ (Failure::Locked | Failure::Missing | Failure::AccessDenied),
            )) => Dispatch::Rejected(reason),
            Err(_) => Dispatch::Uncertain,
        };

        let owner = client.owner().clone();
        let bus = client.connection.server_guid().clone();
        client.close_session(dispatch_by + CLOSE_BUDGET).await;
        drop(client);

        let read_by = end - CLOSE_BUDGET;
        let after = async {
            let mut verifier = until(read_by, Client::connect(address)).await?;
            if verifier.owner() != &owner || verifier.connection.server_guid() != &bus {
                return Err(Failure::Unavailable.into());
            }
            let after = until(read_by, snapshot(&mut verifier, attributes, &mutation)).await;
            verifier.close_session(end).await;
            after
        }
        .await;

        reconcile(evidence, &plan, &mutation, after)
            .ok_or_else(|| invocation.latch_commit_unknown().into())
    })
}

#[cfg(test)]
pub(super) enum DispatchInterruption {
    ExpireBeforeSend,
    Disconnect(tokio::sync::oneshot::Receiver<()>),
}

#[cfg(test)]
async fn interrupt_send(
    client: &Client,
    interruption: Option<DispatchInterruption>,
    send: impl std::future::Future<Output = WireResult<Option<OwnedObjectPath>>>,
) -> WireResult<Option<OwnedObjectPath>> {
    use futures_util::future::{Either, select};
    use std::pin::pin;

    match interruption {
        None => send.await,
        Some(DispatchInterruption::ExpireBeforeSend) => {
            until(std::time::Instant::now(), send).await
        }
        Some(DispatchInterruption::Disconnect(observed)) => {
            let disconnect = async {
                observed.await.expect("peer observed the mutation");
                client.connection.disconnect().await;
                std::future::pending::<()>().await;
            };
            match select(pin!(send), pin!(disconnect)).await {
                Either::Left((result, _)) => result,
                Either::Right(_) => unreachable!(),
            }
        }
    }
}

struct Snapshot {
    item: OwnedObjectPath,
    secret: Option<SecretBytes>,
}

enum Request {
    Write {
        collection: OwnedObjectPath,
        iv: [u8; 16],
        ciphertext: Vec<u8>,
    },
    Delete,
}

struct Plan {
    before: Option<Snapshot>,
    request: Request,
}

async fn prepare(
    client: &mut Client,
    attributes: &Attributes,
    mutation: &Mutation,
) -> WireResult<Option<Plan>> {
    let item = client.find_exact(attributes, false).await?;
    let collection = match mutation {
        Mutation::Write(_) => {
            let collection = client
                .default_collection()
                .await?
                .ok_or(Failure::Unavailable)?;
            if let Some(item) = &item
                && !belongs_to(item, &collection)
            {
                return Err(Failure::RevisionConflict.into());
            }
            client
                .require_unlocked(&collection, COLLECTION)
                .await
                .map_err(super::unavailable_if_missing)?;
            Some(collection)
        }
        Mutation::Delete if item.is_none() => return Ok(None),
        Mutation::Delete => None,
    };
    if let Some(item) = &item {
        client.require_unlocked(item, ITEM).await?;
    }
    let (before, request) = match mutation {
        Mutation::Write(plaintext) => {
            let key = client.open_session().await?;
            let before = match item {
                Some(item) => Some(Snapshot {
                    secret: Some(client.get_secret(&item, attributes, &key).await?),
                    item,
                }),
                None => None,
            };
            let (iv, ciphertext) = key.encrypt(plaintext).map_err(map_crypto_error)?;
            (
                before,
                Request::Write {
                    collection: collection.ok_or(Failure::Internal)?,
                    iv,
                    ciphertext,
                },
            )
        }
        Mutation::Delete => {
            let item = item.ok_or(Failure::Internal)?;
            if client.attributes(&item).await? != *attributes {
                return Err(Failure::RevisionConflict.into());
            }
            (Some(Snapshot { item, secret: None }), Request::Delete)
        }
    };
    Ok(Some(Plan { before, request }))
}

fn belongs_to(item: &OwnedObjectPath, collection: &OwnedObjectPath) -> bool {
    item.as_str()
        .rsplit_once('/')
        .is_some_and(|(parent, _)| parent == collection.as_str())
}

async fn snapshot(
    client: &mut Client,
    attributes: &Attributes,
    mutation: &Mutation,
) -> WireResult<Option<Snapshot>> {
    let Some(item) = client.find_exact(attributes, false).await? else {
        return Ok(None);
    };
    let secret = match mutation {
        Mutation::Write(_) => Some(client.read_unlocked_candidate(&item, attributes).await?),
        Mutation::Delete => {
            client.require_unlocked(&item, ITEM).await?;
            None
        }
    };
    Ok(Some(Snapshot { item, secret }))
}

async fn send(
    client: &Client,
    plan: &Plan,
    attributes: &Attributes,
    invocation: &mut MutationInvocation<'_>,
    started: &mut bool,
) -> WireResult<Option<OwnedObjectPath>> {
    client.connection.require_current().await?;
    let prompt = match (&plan.request, &plan.before) {
        (Request::Write { iv, ciphertext, .. }, Some(before)) => {
            let secret = (
                client.session_path()?,
                iv.as_slice(),
                ciphertext,
                "application/octet-stream",
            );
            *started = true;
            invocation.mark_started();
            client
                .call::<_, ()>(Method::SetSecret(&before.item), &(secret,))
                .await?;
            return Ok(None);
        }
        (
            Request::Write {
                collection,
                iv,
                ciphertext,
            },
            None,
        ) => {
            let secret = (
                client.session_path()?,
                iv.as_slice(),
                ciphertext,
                "application/octet-stream",
            );
            // Exact attributes were verified before dispatch. CreateItem must
            // never do a second, daemon-specific replacement lookup.
            let properties = HashMap::from([
                (
                    "org.freedesktop.Secret.Item.Label",
                    Variant::from("AudioGraph credential"),
                ),
                (
                    "org.freedesktop.Secret.Item.Attributes",
                    Variant::from(attributes.iter().collect::<HashMap<_, _>>()),
                ),
            ]);
            *started = true;
            invocation.mark_started();
            let (item, prompt): (OwnedObjectPath, OwnedObjectPath) = client
                .call(Method::CreateItem(collection), &(properties, secret, false))
                .await?;
            if item.as_str() == "/" && prompt.as_str() == "/" {
                return Err(Failure::Internal.into());
            }
            prompt
        }
        (Request::Delete, Some(before)) => {
            *started = true;
            invocation.mark_started();
            client.call(Method::DeleteItem(&before.item), &()).await?
        }
        (Request::Delete, None) => return Err(Failure::Internal.into()),
    };
    Ok((prompt.as_str() != "/").then_some(prompt))
}

enum Dispatch {
    Acknowledged,
    Uncertain,
    Rejected(Failure),
    Prompt(Option<bool>),
}

fn reconcile(
    evidence: Dispatch,
    plan: &Plan,
    mutation: &Mutation,
    after: WireResult<Option<Snapshot>>,
) -> Option<MutationOutcome> {
    match (&evidence, &after) {
        (Dispatch::Rejected(Failure::Locked), Err(WireError(Failure::Locked))) => {
            return Some(MutationOutcome::NotCommitted(Failure::Locked));
        }
        (Dispatch::Rejected(Failure::Missing), Ok(None)) => {
            let reason = if plan.before.is_some() {
                Failure::Missing
            } else {
                Failure::Unavailable
            };
            return Some(MutationOutcome::NotCommitted(reason));
        }
        _ => {}
    }
    let after = after.ok()?;
    match evidence {
        Dispatch::Acknowledged | Dispatch::Prompt(Some(false))
            if desired_state(plan, mutation, after.as_ref()) =>
        {
            Some(MutationOutcome::Committed)
        }
        Dispatch::Uncertain
            if desired_state(plan, mutation, after.as_ref())
                && !same_state(plan.before.as_ref(), after.as_ref()) =>
        {
            Some(MutationOutcome::Committed)
        }
        Dispatch::Rejected(reason) if same_state(plan.before.as_ref(), after.as_ref()) => {
            // KWallet also returns UnknownObject when SetSecret cannot find
            // its session. Here a fresh read proved the item still exists.
            let reason = if reason == Failure::Missing && matches!(mutation, Mutation::Write(_)) {
                Failure::Unavailable
            } else {
                reason
            };
            Some(MutationOutcome::NotCommitted(reason))
        }
        Dispatch::Prompt(Some(_)) if same_state(plan.before.as_ref(), after.as_ref()) => {
            Some(MutationOutcome::InteractionRequired)
        }
        // A prior state observed after a lost reply cannot exclude a late
        // write. A dismissed prompt also needs confirmation of the prior state.
        _ => None,
    }
}

fn same_state(before: Option<&Snapshot>, after: Option<&Snapshot>) -> bool {
    match (before, after) {
        (None, None) => true,
        (Some(before), Some(after)) => {
            before.item == after.item
                && before.secret.as_ref().map(|secret| secret.0.as_slice())
                    == after.secret.as_ref().map(|secret| secret.0.as_slice())
        }
        _ => false,
    }
}

fn desired_state(plan: &Plan, mutation: &Mutation, after: Option<&Snapshot>) -> bool {
    match (&plan.request, mutation, after) {
        (Request::Write { collection, .. }, Mutation::Write(expected), Some(actual)) => {
            belongs_to(&actual.item, collection)
                && plan
                    .before
                    .as_ref()
                    .is_none_or(|before| before.item == actual.item)
                && actual
                    .secret
                    .as_ref()
                    .is_some_and(|secret| secret.0.as_slice() == expected.as_slice())
        }
        (Request::Delete, Mutation::Delete, None) => true,
        _ => false,
    }
}
