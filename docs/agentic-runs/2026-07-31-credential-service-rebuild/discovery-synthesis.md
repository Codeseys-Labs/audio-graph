# Credential-service rebuild discovery synthesis

Date: 2026-07-31

Coordinating Seed: `audio-graph-a0f6`

Architecture Seed: `audio-graph-efeb`

Snapshot: `f97e19c251e4c227aade1289b2aba56e0d40ffca`, reconciled with the dirty
integration checkout described in
`docs/backlog/commit-state-2026-07-31-credential-store-rebuild.md`.

## Executive finding

The current credential module has useful local protections, but it is not yet a
safe credential service. It combines a flat 22-field secret snapshot, keychain
access, YAML persistence, migration state, precedence, logging, and mutation in
one 3,500-line module. Callers then clone that snapshot into settings and
provider runtimes. A storage-adapter swap would preserve the most important
problems.

The release-blocking issue is a trust-boundary violation: renderer-callable
probe and catalog commands accept a caller-selected endpoint and can resolve a
saved credential in Rust when no draft key is supplied. Endpoint routing tests
substrings across the full URL and otherwise falls back to the OpenAI slot. A
compromised renderer can therefore direct a stored credential to an arbitrary
HTTP(S) server without ever reading the plaintext over IPC. CSP restrictions on
renderer networking do not constrain Rust `reqwest` traffic.

The rebuild must make the unit of authority a typed credential use, not a
string key. Storage, origin binding, runtime resolution, mutation, migration,
and status must all share that contract.

## What exists today

### Storage and migration

- Production defaults to the OS keychain through `keyring`; each of 22 fields
  is a separate entry under service `audio-graph` and account
  `provider:<field>`.
- `credentials.yaml` can be a primary backend, a fallback, an import source,
  and an override. `credentials-state.yaml` tracks only migrated and deleted
  field names.
- A presence load can scan the keychain, import YAML, rewrite state, and apply
  YAML overrides. It is therefore a hidden write, can block or prompt, and can
  silently ignore malformed legacy YAML.
- The process mutex prevents same-process lost updates. It does not provide a
  cross-process protocol or a transaction spanning keychain, migration state,
  settings, and legacy files.
- File replacement uses a temporary file and rename but does not establish
  crash durability with file and parent-directory sync. Windows ACL hardening
  is best effort and secret bytes are written even when it fails.

### Domain and runtime

- The canonical shape is a flat `CredentialStore` with manually duplicated
  allowlists, accessors, setters, registry arrays, readiness rules, settings
  hydration, and frontend types.
- API keys, an AWS access-key/secret/session-token bundle, an AWS profile,
  region, and a Google service-account path share one secret lifecycle even
  though the latter three are references or ordinary configuration.
- `load_or_default` maps every backend failure to an empty store. Startup and
  some provider paths therefore confuse locked, denied, corrupt, and
  unavailable storage with missing credentials.
- Settings hydration clones all stored plaintext into long-lived
  `AppSettings`; providers take further copies. A save or delete updates future
  construction but does not define what happens to active HTTP clients,
  streaming sockets, or AWS refreshers.
- AWS fields are saved and deleted independently. The refresher can retain one
  field from an old generation while rereading fields from a new generation.

### IPC and frontend

- React correctly receives presence/source rather than saved plaintext, and
  replacement drafts are sent through explicit save calls.
- The IPC vocabulary is stringly typed. Store failures become
  `credential_file_error`, source labels are duplicated in TypeScript, and
  provider readiness sometimes treats English prose as a protocol.
- Express Setup and Settings save multiple fields sequentially and then save
  non-secret settings. There is no revision precondition, mutation receipt, or
  recovery state for a partially completed operation.
- App startup, Express Setup, Settings, readiness, and model catalogs maintain
  overlapping snapshots and refresh rules. No credential revision or event
  establishes which view is current.

## Security findings that control the plan

| Severity | Finding | Required disposition |
| --- | --- | --- |
| Release blocker | Saved credentials can be sent to a renderer-selected endpoint by registered Rust commands. | Block before broader migration. Require typed provider/purpose and exact trusted-origin authorization; arbitrary custom endpoints cannot infer a stored key. |
| High | Explicit Windows file storage writes after ACL-hardening failure. | New file backend fails closed before writing any secret bytes. |
| High | Successful migration can leave a permissive plaintext legacy file indefinitely. | Legacy input is import-only, never part of normal reads, and gets an explicit verified cleanup/quarantine state. |
| Major | Backend failures collapse to missing/empty. | Introduce a closed, content-free error taxonomy and preserve availability separately from presence. |
| Major | Logical bundles and migration steps are non-atomic. | Use versioned bundle commits, expected revisions, an intent journal, read-after-write verification, and idempotent recovery. |
| Major | Presence performs migration and can prompt/block. | Separate journal-backed status from explicit initialization, migration, resolve, and diagnose operations. |
| Major | Legacy OpenAI Realtime inline auth is redacted but not imported. | Add fixtures and migrate it before any redacted settings writeback. |
| Major | Stored secret lifetime extends through settings snapshots and provider clones. | Resolve only inside backend-owned transport/runtime construction and invalidate active leases on mutation. |
| Minor | Secret length and a deterministic short fingerprint are logged. | Remove them from normal telemetry; retain only operation id, credential-set id, result code, and revision. |

## Decisions made by this synthesis

1. The renderer is an untrusted control plane. It may submit ephemeral
   replacement drafts, but it cannot retrieve a saved secret or choose an
   unvalidated destination for one.
2. Credentials are typed logical sets with an auth method, secret fields,
   allowed purposes, and audience policy. AWS is one set; profile, region, and
   private-key file paths are not secret fields.
3. The Rust process owns one long-lived `CredentialService` in application
   state. Native-store calls run off the async executor; consumers receive
   scoped secret leases rather than a global plaintext snapshot.
4. Passive status is side-effect free and revisioned. Explicit bootstrap,
   migration, reconciliation, mutation, diagnosis, and resolve operations have
   distinct contracts.
5. The OS-native credential store is the production default. In-memory storage
   is for tests. A new file-v2 backend is an explicit developer/headless choice
   with fail-closed permissions; `credentials.yaml` is legacy import only.
6. A versioned journal prevents deleted or migrated legacy values from being
   silently re-imported. Migration never dual-writes normal mutations back to
   v1 sources.
7. The stable technical namespace is independent of display branding. An
   optional Aria persona does not change the bundle id, paths, package names, or
   native-store namespace.

The detailed normative contract is ADR-0035 and
`docs/security/credential-service-threat-model.md`. The implementation order,
ownership, gates, and rollback are in
`docs/plans/2026-07-31-credential-service-rebuild.md`.

## Discovery verification

- Rust credential tests: 49 passed, 0 failed, 1 ignored native-store smoke.
- Rust endpoint-routing and provider-registry contract suites: passed.
- Rust formatting and documentation secret-hygiene checks: passed.
- Frontend TypeScript/Vitest gates: not run because the clean baseline did not
  have frontend dependencies installed; discovery did not mutate it with an
  install.

These are pre-rebuild results. They do not prove native Keychain, Credential
Manager, or Secret Service behavior in packaged applications.

## Source reports

The full read-only maps were produced outside the repository so their raw
search inventories do not become permanent project documentation:

- `/tmp/audio-graph-credential-discovery/backend-map.md`
- `/tmp/audio-graph-credential-discovery/frontend-consumer-map.md`
- `/tmp/audio-graph-credential-discovery/security-platform-map.md`
- `/tmp/audio-graph-credential-discovery/native-store-research.md`

This synthesis is the durable repository record of the findings selected to
drive the rebuild.
