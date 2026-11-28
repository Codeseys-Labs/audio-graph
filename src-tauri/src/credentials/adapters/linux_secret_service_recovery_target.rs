//! Noninteractive lock evidence and a fresh, read-only verifier.

use super::*;
use crate::credentials::adapters::native_interaction::{ForbidPrompt, ensure_forbid_prompt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in super::super::super) enum RecoveryScope {
    Item,
    DefaultCollection,
}

/// Only a gated, exact locked lookup can create this target. It is bound to
/// one object on one daemon and deliberately has no printable representation.
pub(in super::super::super) struct LockedTarget {
    address: Address,
    owner: OwnedUniqueName,
    bus: zbus::OwnedGuid,
    path: OwnedObjectPath,
    attributes: Attributes,
    scope: RecoveryScope,
}

/// Dedicated worker only. An unlocked or absent match yields no unlock target.
#[cfg_attr(test, allow(dead_code))]
pub(in super::super::super) fn locked_target(
    prompt: &ForbidPrompt<'_>,
    attributes: Attributes,
    scope: RecoveryScope,
) -> WireResult<Option<LockedTarget>> {
    locked_target_at(session_address()?, prompt, attributes, scope)
}

pub(in super::super::super) fn locked_target_at(
    address: Address,
    prompt: &ForbidPrompt<'_>,
    attributes: Attributes,
    scope: RecoveryScope,
) -> WireResult<Option<LockedTarget>> {
    ensure_forbid_prompt(prompt)?;
    if attributes.is_empty() {
        return Err(Failure::Internal.into());
    }
    drive(DEADLINE, async |end| {
        until(end, async {
            let client = Client::connect(address.clone()).await?;
            let Some((path, true)) = candidate(&client, &attributes, scope).await? else {
                return Ok(None);
            };
            Ok(Some(LockedTarget {
                address,
                owner: client.owner,
                bus: client.connection.server_guid().clone(),
                path,
                attributes,
                scope,
            }))
        })
        .await
    })
}

impl LockedTarget {
    pub(in super::super::super) fn path(&self) -> &OwnedObjectPath {
        &self.path
    }

    /// A distinct connection. Revalidate the exact target before the policy
    /// layer can send an unlock, and never resolve a replacement target.
    pub(in super::super::super) async fn connect_for_recovery(
        &self,
    ) -> WireResult<Option<(Connection, OwnedUniqueName)>> {
        let client = self.connect().await?;
        let locked = self.current_lock(&client).await?;
        Ok(locked.then_some((client.connection, client.owner)))
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
        if client.owner != self.owner || client.connection.server_guid() != &self.bus {
            return Err(Failure::Unavailable.into());
        }
        Ok(client)
    }

    async fn current_lock(&self, client: &Client) -> WireResult<bool> {
        match candidate(client, &self.attributes, self.scope).await? {
            Some((path, locked)) if path == self.path => Ok(locked),
            _ => Err(Failure::RevisionConflict.into()),
        }
    }
}

async fn candidate(
    client: &Client,
    attributes: &Attributes,
    scope: RecoveryScope,
) -> WireResult<Option<(OwnedObjectPath, bool)>> {
    let (path, interface) = match scope {
        RecoveryScope::Item => {
            let Some((path, was_locked)) = client.exact_candidate(attributes, false).await? else {
                return Ok(None);
            };
            if was_locked {
                return Ok(Some((path, true)));
            }
            (path, ITEM)
        }
        RecoveryScope::DefaultCollection => {
            let path = client
                .default_collection()
                .await?
                .ok_or(Failure::Unavailable)?;
            (path, COLLECTION)
        }
    };
    let locked = client.locked(&path, interface).await?;
    Ok(Some((path, locked)))
}
