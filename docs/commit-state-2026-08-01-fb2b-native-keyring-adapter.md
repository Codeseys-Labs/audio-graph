# Native keyring adapter commit state

Date: 2026-08-02

Seed: `audio-graph-fb2b`

Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/fb2b-native-keyring-adapter`

Branch: `work/audio-graph-fb2b-native-keyring-adapter`

Base and intake HEAD: `b9bdd48d11144edbee475237028a775f6e0ba0b6`

Initial status: clean

## Scope and custody

This slice owns the dark credential-v2 native entry adapter, its ordinary
secret-free authority journal, the stable mutation lock, focused tests, and
the narrowly required dependency and module declarations. It does not wire
`AppState`, commands, IPC, migrations, providers, the frontend, workflows, or
release profiles.

Custom file-v2 storage, Stronghold, filesystem detectors, DACL qualification,
support-profile tables, legacy import, and fallback selection are explicitly
outside this worktree. In particular, the old Windows Enterprise locator is
migration input owned by `audio-graph-86e9`, not a v2 write target.

## Accepted boundary

- The production critical path is the unified `keyring` 4 adapter behind the
  existing private `CredentialEntryStore` and opaque mutation session.
- The immutable service is `com.codeseys.audiograph.credentials`; active,
  staging, and marker accounts are backend-derived exact locators.
- Windows v2 entry construction adds the underlying `keyring-core`
  `persistence=local` modifier. macOS uses the legacy Keychain facade. Linux
  uses the stock zbus facade without claiming that background access is
  prompt-free.
- A marker and ordinary app-local metadata journal share one random,
  secret-free authority instance id. Only an exact pair is ready; no state is
  repaired, imported, or selected automatically. Open may create the metadata
  directory and stable empty lock file, but uninitialized open writes no
  authority marker or journal state.
- One process mutex serializes entry access. A Rust 1.95 file lock remains held
  for the complete mutation session. Poisoning fails closed as
  `stalled_worker` with a restart action, and cross-process contention plus
  killed-owner release are exercised by child processes.
- The logger denies the exact `keyring_core` target namespace so upstream
  locator-bearing Debug/Trace records cannot reach stderr or the file sink.
- Authority state is byte-bounded and strictly decoded, then validated for
  bounded collections, state/revision/source shape, intent/activation
  cross-references, and receipt coherence before it can open as ready.
- Initialization rejects orphan built-in active entries before publication,
  while preserving typed preflight read failures and leaving staging/custom
  namespaces unenumerated.
- Unix journal metadata creates and repairs `0700` root and `0600` state/lock
  modes. Atomic open/temp-write failures remain definite; commit and exact
  readback uncertainty become `commit_unknown`.
- The synchronous adapter does not satisfy the dedicated blocking-worker work
  retained by `audio-graph-f107`.
- Native errors are classified by variant. Associated prose, bytes, locators,
  secrets, lengths, and fingerprints never enter errors or logs.

## Supersession note

The combined WS3B sequence in
`docs/plans/2026-07-31-credential-service-rebuild.md` remains historical. For
new work, it is replaced by the native-first sequence in
`docs/plans/2026-08-01-native-credential-vertical-resequence.md` and the
narrow amendment in ADR-0036. ADR-0035's backend-owned service, fixed
identities, Rust-only leases, marker/journal agreement, tombstones, and exact
readback requirements remain in force.

## Corrective verification

The originally committed snapshot `66f4e2abdf9f245ef8f42c50e0c9951b1e3546bd`
was rejected by source review and amended in place after one-seam-at-a-time
regressions reproduced the findings.

The next review-blocked snapshot
`b4e3686e6bdf51c2c74756f14a6cbdfb744e08d1` was corrected in place again.
Persisted pending intents now require unique idempotency tokens. Pending
activation authentication uses the same built-in set/authentication-method
rule as record construction, and an exact matching marker/journal pair still
reopens recovery-required when that rule, the pending-token rule, or the epoch
reservation is corrupt.

Committed status epochs no longer saturate. Replace, delete, and activation
prepare reserve a checked successor before their first write; a readable
`u64::MAX` journal is terminal and returns content-free
`recovery_required`/`reconcile` for new event-producing work. A pending
activation exclusively reserves its exact predecessor/successor pair, blocks
new replace/delete work globally, and starts only when no unresolved intent
exists. Completed idempotent replace/delete replays remain write-free while
that reservation exists. Every activation continuation, rollback, cleanup,
and restart recovery validates the complete persisted/prepared pair before
native or settings mutation, and the final cleanup assigns the reserved
successor directly.

The terminal after-effect review also exercises commits that become durable
before the adapter reports `commit_unknown`. Exact replace/delete replay now
installs the freshly loaded, monotonic authoritative journal in the service
cache without another write or duplicate event. Activation recovery first
loads the live journal, returns its exact completed receipt when cleanup was
already durable, and otherwise continues from the live pending stage rather
than a stale cached stage. When this reconciliation exposes an unpublished
epoch, event cursors report a gap requiring resnapshot; the `u64::MAX - 1` to
`u64::MAX` cases remain finite and duplicate-free.

The immutable exact-candidate review then rejected
`64e99e1637985df5283898564f876459a375ddf3`. Persisted activation authority
now requires its activation intent to be the sole pending intent globally, so
an exact marker/journal pair containing an unrelated replace or delete reopens
recovery-required. Activation-absent recovery still permits coherent pending
replace/delete intents for distinct sets.

On Unix, a recursive `DirBuilder` requests `0700` for every newly created
journal-root component before it becomes observable; explicit final
verification and repair remain in place. A child process sets a permissive
`umask 000`, calls the creation helper, proves `0700` before final hardening,
then proves exact `0700`/`0600` root, journal, and lock modes. Chmod, fchmod,
wrong-type, and symlink hardening failures now remain a closed
`permission_hardening_failed`/`repair_permissions` result before publication.
Definite create/open/read failures remain `store_unavailable`/`retry`, while a
hardening failure after journal publication remains `commit_unknown`.

Required cloud-profile results run from `src-tauri` in the corrected tree:

- focused terminal after-effect regressions:
  `cargo test --no-default-features --features cloud --locked ambiguous_final_ -- --nocapture`
  — 3 passed, 0 failed;
- focused persisted activation exclusivity regressions:
  `cargo test --no-default-features --features cloud --locked pending_activation_excludes_unrelated_pending -- --nocapture`
  — 2 passed, 0 failed; the activation-absent multi-set replace/delete control
  also passed;
- focused Unix creation and permission-classification regressions — owner-only
  creation under permissive child umask, typed wrong-type/symlink denial,
  ordinary read failure, public remediation mapping, and postpublication
  `commit_unknown` each passed;

- focused adapter suite:
  `cargo test --no-default-features --features cloud --locked credentials::adapters:: -- --nocapture`
  — 37 passed, 0 failed;
- full credential suite:
  `cargo test --no-default-features --features cloud --locked credentials:: -- --nocapture`
  — 205 passed, 0 failed, 1 pre-existing opt-in native smoke ignored;
- credential log content boundary:
  `cargo test --no-default-features --features cloud --locked logging::tests::credential_provider_debug_and_trace_never_reach_the_file_sink -- --exact --nocapture`
  — 1 passed, 0 failed;
- strict Clippy:
  `cargo clippy --no-default-features --features cloud --all-targets --locked -- -D warnings`
  — passed;
- host compile:
  `cargo check --lib --no-default-features --features cloud --locked`
  — passed;
- `cargo fmt --all -- --check`
  — passed;
- `cargo metadata --no-default-features --features cloud --locked --format-version 1 --no-deps`
  — passed;
- `cargo audit` — no known vulnerabilities; two allowed transitive
  unmaintained warnings remain (`atomic-polyfill` through `heapless` and
  `bincode` through `surrealmx`/SurrealDB).

Early focused red/green commands accidentally compiled the default local-ML
feature graph. Those runs are non-authoritative and are not substituted for
any required cloud/locked gate above.

Windows cross-compilation was retried and again stopped in `ring`/`aws-lc-sys`
because this Linux host lacks the MSVC `lib.exe` toolchain. The macOS retry
stopped because Rust 1.95's `x86_64-apple-darwin` standard library is not
installed. Neither blocker is caused by this credential slice, and neither is
reported as target or packaged behavior proof. Windows inherited-ACL and
namespace write-through evidence remains downstream under
`audio-graph-c4c5`; packaged prompt/unlock behavior remains under
`audio-graph-3098` and `audio-graph-c4c5`.

The external implementation artifact contains the full command/result handoff
and the final amended commit SHA.
