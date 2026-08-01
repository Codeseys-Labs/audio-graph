---
status: proposed
date: 2026-07-31
deciders: [AudioGraph maintainers]
---

# ADR-0035: Rebuild Credentials as a Backend-Owned Typed Secret Service

## Context and Problem Statement

ADR-0019 correctly selected OS-native secret storage behind a Rust facade, but
its first implementation retained a flat field store and made legacy YAML part
of live precedence and fallback. The result is safer at rest than the original
plaintext-only design, but it is not a complete authority, migration, or
runtime-lifecycle boundary.

The current module stores 22 independent scalar fields, reconstructs a full
plaintext snapshot, and hydrates that snapshot into long-lived application
settings. Presence reads can migrate or rewrite storage. Backend failures can
become an empty store. AWS rotation can mix generations. Legacy inline OpenAI
Realtime auth is redacted without being imported. The file fallback can write
after Windows ACL hardening fails.

Most importantly, registered renderer commands accept caller-selected provider
endpoints. When a replacement draft is absent, Rust can infer a stored key from
the URL and attach it to the request. Substring matching and a generic OpenAI
fallback allow an untrusted renderer to cause saved-key egress to an arbitrary
server without receiving the plaintext over IPC. The credential boundary must
therefore govern use and audience, not only storage and readback.

## Decision Drivers

- A compromised renderer must not read a saved secret or use Rust as a
  credential-forwarding oracle.
- Long-lived sockets, SDK clients, and refresh workers remain backend-owned.
- First-class and custom endpoints need explicit, testable audience policy.
- Multi-field auth must rotate and delete as one logical generation.
- Native-store failures must remain distinct from missing credentials and must
  never silently activate plaintext fallback.
- Passive UI status must not write, prompt, migrate, or contact a provider.
- Existing v1 keychain, YAML, and inline-setting users need lossless,
  idempotent migration and anti-resurrection behavior.
- Windows, macOS, Linux, headless tests, and explicit development fallback need
  honest, separately evidenced behavior.
- A display rename must not orphan secrets or add a second migration axis.

## Considered Options

- Patch the current field store and retain live YAML precedence
- Replace only the current keyring facade while keeping whole-store snapshots
- Store the entire application secret set in one native entry
- Store one scalar per native entry and coordinate bundles in application code
- Build a typed backend service with one native entry per logical bundle
- Adopt an application-managed encrypted vault as the production default

## Decision Outcome

Chosen option: **build a typed backend service with one native entry per logical
credential bundle**.

ADR-0035 supersedes the credential-storage, fallback, and migration portions of
ADR-0019. ADR-0019's separate `ConfigCodec` decision remains proposed and is not
part of this rebuild.

### Domain and authority

The public contract uses closed `CredentialSetId`, `AuthMethodId`,
`CredentialPurpose`, and `CredentialAudience` types. It does not accept an
arbitrary field name as the unit of authority.

- One provider API key is one logical set.
- OpenAI first-party use may share one set across its explicitly declared
  ASR/LLM/TTS/realtime purposes, but only for OpenAI-owned HTTPS origins.
- An OpenAI-compatible custom endpoint gets a separate credential set bound to
  an exact normalized HTTPS origin. It never inherits `openai_api_key` or any
  provider-specific key by URL substring or fallback.
- AWS access-key id, secret access key, and optional session token form one
  logical set. AWS profile and region remain non-secret settings.
- A Google service-account path is a private locator in settings, not a secret
  value in the credential store. The JSON file is opened only by backend-owned
  Vertex auth code.

Every stored-secret resolution names the set, auth method, backend-owned
purpose, and normalized audience. Policy is checked before the native record is
read. Saved credentials require HTTPS. A loopback HTTP development endpoint may
use an ephemeral draft, not a stored credential. HTTP clients disable redirects
or re-authorize every hop without forwarding authorization cross-origin.

### Service lifecycle

AudioGraph owns one long-lived Rust `CredentialService` in application state.
It exposes distinct operations:

- side-effect-free `snapshot_status`;
- explicit `initialize` and `migrate_legacy`;
- `replace_set` and `delete_set` with expected revision;
- authorized `resolve_for_use` returning a scoped zeroizing lease;
- explicit, possibly interactive `diagnose_or_unlock`; and
- recovery/cleanup operations with typed receipts.

Native calls run on one serialized blocking worker rather than Tauri's async/UI
executor. Startup, status, readiness, and background probes use
`ForbidPrompt`. Only a user-initiated unlock/recovery action may use
`AllowPrompt`.

No full plaintext credential snapshot is hydrated into `AppSettings`. Provider
transport construction resolves only the set it needs. Each lease carries the
committed revision. A successful replace/delete publishes a typed change event:
new HTTP work uses the new revision, and backend-owned long-lived consumers stop
or reauthenticate according to their declared lifecycle policy. The application
does not claim to revoke requests already accepted by a remote provider.

### Native persistence contract

Production uses `keyring-core` with explicit platform adapters rather than the
automatic `keyring::Entry` v1 facade:

- Windows: Credential Manager generic credentials with an explicit
  company-prefixed target and **Local** persistence.
- macOS: the user/login Keychain through the selected legacy keychain adapter;
  moving to the data-protection keychain requires a separate ADR and signed
  migration evidence.
- Linux: a thin Secret Service adapter that can exact-match without
  automatically unlocking or prompting during background work.

The immutable logical service namespace is
`com.codeseys.audiograph.credentials`; accounts are `v2/<credential-set-id>`.
Windows uses an equivalent explicit target
`Codeseys.AudioGraph.Credentials/v2/<credential-set-id>`. These identifiers and
the Tauri bundle id do not derive from the display brand.

Each logical set is encoded as a versioned UTF-8 JSON envelope and stored via
the binary `set_secret`/`get_secret` API. The envelope contains its schema,
random 128-bit revision, typed discriminator, and only the fields belonging to
that bundle. The final encoded value must be no larger than **2,560 bytes**, the
portable Windows generic-credential blob ceiling. Oversize values are rejected
before any native call; the service never truncates, splits, or silently moves
them to plaintext. A future bundle that cannot fit needs a new secure-backend
decision.

Replace uses one native-entry boundary:

1. validate and encode;
2. enforce the payload limit;
3. replace the entry;
4. read back, parse, and verify the exact revision; and
5. publish the committed revision and change event.

If verification cannot establish the new revision, return `commit_unknown` and
do not publish success. This prevents field-level mixed generations. It does
not claim an ACID transaction across power failure, other processes, native
sync, or migration metadata.

### Status, errors, and concurrency

Passive status is journal-backed and revisioned. It reports backend kind,
last-established availability, migration/cleanup state, and configured or
tombstoned sets without reading every secret. External native-store edits are
not a supported live-update path; an explicit reconcile/diagnose operation may
probe exact known locators.

The service uses a closed, content-free error model. At minimum it preserves
missing, locked, access denied, cancelled, unavailable, unsupported store,
corrupt record, unsupported schema, oversized payload, ambiguous match,
migration required/conflict, revision conflict, insecure transport, audience
denied, permission-hardening failure, commit unknown, and internal backend
failure. Only native item-not-found becomes `missing`. Raw native errors and
raw bytes carried by decoding errors are never formatted into IPC, logs,
analytics, docs, or Seeds.

Mutation receipts contain operation id, credential-set id, previous/new
revision where applicable, result code, and safe recovery action. They contain
no value, exact length, fingerprint, private locator, or native error prose.

### File and test backends

Native storage is the only automatic production backend. Native locked,
denied, unavailable, cancelled, and failure results do not cause YAML access.

- Unit/contract tests use an injected in-memory backend with deterministic
  failure and interruption scripts.
- A new file-v2 backend may be selected explicitly for headless development or
  recovery. It has a separate path/format from `credentials.yaml`, creates
  owner-only storage before writing bytes, fails closed when permissions cannot
  be established, uses file and parent-directory sync plus a single-writer
  lock, and reports a persistent degraded-security status.
- `credentials.yaml` is import-only. It is never a live v2 fallback or
  override.

### Migration and rollback

An explicit, idempotent migration inventories exact known v1 keychain locators,
legacy YAML, legacy inline settings, and prior v1 state. It does not enumerate a
user's general keychain and does not run from `snapshot_status`.

- Different live candidate values create a non-secret conflict requiring an
  explicit source choice.
- A v2 intent and tombstone/authority journal is durable before destructive
  cleanup.
- The v2 bundle is read back and byte/revision verified before inline settings
  are redacted or a legacy source is quarantined/deleted.
- OpenAI Realtime inline auth is included in the migration fixture matrix.
- Once a v2 set is committed or tombstoned, legacy sources cannot silently
  resurrect it.
- Normal v2 mutation is not dual-written to v1. Retained legacy material is
  quarantined, permission-hardened, and visibly pending cleanup; it is never a
  normal read source.

Rollback before migration activation is branch/config rollback. After a v2
mutation, an old binary may require credential re-entry because it cannot read
the v2 namespace; preserving silent v1 dual-write would violate rotation and
deletion guarantees. Migration cleanup therefore explains downgrade risk and
requires packaged upgrade evidence before release.

### Settings activation

One credential bundle can commit atomically at the native-entry boundary;
native storage and `config.yaml` cannot form a distributed transaction. UI
flows use staged activation:

1. validate the full secret bundle and non-secret settings draft;
2. commit the bundle with an expected revision;
3. persist settings;
4. activate the runtime configuration only after both commits succeed; and
5. if settings persistence fails, report a dormant saved credential and offer a
   retry or revision-checked compensation action.

A dormant credential is safer than active settings selecting a missing or
partially written bundle. Silent rollback is forbidden because another window
may have changed the same revision.

### Consequences

- **Positive**: Hiding saved values and authorizing their use become one
  backend-owned security boundary.
- **Positive**: AWS and future multi-field auth cannot expose mixed field
  generations through ordinary service mutation.
- **Positive**: Background status is fast, prompt-free, side-effect free, and
  coherent by revision.
- **Positive**: Native-store failure and missing credentials produce different
  user recovery paths.
- **Positive**: Display branding can evolve without changing secret identity.
- **Negative**: Existing consumers and settings flows need coordinated
  migration; a storage-only patch is insufficient.
- **Negative**: Linux needs a thinner Secret Service adapter than the current
  high-level auto-prompting path.
- **Negative**: The 2,560-byte portable bundle limit must be checked for every
  new auth method.
- **Negative**: An old binary cannot safely observe v2-only rotations after v1
  cleanup, so downgrade may require credential re-entry.
- **Neutral**: Ephemeral user-entered drafts still exist briefly in renderer
  memory. Native secure-entry UI is outside this decision.

## Pros and Cons of the Options

### Patch the current field store

- Good, because it minimizes code churn.
- Bad, because field drift, whole-store snapshots, hidden writes, live YAML,
  confused-deputy egress, and runtime lifetime remain architectural.

### Replace only the keyring facade

- Good, because platform error handling improves.
- Bad, because storage mechanics alone do not define audience authority,
  logical bundles, migration, IPC, or active-session invalidation.

### One native entry for all application secrets

- Good, because one replace cannot mix providers.
- Bad, because unrelated mutations share one blast radius and the Windows
  2,560-byte ceiling is immediately fragile.

### One entry per scalar

- Good, because small independent API keys are simple.
- Bad, because AWS and future OAuth bundles can partially rotate/delete and
  readers can combine generations.

### Typed service with one entry per logical bundle

- Good, because the persistence unit matches rotation, deletion, resolution,
  and audience policy.
- Good, because it supports injected deterministic backends and typed IPC.
- Bad, because it requires a coordinated backend, provider, migration, and UI
  cutover.

### Application-managed encrypted vault

- Good, because it can support large records and cross-platform uniformity.
- Bad, because it introduces a wrapping-key/unlock/backup UX the product has
  not designed and merely moves key custody if no user secret is involved.
- Neutral, because it remains a future option if a real bundle exceeds the
  native portable limit.

## Verification and Release Evidence

Deterministic contract tests must cover codec/size boundaries, error mapping,
prompt prohibition, revision conflicts, commit-unknown readback, AWS generation
consistency, migration interruption at every persistence cut point, legacy
anti-resurrection, content-free logs/IPC, and hostile endpoint/redirect cases.

Release claims require packaged Windows, macOS, and Linux save/read/replace/
delete/restart tests. They must include stable-identity upgrades, v1 migration,
locked/denied/cancelled/unavailable behavior where supported, Windows Local
persistence and 2,560-byte boundary, signed macOS upgrade behavior, and Linux
Secret Service locked/absent-session behavior. Compile-only CI and the existing
ignored native smoke are not release proof.

## More Information

- Threat model: `docs/security/credential-service-threat-model.md`
- Discovery synthesis:
  `docs/agentic-runs/2026-07-31-credential-service-rebuild/discovery-synthesis.md`
- Implementation plan: `docs/plans/2026-07-31-credential-service-rebuild.md`
- Native-store research:
  `/tmp/audio-graph-credential-discovery/native-store-research.md`
- Existing decision: ADR-0019
- Tracking: `audio-graph-a0f6`, `audio-graph-efeb`,
  `audio-graph-cffc`, `audio-graph-f70b`, `audio-graph-873d`, and
  `audio-graph-c420`

Primary platform references used for this decision:

- `keyring-core` API and error model:
  https://docs.rs/keyring-core/1.0.0/keyring_core/
- Windows generic credential shape and blob/persistence limits:
  https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw
- Apple keychain model:
  https://developer.apple.com/documentation/security/keychain-services
- Freedesktop Secret Service specification:
  https://specifications.freedesktop.org/secret-service/latest-single/
