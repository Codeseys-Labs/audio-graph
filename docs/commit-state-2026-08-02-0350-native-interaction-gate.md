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
  actual root test module. It rejects any externally visible free function
  that returns or constructs either capability, independent of function name,
  and requires the sole capability literals to remain inside
  `NativeInteractionGate::acquire` and
  `NativeRecoveryFacade::diagnose_or_unlock` respectively.

## TDD evidence

All build, test, Clippy, and metadata commands use
`CARGO_TARGET_DIR=/tmp/audio-graph-target-0350`, Rust 1.95.0, `--locked`,
`--no-default-features`, and `--features cloud` where the subcommand accepts
those feature flags.

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
- Exact sealed source inventory: 1 passed, 0 failed.
- `credentials::adapters::native_interaction::tests::`: 12 passed, 0 failed.
- `credentials::adapters::keyring_entry::tests::`: 9 passed, 0 failed.
- `credentials::adapters::native_keyring::tests::`: 25 passed, 0 failed.
- `credentials::adapters::`: 57 passed, 0 failed.
- `credentials::`: 225 passed, 0 failed, 1 pre-existing opt-in OS-keychain
  smoke test ignored.
- Exact credential logging canary: 1 passed, 0 failed; the emitted canary was
  the non-secret `audio-graph-debug-remains-enabled` string.
- Locked cloud library check: passed.
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
