# Credential service v2 rebuild plan

Date: 2026-07-31

Epic: `audio-graph-a0f6`

Architecture: ADR-0035 and
`docs/security/credential-service-threat-model.md`

Integration base: `f97e19c251e4c227aade1289b2aba56e0d40ffca`

## Outcome

AudioGraph will replace the flat, snapshot-oriented credential store with a
backend-owned typed secret service. Completion means the v2 service is the only
production authority, every consumer resolves a scoped credential through it,
legacy input is import-only, the renderer cannot direct saved-key egress, and
packaged Windows/macOS/Linux evidence supports the claims made to users.

This is not a storage-library swap. The work includes authority policy, domain
and IPC contracts, service lifecycle, native/file adapters, migration, provider
consumers, frontend workflows, legacy retirement, and platform proof.

## Current queue

| Seed | Priority | Workstream | Done condition in brief |
| --- | --- | --- | --- |
| `audio-graph-efeb` | P1 | WS0 architecture | ADR, threat model, synthesis, executable plan, and queue reviewed and durable |
| `audio-graph-59d1` | P0 | WS0B baseline | Clean integration branch plus dirty-overlap custody/invariants |
| `audio-graph-cffc` | P0 | WS1 origin safety | No saved key can reach a renderer-selected or untrusted endpoint |
| `audio-graph-e11c` | P0 | WS2 contract | Exhaustive Rust-owned typed set/status/error/mutation contract and checked TS projection |
| `audio-graph-a6bf` | P0 | WS3A core | Revisioned service, scoped resolve, fake/fault/concurrency tests |
| `audio-graph-fb2b` | P0 | WS3B adapters | Explicit native adapters and fail-closed opt-in file-v2 backend |
| `audio-graph-86e9` | P0 | WS3C migration | Explicit verified v1 import, tombstones, conflicts, recovery |
| `audio-graph-f107` | P0 | WS4 IPC | One AppState service, typed redacted IPC/events, no fail-open |
| `audio-graph-54e7` | P0 | WS5A runtimes | Every provider uses scoped set/purpose/audience leases |
| `audio-graph-cae3` | P1 | WS5B onboarding | Passive startup/Express use coherent revisioned status |
| `audio-graph-2c33` | P1 | WS5C Settings | Typed bundle editors, CAS conflicts, atomic AWS UI |
| `audio-graph-c826` | P0 | WS6 cutover | Semantic fan-in and end-to-end fake/migration/rollback proof |
| `audio-graph-5f75` | P1 | WS7 retirement | V1 APIs, live YAML, duplicated protocols, and stale docs removed |
| `audio-graph-c4c5` | P0 | WS8 platforms | Packaged native-store matrix passes on Windows/macOS/Linux |
| `audio-graph-0ff1` | P2 | WS9 CI | Approval-gated secret-free CI enforcement after platform proof |

Existing findings remain part of the graph:

- `audio-graph-c420`: preserve unavailable/locked/denied/corrupt separately
  from missing through startup, IPC, readiness, and UI.
- `audio-graph-f70b`: migrate OpenAI Realtime inline auth before redacted
  settings writeback.
- `audio-graph-873d`: rotate and delete AWS access/secret/session fields as one
  bundle across storage, runtime, and Settings.
- `audio-graph-16fc`: retain AudioGraph as the technical/product identity and
  reserve optional Aria naming for a display-only assistant persona.

`audio-graph-0ff1` is deliberately not an epic blocker until workflow-edit
approval is granted. Local executable gates and packaged evidence remain
mandatory.

## Dependency flow

```text
efeb architecture
  ├─> 59d1 clean integration custody
  └─> cffc saved-key origin hotfix
         └─> e11c typed contract
                ├─> a6bf service core
                └─> fb2b native/file adapters
                       └─────────────┐
                a6bf ───────────────┼─> 86e9 migration
                                     └─> f107 IPC/AppState
                                           ├─> 54e7 runtimes
                                           ├─> cae3 onboarding
                                           └─> 2c33 Settings
                                                  │
                       c420 + f70b + 873d + cffc ──┴─> c826 cutover
                                                          └─> 5f75 retire v1
                                                                 └─> c4c5 platform proof
                                                                        └─> 0ff1 CI, approval gated
```

The live Seeds dependency graph is authoritative if this diagram and queue
later diverge.

## Wave 0: architecture and custody

### WS0 — architecture

Branch: `work/audio-graph-efeb-credential-architecture`

Owns only:

- ADR-0035 and its index entry;
- credential threat model;
- discovery synthesis and commit-state record;
- product naming decision; and
- this implementation plan.

Gates:

- no load-bearing TBD in ADR-0035;
- primary-source native-store decision incorporated;
- `bun scripts/check-docs-secret-hygiene.mjs`;
- `git diff --check`;
- reviewer checks threat coverage, migration honesty, platform non-claims,
  dependency order, and dirty-worktree custody; and
- `sd doctor --fix` plus ready/blocked reconciliation in the conductor checkout.

### WS0B — semantic integration baseline

Create `work/audio-graph-cred-v2-integration` from exact base `f97e19c`; do not
copy the dirty main checkout wholesale. The integrator records for every
credential-adjacent dirty file:

- base hash and dirty hash;
- dirty hunks relevant to current MVP behavior;
- the tests/invariants that behavior relies on;
- future workstream ownership; and
- the semantic apply strategy.

Main remains custody-only. Do not clean, reset, stage, commit, or sync its
unrelated work.

## Wave 1: remove the immediate saved-key oracle

### WS1 — `audio-graph-cffc`

Use one security implementer and one reviewer. Own the endpoint authorization
contract, focused command/runtime call sites, and its generated projection; do
not redesign Settings.

Required behavior:

- saved-key use names an explicit provider identity and purpose;
- canonical provider origins are exact normalized HTTPS origins owned by Rust;
- an arbitrary custom origin cannot infer `openai_api_key` or another provider
  key from host/path/query/fragment/userinfo text;
- custom endpoints use a draft for that invocation until an explicit
  origin-bound credential UX exists;
- non-loopback HTTP cannot receive a saved credential;
- redirects are disabled or each hop is re-authorized without cross-origin
  `Authorization`; and
- denial sends no network request containing authorization.

Adversarial gates cover look-alike hosts, suffix/prefix traps, provider names in
paths/queries, credentials in URL authority, Unicode/punycode, trailing dot,
default/alternate ports, HTTP, loopback drafts, and same/cross-origin redirects.

If a real provider needs an unmodeled origin or redirect, disable saved-key use
for that route and file evidence; never restore substring/default routing.

## Wave 2: one shared contract

### WS2 — `audio-graph-e11c`

One owner controls the Rust source, generator, generated TypeScript, and
compatibility re-export. No parallel worker hand-edits generated output.

The contract must represent:

- stable credential-set and auth-method ids;
- secret, private-locator, and ordinary-config field classes;
- required-together and alternative groups;
- allowed provider consumers, purposes, and audience policy;
- passive service/set state and migration/cleanup state;
- content-free error codes and safe recovery actions;
- revisions, idempotency tokens, mutation receipts, and active-use action; and
- the 2,560-byte portable encoded-record limit.

Every current allowlisted v1 field has an explicit migrate, config, deprecate,
or remove disposition. No plaintext secret field appears in a status/response
DTO.

## Wave 3: dark service implementation

Wave 3 branches share the accepted WS2 base and have disjoint ownership. They
can execute in parallel, then fan in serially.

### WS3A — `audio-graph-a6bf`, service core

Own `credentials/{domain,service,fake,test_support}` and module exports only.
Implement status, expected-revision mutations, operation idempotency, scoped
resolve, tombstones, events, serialized blocking-worker seams, zeroizing secret
containers, and failure injection. Keep it dark and unwired.

### WS3B — `audio-graph-fb2b`, adapters

Own `credentials/adapters/**`, narrowly required filesystem utilities, and
approved dependency/lockfile changes only.

Native contract:

- explicit `keyring-core` platform adapters;
- immutable service `com.codeseys.audiograph.credentials`, account
  `v2/<set-id>`;
- Windows equivalent target `Codeseys.AudioGraph.Credentials/v2/<set-id>` with
  Local persistence;
- one binary UTF-8 JSON envelope per logical set via `set_secret`;
- final payload `<= 2560` bytes;
- background `ForbidPrompt`, user-initiated `AllowPrompt` only;
- serialized blocking calls; and
- replace followed by exact revision readback before success publication.

File-v2 is selected explicitly, uses a new path and format, fails closed before
secret bytes when ACL/mode hardening fails, synchronizes file and directory
where supported, and holds an interprocess writer lock. Native failure never
selects it.

### WS3C — `audio-graph-86e9`, migration

Own migration modules, legacy readers, and sanitized fixtures. Import exact v1
locators, YAML, prior migration state, and every inline settings shape.

The state machine is inspect -> plan -> intent -> v2 write -> exact readback ->
commit/tombstone -> optional quarantine/cleanup. Presence never runs it.
Different candidate values become a conflict. Inject failure after every step
and reopen. Redact inline settings only after verified import. Include
`openai_realtime_agent.auth.api_key` and partial AWS fixtures. A v2 record or
tombstone always defeats legacy resurrection.

## Wave 4: service lifecycle and IPC

### WS4 — `audio-graph-f107`

One backend owner has sole wave ownership of credential slices in `state.rs`,
`lib.rs`, `commands.rs`, `error.rs`, and the settings inline-migration bridge.

Install one `Arc<CredentialService>`. Expose versioned redacted status,
replace/delete, explicit migrate, diagnose/unlock, and change events. Preserve
typed failures end to end. Do not report a successful mutation followed by a
fail-open empty cache. Keep v1 compatibility internal and bounded; never add a
saved plaintext read command.

The v2 service remains dark/selectable until runtime/UI consumers and migration
have passed the Wave 5 and 6 gates.

## Wave 5: consumer migration

These branches share WS4 and may proceed in parallel because their ownership is
disjoint.

### WS5A — `audio-graph-54e7`, Rust runtimes

Own backend provider credential-resolution call sites. Replace full-store
hydration with one scoped lease per provider purpose/audience. Inventory every
readiness, catalog, probe, ASR, LLM, TTS, Gemini, OpenAI Realtime, and AWS path.

One-shot requests resolve per request or from a revision-fenced client.
Long-lived consumers register the set revision and stop or reauthenticate on
change. AWS refresh reads one entire bundle. Active requests already accepted
by a provider may finish; delete cannot claim provider-side revocation.

### WS5B — `audio-graph-cae3`, App and Express

Own `App`, Express Setup, fallback/onboarding components, their focused client
hook/tests, and only required localized strings. Passive startup/focus must not
prompt, migrate, write, or contact providers. Serialize submissions and use the
staged bundle -> settings -> runtime activation protocol.

### WS5C — `audio-graph-2c33`, Settings

One frontend owner controls `useSettingsController` and credential settings
components/types/tests. Replace per-key loops with bundle mutations. Surface
revision conflicts and repair states. Key readiness/catalog caches by revision,
not a boolean key/no-key class. Remove raw rejection logging and English-prefix
protocols. Clear ephemeral drafts on settle/discard/unmount.

## Wave 6: semantic fan-in and authority cutover

### WS6 — `audio-graph-c826`

Only the integrator applies accepted branch contributions. For every branch,
verify merge-base, contribution footprint, unrelated commit count, and semantic
invariants before applying; then rerun gates on the assembled snapshot.

Required end-to-end fake flow:

1. user submits a draft bundle;
2. service commits and readback-verifies one revision;
3. passive status reports that revision without reading/migrating;
4. runtime receives it only for an authorized purpose/audience;
5. rotation/delete blocks new old-revision leases and fences long-lived use;
6. restart preserves committed/tombstoned state;
7. every migration interruption recovers deterministically; and
8. locked/unavailable/conflict states remain distinct through UI.

Before the first v2-exclusive mutation, rollback to v1 is exercised. After it,
old-binary downgrade is blocked and recovery is forward-only because dual-write
would reintroduce stale/resurrection risk.

## Waves 7 through 9

### WS7 — `audio-graph-5f75`, retire v1

Remove obsolete per-key/whole-store APIs, live YAML precedence/fallback,
plaintext-shaped frontend types, duplicate key/source lists, fail-open loaders,
and prose parsing after all consumers prove v2. Keep legacy reading only inside
the explicit importer. Update recovery, security, README, and ADR status/index.

### WS8 — `audio-graph-c4c5`, packaged platform proof

Run dummy-secret save/read/replace/delete/restart and v1-upgrade tests against
packaged artifacts, not only unit tests:

- Windows Credential Manager: explicit target, Local persistence, 2,560 success
  and 2,561 local rejection, serialization.
- macOS Keychain: stable bundle/signing identity, locked/denied/cancelled and
  same-identity upgrade behavior.
- Linux Secret Service: unlocked, locked, prompt dismissed, missing session
  bus/service, and supported GNOME/KWallet implementations; background never
  prompts.
- All: display-only Aria experiment does not change native locator; file-v2
  permission and concurrent-writer failure are proven; no real key is used.

A failing or unavailable platform case blocks parity/release claims for that OS.

### WS9 — `audio-graph-0ff1`, approval-gated CI

Do not edit `.github/workflows/**` without explicit approval. After WS8, wire
only deterministic, secret-free checks that the chosen runners can honestly
execute. Retain local gates if hosted runners cannot support an interactive
native store.

## Integration gates

Run focused gates in worker worktrees and repeat them after fan-in:

- credential domain/service/adapter/migration/failure/concurrency Rust tests;
- hostile saved-key destination and redirect tests;
- provider runtime lifecycle and AWS generation tests;
- credential IPC serialization/registration tests;
- App, Express, Settings, source/i18n, and generated-contract tests;
- `bun run check:provider-registry`;
- `bun run check:endpoint-credential-routing`;
- the new generated credential-contract check;
- `cargo fmt --check`, targeted clippy, and relevant Rust suites using isolated
  target directories;
- Biome, TypeScript, focused Vitest, and frontend build;
- docs/Seeds secret hygiene and `git diff --check`; and
- packaged evidence matrix before release claims.

Network/provider tests use local capture servers or fakes unless explicitly
authorized as live. No real credential belongs in a test, log, fixture, doc,
Seed, screenshot, or CI artifact.

## Review and stop rules

- Each implementation branch gets one read-only review against its Seed and
  ADR. At most one fix/recheck round is allowed; a remaining blocker returns the
  workstream to discovery/planning instead of accumulating patches.
- A standing critic examines each squash-merged wave snapshot, not live worker
  worktrees. Every finding becomes an updated or new Seed.
- Stop for ambiguous authority, plaintext/secret-derived artifacts, an adapter
  contradicting the typed contract, unprovable anti-resurrection, unexplained
  dirty-main semantic loss, unapproved workflow/lockfile expansion, or unrelated
  branch history.

## Rollback and recovery boundaries

- WS1 is a one-way security tightening. A missing provider alias falls back to
  draft-only/disabled saved auth, never vulnerable default routing.
- WS2/WS3 are dark and revertible before any import or v2-exclusive mutation.
- Shadow import leaves v1 untouched until v2 exact readback verifies. Failed
  migration reports recovery required and keeps prior authority.
- File-v2 stays disabled if confidentiality or durability cannot be proven.
- Before cutover, an internal compatibility selection may return to v1.
- After v2-only replace/delete and legacy cleanup, old binaries may require
  credential re-entry. Do not dual-write stale v1 data to make downgrade appear
  safe.
- Platform failure blocks enabling/releasing v2 for that platform; it does not
  justify silent plaintext fallback.

## Branding boundary

Keep AudioGraph as product/repository/engine and technical identity for this
cycle. “Aria” can be tested as a display-only assistant persona, such as “Ask
Aria” or “Aria Live.” Do not expand A.R.I.A. into package, bundle, keychain,
path, protocol, binary, crate, repository, or telemetry identities during this
rebuild. A future full rename requires its own legal/brand and versioned
technical migration work.
