//! Noninteractive lock evidence and a fresh, read-only verifier.

use super::*;
use crate::credentials::adapters::keyring_entry::EntryLocator;
use crate::credentials::adapters::native_interaction::{ForbidPrompt, ensure_forbid_prompt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecoveryScope {
    Item,
    DefaultCollection,
}

/// Only a gated, exact locked lookup can create this target. It is bound to
/// one object on one daemon and deliberately has no printable representation.
pub(in super::super::super) struct LockedTarget {
    address: Address,
    owner: ServiceOwner,
    bus: OwnedGuid,
    path: OwnedObjectPath,
    attributes: Attributes,
    scope: RecoveryScope,
}

/// Dedicated worker only. Accept only locators minted by the application's
/// credential adapter. Use the default collection only for an absent item.
#[cfg_attr(test, allow(dead_code))]
pub(in super::super::super) fn locked_target(
    prompt: &ForbidPrompt<'_>,
    locator: &EntryLocator,
) -> WireResult<Option<LockedTarget>> {
    locked_target_at(session_address()?, prompt, locator)
}

pub(in super::super::super) fn locked_target_at(
    address: Address,
    prompt: &ForbidPrompt<'_>,
    locator: &EntryLocator,
) -> WireResult<Option<LockedTarget>> {
    ensure_forbid_prompt(prompt)?;
    let attributes = attributes_for(locator);
    drive(DEADLINE, async |end| {
        until(end, async {
            let client = Client::connect(address.clone()).await?;
            let (path, scope, locked) = candidate(&client, &attributes).await?;
            if !locked {
                return Ok(None);
            }
            Ok(Some(LockedTarget {
                address,
                owner: client.owner().clone(),
                bus: client.connection.server_guid().clone(),
                path,
                attributes,
                scope,
            }))
        })
        .await
    })
}

pub(in super::super::super) fn attributes_for(locator: &EntryLocator) -> Attributes {
    Attributes::from([
        (
            "xdg:schema".to_owned(),
            "com.codeseys.audiograph.credentials.v2".to_owned(),
        ),
        ("service".to_owned(), locator.service().to_owned()),
        ("account".to_owned(), locator.account().to_owned()),
    ])
}

impl LockedTarget {
    pub(in super::super::super) fn path(&self) -> &OwnedObjectPath {
        &self.path
    }

    pub(super) async fn check_recovery_connection(
        &self,
        client: &Client,
    ) -> WireResult<(ServiceOwner, bool)> {
        self.check_binding(client)?;
        let locked = self.current_lock(client).await?;
        Ok((client.owner().clone(), locked))
    }

    /// Verification is a distinct five-second transaction. It consumes
    /// encrypted read-back internally and never exposes the plaintext.
    pub(in super::super::super) fn verify(&self, prompt: &ForbidPrompt<'_>) -> WireResult<()> {
        ensure_forbid_prompt(prompt)?;
        drive(DEADLINE, async |end| {
            let read_by = end - CLOSE_BUDGET;
            let mut client = until(read_by, self.connect()).await?;
            let result = until(read_by, async {
                if self.current_lock(&client).await? {
                    return Err(Failure::Locked.into());
                }
                match self.scope {
                    RecoveryScope::Item => {
                        client
                            .read_unlocked_candidate(&self.path, &self.attributes)
                            .await?;
                    }
                    RecoveryScope::DefaultCollection => {
                        client.read_exact(&self.attributes).await?;
                    }
                }
                Ok(())
            })
            .await;
            client.close_session(end).await;
            result
        })
    }

    async fn connect(&self) -> WireResult<Client> {
        let client = Client::connect(self.address.clone()).await?;
        self.check_binding(&client)?;
        Ok(client)
    }

    fn check_binding(&self, client: &Client) -> WireResult<()> {
        if client.owner() != &self.owner || client.connection.server_guid() != &self.bus {
            return Err(Failure::Unavailable.into());
        }
        Ok(())
    }

    async fn current_lock(&self, client: &Client) -> WireResult<bool> {
        let (path, scope, locked) = candidate(client, &self.attributes).await?;
        if path == self.path && scope == self.scope {
            Ok(locked)
        } else {
            Err(Failure::RevisionConflict.into())
        }
    }
}

async fn candidate(
    client: &Client,
    attributes: &Attributes,
) -> WireResult<(OwnedObjectPath, RecoveryScope, bool)> {
    if let Some((path, was_locked)) = client.exact_candidate(attributes, false).await? {
        let locked = was_locked || client.locked(&path, ITEM).await?;
        return Ok((path, RecoveryScope::Item, locked));
    }
    let path = client
        .default_collection()
        .await?
        .ok_or(Failure::Unavailable)?;
    let locked = client.locked(&path, COLLECTION).await?;
    Ok((path, RecoveryScope::DefaultCollection, locked))
}
