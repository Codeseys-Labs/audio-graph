# Native interaction gate commit state

Date: 2026-08-02

Seed: `audio-graph-0350`

Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/0350-native-interaction-gate`

Branch: `work/audio-graph-0350-native-interaction-gate`

Base and intake HEAD: `0a931be2fbe83573d50cfb01f342336275b10ef6`

Initial worktree status: clean

## Scope and custody

This isolated slice owns only the new native interaction gate, the keyring
entry seam, native-keyring lease threading, the private adapter module
declaration, colocated tests, and this commit-state record. Cargo manifests and
locks, the authority-journal implementation, credential domain/service/runtime
and root module, platform modules, IPC, frontend, provider wiring, state,
commands, workflows, and Seeds are excluded.

The main checkout contains unrelated dirty work under parent/integrator
custody. This worktree neither reads that work into the candidate nor edits,
stages, resets, syncs, or commits it. Seed queue updates remain with the parent
checkout so this isolated candidate does not sweep shared tracker state.

## Frozen boundary

- One process-wide, poison-aware mutex plus irreversible stalled latch covers
  every ordinary native entry call and the complete mutation session.
- `ForbidPrompt` is construction-sealed, non-cloneable, non-defaultable, and
  required by each closed ordinary keyring boundary signature. There is no
  caller-selectable interaction-policy bool, enum, generic, or re-export.
- `AllowPrompt`, its constructor, and `RecoveryBoundary` remain inside the
  private explicit-recovery child. The parent-facing facade is monomorphic and
  accepts only a closed recovery target and cancellation token; it carries no
  secret, record, bytes, locator, path, mutation, closure, or callback.
- Recovery acquires the same gate, performs at most one allow-prompt action,
  drops that capability, and verifies separately under the ordinary sealed
  forbid-prompt capability. Cancellation before or after gate acquisition
  performs zero recovery calls.
- Pre-invocation/read panic becomes `StalledWorker`; a panic or returned
  uncertainty after mutation starts becomes `CommitUnknown`. Either outcome
  irreversibly stalls later in-process access before journal or native I/O.
- Native mutation sessions are bound to their begin-operation and set. Scope
  drift stops before I/O. After a native write, only its matching verification
  lane may access authority/keyring state; overwrite, cross-kind reads,
  load/persist/commit, and delete return `OperationInProgress` until exact
  readback clears the expectation.
- Initialization, provider reads, active/staging operations, readback,
  cleanup, and authority journal readback all use the same per-operation
  sealed prompt identity. Initialization checks each of the 17 built-in active
  locators exactly once and in registry order before authority publication.
- The reachability proof lexically masks Rust strings, raw strings, character
  literals, line comments, and nested block comments before recognizing the
  actual root test module. It masks that module's complete brace-matched range
  while preserving every compiled suffix item. It rejects any externally
  visible free function or owned-returning impl method that returns or
  constructs either capability, independent of function name, and requires the
  sole capability literals to remain inside `NativeInteractionGate::acquire`
  and `NativeRecoveryFacade::diagnose_or_unlock` respectively.
- `NativeInteractionLease` implements `Drop` as a compiler-enforced move
  barrier. Safe Rust cannot move its mutex guard, non-optional `ForbidPrompt`,
  or `MutationInvocation` fields out of a consuming lease. Compiler witnesses
  freeze the allowed borrowed signatures.
- A masked, token-normalized inventory freezes every production method on
  `NativeInteractionGate` and `NativeInteractionLease`, regardless of
  visibility, plus the exact impl headers and sole un-attributed root Gate and
  Lease declarations. Exact fields are bound to those sole declarations. The
  only accepted lease impls are the inherent impl and `Drop`; aliases, trait
  impls, associated constants/types/macros, callback methods, consuming
  methods, and wrapper/tuple owned returns fail closed. Direct `unsafe`,
  `ManuallyDrop`, `forget`, and qualified `ptr::read` escape primitives are
  rejected outside comments and strings.
- A raw-byte SHA-256 review seal covers the complete
  `native_interaction.rs`. Its expected digest lives under `cfg(test)` in the
  parent `adapters/mod.rs`, avoiding self-reference. Any source drift fails the
  exact unit test unless the reviewer-visible second-file seal is consciously
  updated.

## Second immutable rereview correction

Immutable candidate `cda992ed1d4fcbe32c76f16e73fe13d9ed6a6546`
(`review-blocked/audio-graph-0350-owned-token-method-cda992e`) was blocked
because the factory scanner skipped every impl method. This admitted a
sibling-visible consuming method returning the already-minted
`ForbidPrompt`; moving that field dropped the mutex guard and left an ordinary
native-call capability alive outside the serialized lease.

The rereview artifact
`/tmp/audio-graph-artifacts/2026-08-02/audio-graph-0350-rereview.md` was verified
at SHA-256
`34f00ee4ed68d5c43537a4f8ada64a3dc63ef8411e97d075b6d7c25ace57d310`.

The correction deliberately uses both compiler and structural enforcement.
The `Drop` implementation makes the concrete field move illegal in safe Rust,
while the exact source inventory rejects new method surfaces even when their
bodies are compile-valid (`loop {}`), return unrelated types, accept callbacks,
or use private visibility reachable by descendant platform modules. The
inventory lexically excludes direct `#[cfg(test)]` methods instead of trusting
comment markers.

## Third immutable review correction

Immutable candidate `e6fb383c8fee89b41a38aaba17ddfe325bf165e7`
(`review-blocked/audio-graph-0350-post-test-suffix-e6fb383`) was blocked by two
source-proof false negatives. The old extractor discarded every compiled item
after the root test module, hiding a sibling-visible post-test
`ForbidPrompt` factory. The Lease field proof also trusted the first
same-named struct, allowing a cfg-disabled exact decoy to hide a live
`Option<MutexGuard>` plus safe `take()`.

The third-review artifact
`/tmp/audio-graph-artifacts/2026-08-02/audio-graph-0350-third-review.md` was
verified at SHA-256
`27590f00cca78d4d3a391e600b7fd4fe8a8cc54ae2a627f6d238252073c3941f`.

The correction now masks the matching test module's complete attached
attribute cluster and brace range, retains all suffix bytes, collects every root Gate
and Lease declaration, and binds exact fields to the sole un-attributed live
declarations. Direct outer-attribute parsing, balanced const-generic impl
headers, and module-item macro rejection add bounded defense in depth.

A nested read-only adversary then demonstrated why the lexer must not be
treated as a general Rust macro expander: non-local function-body item macros,
procedural attributes/custom derives, doubled attributes, inline const syntax,
and first-match capability decoys create additional expansion classes. The
complete-source digest is therefore the universal closed-world review boundary;
compiler witnesses and lexical fixtures remain narrower semantic evidence.
The seal is reviewer-visible change coupling, not cryptographic authenticity:
an editor can update both files, but the second-file change is explicit review
surface. Any future reviewed `include!` seam requires separate included-source
inventory and a conscious seal update.

## TDD evidence

The final correction's build, test, and Clippy commands use
`CARGO_TARGET_DIR=/tmp/audio-graph-target-0350-third`, Rust 1.95.0, `--locked`,
`--no-default-features`, and `--features cloud` where the subcommand accepts
those feature flags. Earlier candidate evidence used
`/tmp/audio-graph-target-0350`.

- Source inventory RED: the exact
  `credentials::adapters::native_interaction::tests::sealed_prompt_policy_source_inventory_is_closed`
  test failed first because `NativeInteractionGate` was absent. Subsequent RED
  fixtures rejected the missing `ForbidPrompt` ordinary signature and the
  recovery facade outside the private child. GREEN: exact inventory 1 passed,
  0 failed.
- Recovery runtime RED: the exact cancellation test did not compile while the
  private child-local test facade was absent. GREEN: the native-interaction
  suite passed 6 of 6, including pre/post-acquire cancellation, one Allow then
  separate Forbid verification, shared-gate serialization, returned
  uncertainty, and panic latching.
- Source-adversary RED: comment markers initially let a forbidden later impl,
  re-export, or stalled-latch reset escape the structural inventory. A second
  RED exposed a sibling-visible recovery minting function. GREEN: inventory
  scans the real complete source, rejects marker evasion and multiline
  visibility/derive/alias shapes, and nests the sole `AllowPrompt` literal
  construction below the private recovery facade with no exposed minting
  function.
- Immutable-review factory RED: blocked candidate
  `66f19b3848148234917c2e44173e2603d001dd6f` let arbitrarily named,
  sibling-visible free factories for both `ForbidPrompt` and `AllowPrompt`
  pass its detector. The two multiline/qualified-return mutants reported 0
  passed and 2 failed. A tighter mutant hid both constructor literals inside
  visible free-function bodies whose return types did not name the
  capabilities; it independently reported 0 passed and 1 failed. GREEN: all
  three factory/body tests pass under name-independent free-function scanning,
  while displaced-literal fixtures prove the sole intended impl-method sites.
- Immutable-review extraction RED: an exact `#[cfg(test)]` plus `mod tests {`
  delimiter inside either a raw string or block comment truncated the old
  production view; both mutants failed before their later factory/reset was
  visible. GREEN: same-length lexical masking plus root token/depth recognition
  preserves both later factories and the forbidden stalled-latch reset. Both
  delimiter tests pass without marker comments defining the trusted boundary.
- Second immutable-review compiler RED: a `T: Drop` witness failed with E0277
  on `cda992e`. After the no-op `Drop` barrier landed, a real production mutant
  with a qualified, multiline
  `into_forbid_prompt(self) -> ForbidPrompt<'gate>` field move failed `cargo
  check` with E0509: Rust refused to move from a type implementing `Drop`.
  The mutant was then removed and locked cloud check returned green.
- Second immutable-review inventory RED: direct, wrapped, and qualified tuple
  owned returns on `NativeInteractionLease` all evaded the old scanner (`false,
  false, false`) while the borrowed `&ForbidPrompt` accessor and existing
  borrowed mutation tuple were correctly safe controls. GREEN: the owned
  returns are rejected and both borrowed controls remain accepted.
- Closed method/field inventory GREEN: exact all-visibility Gate and Lease
  headers, the inherent/Drop impl set, and the guard/non-Option prompt/mutation
  field tuple are frozen. Mutants remove `Drop`, add private or public consuming
  and callback methods, change the existing accessor to owned/wrapped returns,
  introduce `Option::take`-capable field shape, add trait impls or type/use
  aliases, and add associated const/macro generation; every mutant fails the
  inventory. Masked unsafe/`ManuallyDrop`/`forget`/`ptr::read` mutants also
  fail, with comment and raw-string controls remaining green.
- Third-review suffix RED: the exact post-root-tests factory failed at
  `factory.contains("fabricate_after_tests")` because the production view
  omitted the suffix. GREEN: the test module is blanked in place; factory,
  capability literal, unsafe, alias, extra impl, poison reset, and item-macro
  suffixes remain visible to their detectors.
- Third-review field RED: a cfg-disabled exact Lease declaration plus live
  `Option<MutexGuard>`, `Some(guard)`, and `_guard.take()` left the old
  inventory green. GREEN: the exact in-memory mutant is rejected. A real
  production compiler probe separately confirmed that the unsafe serialization
  shape itself is valid safe Rust, making this structural proof necessary.
- Compiler move-barrier recheck RED: a restored real
  `into_forbid_prompt(self)` field-move mutant still fails locked cloud check
  with E0509. Removing it restores the production build.
- Universal source-seal GREEN: raw SHA-256 digest
  `46631c5488f39ad3d4c488a6e19c43ce32ee090f4c5e51b97d2f959ab0b9b9fc`
  matches the separate private `[u8; 32]` constant in `adapters/mod.rs`.
- Exact active/staging readback RED:
  `wrong_active_and_staging_readback_latch_before_later_native_io` returned a
  normal corrupt readback. GREEN: both mismatches return `CommitUnknown`, latch
  the gate, and permit zero later keyring reads.
- Full-lease trace RED:
  `complete_ordinary_policy_trace_is_forbid_prompt_only` initially had no
  prompt trace. GREEN: the trace records a safe opaque prompt address and a
  closed target enum, then proves one identity per open, initialize, provider,
  and mutation lease, including the exact ordered 17 built-in initialization
  reads.
- Session state-machine RED:
  `mutation_session_rejects_scope_drift_and_unverified_overwrite_before_io`
  first allowed an out-of-scope active read. A second RED allowed cross-kind
  ordinary reads while staging verification was pending. A third RED returned
  an authority journal while verification was pending. GREEN: the exact test
  passes with unchanged keyring/journal counters for all rejected lanes,
  pending overwrite, and pending commit.
- Abandoned verification RED: dropping a mutation session with expected
  readback pending initially left a later store usable. GREEN: session drop
  irreversibly stalls the shared gate before releasing it. Exact successful
  verification clears the expectation and leaves the gate healthy.
- Same-session latch RED: after readback mismatch, the held session initially
  returned another `CommitUnknown` and touched authority state. GREEN: every
  continuation checks the irreversible latch first, returns `StalledWorker`,
  and leaves both journal and keyring counters unchanged.
- Delete uncertainty GREEN: both false-success and failed readback tests return
  `CommitUnknown`, stall the same gate after lease drop, preserve zero native
  error formatting, and show unchanged boundary counters on later acquisition.
- Ready journal cut GREEN: a definite before-publication replacement failure
  remains `Unavailable` and leaves the gate usable; an after-publication
  `CommitUnknown` latches the gate before later journal/keyring I/O.
- Shared-store serialization GREEN: two distinct stores injected with the same
  gate serialize, and the queued second store performs zero keyring reads until
  the first mutation lease drops.

## Panic-hook limitation and downstream custody

`catch_unwind` converts returned adapter failures and latches the gate, but the
Rust process panic hook runs before the unwind is caught. The focused tests use
only non-secret panic canaries and therefore do not claim that secret-,
locator-, path-, native-code-, or provider-prose-bearing platform panics are
redacted from crash reporting. That process-global diagnostic boundary,
concurrent hook canaries, and packaged Windows/macOS/Linux proof remain open
under priority-zero Seed `audio-graph-e004`; per-call global hook swapping is
not authorized here.

## Candidate verification

The final command results are recorded here before the amended single candidate
commit:

- Rustfmt check: passed.
- Arbitrary factory/body mutant filter: 3 passed, 0 failed.
- Raw-string/block-comment delimiter mutant filter: 2 passed, 0 failed.
- Exact lexical-site mutant: 1 passed, 0 failed.
- Owned impl-return mutant matrix: 1 passed, 0 failed (direct, wrapped, and
  qualified tuple owned returns rejected; two borrowed controls accepted).
- Exact Gate/Lease impl, all-method, field, alias, callback, trait, associated
  item, and Drop-removal inventory mutant matrix: 1 passed, 0 failed.
- Masked destructor-escape primitive matrix: 1 passed, 0 failed.
- Real consuming field-move compiler mutant: failed as required with E0509;
  post-removal locked cloud check passed.
- Real cfg-decoy/live `Option<MutexGuard>` plus `take()` mutant: locked cloud
  library check compiled successfully, while the exact structural inventory
  failed as required; the mutant was then removed.
- Exact reviewed-source SHA-256 seal: 1 passed, 0 failed.
- Exact sealed source inventory: 1 passed, 0 failed.
- `credentials::adapters::native_interaction::tests::`: 18 passed, 0 failed.
- `credentials::adapters::keyring_entry::tests::`: 9 passed, 0 failed.
- `credentials::adapters::native_keyring::tests::`: 25 passed, 0 failed.
- `credentials::adapters::`: 63 passed, 0 failed.
- `credentials::`: 231 passed, 0 failed, 1 pre-existing opt-in OS-keychain
  smoke test ignored.
- Exact credential logging canary: 1 passed, 0 failed; the emitted canary was
  the non-secret `audio-graph-debug-remains-enabled` string.
- Locked cloud library+tests check: passed.
- Strict locked cloud all-target Clippy with warnings denied: passed.
- Locked cloud metadata: passed.
- Configured `cargo audit`, run from `src-tauri`: passed after loading 1,178
  advisories, with only the two repository-classified unmaintained warnings
  for `atomic-polyfill` and `bincode`.
- Audit invocation correction: a preliminary worktree-root
  `cargo audit --file src-tauri/Cargo.lock` returned four vulnerabilities and
  22 warnings because `--file` from that directory bypassed
  `src-tauri/.cargo/audit.toml`. Cargo inputs were confirmed byte-identical to
  the base. The authoritative configured invocation above exited zero; no
  waiver or dependency change was used.
- `git diff --check`: passed before candidate staging.

No production factory or runtime wiring is added by this slice; the frozen
facade remains dark until its separately owned platform/runtime assembly work.

The existing `sha2` dependency comment in `Cargo.toml` says SHA-256 is used
only for credential fingerprinting. The review seal makes that comment stale,
but Cargo files are outside this correction's ownership and remain
byte-identical to base; the documentation-only follow-up is left to the owning
assembly Seed.
