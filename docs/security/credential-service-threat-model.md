# Credential service threat model

Date: 2026-07-31

Owner: `audio-graph-a0f6`

Decision: ADR-0035

## Scope

This model covers saved provider authentication material from user entry or
legacy import through native persistence, passive status, runtime resolution,
rotation, deletion, and cleanup. It also covers the authority to attach a
saved credential to network traffic.

It does not claim to protect secrets from an administrator/root account, a
fully compromised logged-in OS account, a malicious provider that legitimately
receives its own credential, or plaintext the user deliberately pastes into a
renderer input before submission. Ephemeral replacement drafts in renderer
memory are accepted for the desktop form UX; saved-secret readback is not.

## Assets

- Provider API keys, AWS access/secret/session material, and future OAuth
  refresh material.
- Correct grouping: fields from different revisions must never be combined.
- Authority: which provider purpose and network origin may receive a secret.
- Availability and integrity of native-store records and migration metadata.
- Non-secret but sensitive metadata, including credential presence, source,
  private locator paths, secret length, and stable fingerprints.
- Deletion intent, including protection against resurrection from a legacy
  source or older generation.

## Trust boundaries and actors

| Boundary or actor | Assumption | Required control |
| --- | --- | --- |
| Main-window renderer, XSS, or compromised frontend dependency | May invoke every command exposed to its capability; cannot be trusted with stored plaintext or destination authority. | No saved-secret readback; closed IPC types; backend-owned policy; exact purpose/audience checks; no free-form backend errors. |
| User-entered replacement draft | Plaintext exists briefly in a password input and IPC request. | Never persist in frontend storage, analytics, screenshots, error objects, or logs; clear on settle/unmount; backend validates bounds before storage. |
| Custom or remote endpoint | May be renderer-selected and therefore cannot infer or retarget a saved credential. | Built-in saved use requires an exact backend-owned HTTPS/WSS origin; custom endpoints are draft-only until v2 stores the exact binding inside the protected bundle; rebinding requires a new backend-issued set plus the secret; redirects disabled; no URL substring inference. |
| Rust provider/runtime code | Trusted to use a credential only through the service contract. | Resolve by typed purpose and audience; scoped zeroizing lease; revision tracking; mutation invalidation. |
| Native credential store | Trusted for at-rest protection within its documented OS/user boundary; can be locked, unavailable, denied, or prompt. | Typed adapter errors; blocking calls off async threads; no silent fallback; packaged-platform proof. |
| Legacy YAML and v1 keychain entries | Untrusted migration inputs that may conflict, be permissive, stale, or malformed. | Explicit import transaction; source inventory; verification; tombstones; quarantine/cleanup; never normal-read fallback. |
| Explicit file-v2 backend | Developer/headless escape hatch, not an automatic production fallback. | Separate path and format; owner-only creation before bytes; fail closed; file and parent sync; interprocess lock; prominent degraded-security status. |
| Same-user second process or old app version | Another v2 process may race; an old process may use stale legacy records and cannot be forced to honor v2. | Cross-process v2 mutation lock, expected-revision check inside the lock, native present/tombstone record, verified legacy cleanup gate, and honest downgrade status. |
| Crash or power interruption | Can happen after any persistence step. | Atomically replaced intent journal, operation id in the native record, read-after-write verification, durable native tombstone, idempotent recovery, and no destructive cleanup before commit. |
| Logs, telemetry, diagnostics, UI errors | Treated as exportable support material. | Content-free codes and bounded safe parameters; no secret value, body, path, exact length, deterministic fingerprint, or native adapter prose. |

## Security invariants

1. **No saved-secret readback.** No renderer command, event, log, diagnostic,
   panic, or serialized settings object returns stored secret bytes.
2. **No confused-deputy egress.** A saved secret is resolved only for a typed
   `(credential set, auth method, purpose, audience)` authorized by backend
   policy. An arbitrary endpoint cannot infer or select a saved credential.
3. **Secure transport for saved credentials.** Network credentials are
   resolved only for exact backend-owned HTTPS or WSS origins. Loopback HTTP/WS
   development endpoints may use an ephemeral draft only. Redirects are
   disabled for credential-bearing HTTP clients. Non-URL SDK access uses a
   separate typed audience rather than synthesizing a URL.
4. **Bundle consistency.** A reader sees every field from one committed
   generation or a typed failure; it never observes a partially rotated AWS or
   future multi-field set.
5. **Failure is not absence.** Locked, denied, unavailable, corrupt,
   unsupported, cancelled, and migration-required states remain distinguishable
   from `missing` through Rust and IPC.
6. **Passive means read-only.** Opening Settings or requesting status cannot
   migrate, rewrite, delete, prompt for unlock, contact a provider, or change
   authority.
7. **No silent downgrade.** Production never falls back from native storage to
   plaintext because the native store failed. The file-v2 backend is selected
   explicitly before service initialization.
8. **No legacy resurrection.** Once a v2 set is committed or tombstoned, no v1
   keychain, YAML, inline setting, stale generation, or state-file loss can
   silently become authoritative.
9. **Minimum lifetime and copies.** Stored values are not hydrated into global
   settings. Secret containers redact `Debug`, zeroize on drop where the Rust
   type permits it, and are scoped to transport/runtime construction.
10. **Mutation has an observable result.** Replace/delete use an expected
    per-set revision checked inside the v2 cross-process lock and return a
    content-free receipt. A conflict or partial external operation cannot be
    reported as success.
11. **Active use is explicit.** A committed mutation invalidates leases and
    signals backend-owned consumers. New work cannot use the old generation;
    managed long-lived sessions stop or reauthenticate according to a declared
    provider policy. In-flight remote work and provider-side copies cannot be
    retroactively revoked.
12. **Branding cannot orphan secrets.** Display-name changes never alter the
    immutable native-store service namespace, application bundle identifier,
    filesystem root, or account schema without a separate versioned migration.

## Credential domain

The service stores logical credential sets, not arbitrary field names.

| Auth family | Secret unit | Non-secret configuration or locator | Audience rule |
| --- | --- | --- | --- |
| First-class API-key provider | One API key per provider identity | Endpoint/model/provider selection | Backend-owned allowlist of canonical HTTPS/WSS origins and purposes |
| OpenAI first party | One shared OpenAI key | ASR/LLM/TTS/realtime selection | OpenAI-owned HTTPS/WSS origins only; no generic-endpoint fallback |
| Custom OpenAI-compatible endpoint | One key in a backend-issued custom set after the v2 binding workstream | Label, model path, and non-authoritative origin copy | Exact normalized HTTPS/WSS origin is inside the protected bundle; changing it requires a new set and complete secret; draft-only until that workstream lands |
| Gemini API key | One API key | API-key auth mode and model | Google Gemini API origins/purposes only |
| Google service account | No JSON contents in this service | Private file locator and Vertex settings | File is opened by backend under explicit Vertex purpose; path status is not credential presence |
| AWS static/session | Access id + secret + optional session token as one atomic set | Region and auth-source selection | Typed AWS SDK partition/service/region audience; profile and region remain settings |
| AWS ambient/profile | No secret stored by AudioGraph | Profile/region/default-chain choice | Passive status says configured/unknown; only an explicit STS probe claims validation |

The provider registry may reference credential-set and auth-method ids, but it
does not own secret values. A focused generated credential contract owns the
closed ids, field constraints, passive status, mutation receipts, and safe error
codes shared with TypeScript.

## Status and error model

`snapshot_status` reads only service metadata and returns a monotonically
increasing global status epoch, selected backend kind, availability last
established by an explicit operation, migration/cleanup state, and per-set
configured/tombstoned state. Each set separately exposes an opaque random
revision token for equality/CAS. Status does not read every native secret to
derive presence.

`diagnose` is explicit and may access/unlock the native store. `resolve` accesses
one authorized set for one use. `migrate` inventories legacy sources and writes
v2. The UI must not treat these operations as interchangeable.

Minimum safe codes are:

- `missing`
- `locked`
- `access_denied`
- `cancelled`
- `store_unavailable`
- `store_unsupported`
- `corrupt_record`
- `unsupported_schema`
- `payload_too_large`
- `ambiguous_match`
- `conflict`
- `migration_required`
- `migration_conflict`
- `recovery_required`
- `legacy_cleanup_required`
- `permission_hardening_failed`
- `invalid_credential_set`
- `audience_not_allowed`
- `insecure_transport`
- `revision_conflict`
- `operation_in_progress`
- `stalled_worker`
- `commit_unknown`
- `internal`

The backend keeps native causes for local debugging behind redaction. IPC
exposes a stable code, retryability, recovery action, and only allowlisted safe
parameters such as a credential-set id or provider label.

## Mutation and activation

Atomicity applies to one logical credential set. Native credential storage and
`config.yaml` cannot form a distributed transaction. A credential-only rotation
can replace the active record directly. A settings-plus-secret flow uses one
backend-owned prepare/commit protocol:

1. validate the bundle plus expected credential and settings revisions;
2. write/readback-verify a backend-generated staging entry and journal the
   pending operation without publishing it; this reserves the set so another
   cooperating mutation returns `operation_in_progress`;
3. atomically persist settings with a non-secret pending operation marker and a
   revision-fenced backup;
4. recheck both expected revisions, replace/readback-verify the active record,
   commit the journal, and publish;
5. clear the marker and staging entry.

Normal resolution never sees staging. Before active replacement the old
credential remains live. A definite settings or native-write failure restores
or retains the prior pair; an interruption or uncertain commit gates provider
activation with `recovery_required` until idempotent recovery completes.

All v2 mutations acquire `credential-v2/mutation.lock`. The lock covers journal
load, expected-revision check, native write/readback, and journal commit. This
guarantees CAS only for cooperating AudioGraph v2 processes. A keychain editor,
old binary, or other actor that ignores the lock is detected as disagreement
when possible and is outside the atomicity claim.

## Migration safety

Migration recognizes v1 keychain entries, `credentials.yaml`, legacy inline
settings, and prior deletion/import state. It runs as an explicit bootstrap or
user action, never as a presence read.

- Inventory sources without logging values.
- Detect conflicting candidates by secret equality inside the process; expose
  only `conflict` and source kinds.
- Require explicit selection when different live candidates exist.
- Atomically write a non-secret v2 intent, replace the active native record,
  and read back its exact operation id/revision before committing the journal.
- Delete writes a secret-free tombstone into the same active native locator.
  The tombstone is retained and remains authoritative even if the journal is
  lost; record it before any destructive legacy cleanup.
- Import multi-field auth only from one complete source snapshot. Never combine
  partial AWS fields across keychain, YAML, inline settings, or uncertain
  generations; require source selection or re-entry.
- Redact inline settings only after their corresponding v2 set verifies.
- Include the legacy OpenAI Realtime auth field in fixtures and import.
- Quarantine and permission-harden retained legacy plaintext; never read it in
  normal v2 operation. Make delete/cleanup explicit and report downgrade risk
  until complete.
- Recovery replays or rolls back an intent idempotently after each injected
  interruption point. Missing/corrupt journal metadata halts resolution and
  import until exact closed v2 locators are reconciled under `ForbidPrompt`.

Normal v2 mutations are not dual-written to legacy stores. V2-exclusive
mutation stays disabled for a migrated set until its readable legacy copies are
verified removed or quarantined. An old binary can still be launched and may
require re-entry; the service does not claim to prevent downgrade execution.

## Required abuse and failure tests

- Host look-alikes, credentials in URL authority, path/query substring traps,
  Unicode/punycode, alternate ports, HTTP, loopback, and cross-origin redirects.
- Compromised-renderer attempts against every registered command that can cause
  backend HTTP/WebSocket/SDK egress.
- Rotation/delete during HTTP requests, each streaming provider, and AWS SDK
  refresh; assert no new work uses the prior generation.
- Multi-field bundle interruption at every write/journal/commit/cleanup step.
- Locked, denied, cancelled, unavailable, unsupported, corrupt, too-large, and
  missing native-store behavior; assert no case becomes `missing` or file
  fallback.
- Legacy source conflicts, malformed YAML, permissive permissions, missing
  state, tombstone loss attempts, OpenAI Realtime inline auth, downgrade, and
  repeated idempotent migration.
- Two windows using the same expected revision and a supported second-process
  simulation for journal/native/file-v2 coordination, including lock timeout,
  crash release, and an actor that ignores the lock.
- Hung blocking-worker behavior: no second mutation or automatic retry begins
  while an OS call remains indeterminate.
- macOS `ForbidPrompt` guard failure/restoration and packaged locked-keychain
  proof; no background operation may display a prompt.
- Secret canaries through Debug, logs, errors, telemetry, IPC serialization,
  crash diagnostics, docs/Seeds hygiene, and generated TypeScript types.
- Packaged save/read/replace/delete/restart behavior on Windows Credential
  Manager, macOS Keychain, and Linux Secret Service, including locked or absent
  service states where the platform permits automation.

## Residual risk and non-claims

- Native stores usually protect credentials at the logged-in-user boundary,
  not from every same-user process.
- Cross-process compare-and-set covers only cooperating AudioGraph v2 processes;
  platform credential tools can bypass the journal lock.
- AudioGraph cannot stop an older installed executable from running. Verified
  legacy cleanup prevents normal stale reads but downgrade may require re-entry.
- A remote provider necessarily receives its credential and authorized content.
- An in-flight request or already accepted remote session may outlive a local
  delete; users must revoke/rotate provider-side material for remote revocation.
- Zeroization reduces lifetime but cannot prove removal from every allocator,
  OS API, TLS stack, or provider SDK copy.
- Packaged smoke evidence is required before claiming parity across all three
  desktop platforms. Compile-only CI is not that evidence.
