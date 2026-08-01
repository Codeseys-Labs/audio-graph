# AudioGraph

AudioGraph turns live or recorded speech into durable notes and a temporal
knowledge graph while keeping provider authority explicit.

## Credential language

**Credential Set**:
A logical authentication bundle that changes, rotates, and disappears as one generation.
_Avoid_: Credential field, key slot

**Built-in Credential Set**:
A credential set with a stable AudioGraph-defined identity for a first-class provider.
_Avoid_: Provider key name

**Custom Credential Set**:
A backend-issued credential set bound to one custom secure network origin.
_Avoid_: Custom key slot, user-named credential

**Authentication Method**:
The mechanism by which a credential set authenticates, independent of provider identity.
_Avoid_: Provider, credential set

**Credential Field**:
One member or related setting of a credential set's authentication choice.
_Avoid_: Credential set

**Secret**:
Authentication material whose presence may establish a configured credential set.
_Avoid_: Private locator, configuration

**Private Locator**:
A sensitive reference to authentication material held elsewhere; it is not a stored secret or credential presence.
_Avoid_: Secret, credential

**Ordinary Configuration**:
Non-secret provider selection or routing information that does not establish credential presence.
_Avoid_: Credential, secret

**Credential Presence**:
The passive claim that a complete secret-bearing credential set is configured, without revealing or validating its contents.
_Avoid_: Valid credential, private-locator presence

**Credential Purpose**:
The backend-owned reason a credential may be used, such as transcription or language-model inference.
_Avoid_: Provider stage

**Credential Audience**:
The exact secure network origin or typed provider SDK destination authorized to receive a credential.
_Avoid_: Endpoint string, provider label

**Credential Use Policy**:
One atomic authorization or explicit denial binding a credential set, consumer, authentication method, purpose, audience, and active-use action.
_Avoid_: Independent allowlists, provider-wide authority

**Credential Revision**:
An opaque identity for one committed generation of a credential set, used only for equality and concurrency control.
_Avoid_: Version number, fingerprint

**Status Epoch**:
A monotonically increasing order for credential-service status snapshots across all sets.
_Avoid_: Credential revision

**Tombstone**:
An authoritative retained statement that a credential set is deleted and must not be resurrected from older material.
_Avoid_: Missing credential

**Pending Activation**:
A credential-and-settings change that is not yet authoritative for provider use.
_Avoid_: Saved credential

**Active-use Action**:
The declared response a credential consumer takes when its credential generation changes.
_Avoid_: Revocation
