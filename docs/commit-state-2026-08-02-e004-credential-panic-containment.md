# Commit state: e004 credential panic containment

Date: 2026-08-02

## Scope and custody

- Seed: `audio-graph-e004`
- Branch: `work/audio-graph-e004-credential-panic-containment`
- Accepted base: `8be073fecd8db650548c3d28734ebdebae26e379`
- Worktree: `.worktrees/e004-credential-panic-containment`
- Main checkout: broadly dirty user work; untouched except canonical Seeds updates
- Owned candidate files:
  - `src-tauri/src/crash_handler/mod.rs`
  - `src-tauri/src/credentials/adapters/keyring_entry.rs`
  - `src-tauri/Cargo.toml`
  - `src-tauri/Cargo.lock`
  - this document
- Excluded: runtime/service/domain/native-interaction/platform adapters,
  AppState/commands, IPC/frontend/generated files, CI, and file-v2 work.

No push, Seeds sync, workflow mutation, or production activation is part of
this candidate.

## Defect

`catch_unwind` receives a panic only after Rust's process hook has run. The
existing AudioGraph hook extracted the original `String`/`&str`, persisted it
in `crashes/`, and chained it to stderr. A credential backend panic could
therefore export a secret, private locator/path, or native/provider prose even
though the adapter and runtime later returned a content-free failure.

A second installed-hook inventory found that the enabled Sentry `panic`
integration initialized after AudioGraph's hook. `sentry-panic` takes the
current hook and installs a wrapper that reads the original `PanicHookInfo`
before chaining. Relying on downstream event scrubbing would not make
AudioGraph the sole payload-observing hook owner.

## Implemented boundary

- The crash handler owns one nested thread-local credential-panic scope.
- `with_redacted_credential_panic_payload` returns the closure result unchanged
  and does not catch or suppress unwinding.
- While the scope is active, the global hook substitutes the single closed
  code `credential_boundary` for the payload, thread, location, and backtrace.
- The sensitive path does not pass the original `PanicHookInfo` to the prior
  hook. It writes only a closed stderr notice and emits the existing
  content-free analytics diagnostic under `panic.credential_boundary`.
- Ordinary panics keep the previous crash report, backtrace, and chained hook.
- The marker is thread-local, nested, restored by RAII during unwind, and
  fail-closed during thread-local destruction.
- Every caught `KeyringEntryAdapter` read, write, authority write, delete, and
  delete readback executes inside the scope. The higher serialized runtime will
  additionally wrap whole request dispatch so service/fake/recovery panics use
  the same boundary.
- Sentry's automatic panic integration is removed. AudioGraph's global hook is
  the only production `set_hook`/`take_hook` source and already emits the
  closed analytics event.

The hook never hashes, measures, formats, or forwards the sensitive payload.
It does not attempt racy per-call global hook replacement.

## TDD evidence

The first focused child-process test was written before the scope API. It
failed at compile time at the intended missing
`with_redacted_credential_panic_payload` seam (Rust `E0425`).

The final isolated child-process test installs the actual AudioGraph hook,
then initializes analytics in production order with its transport disabled,
and uses runtime-built canaries for:

- secret content;
- a credential locator;
- a private path; and
- native/provider prose.

It verifies all canaries are absent from crash files and captured stderr while
the closed code remains. A separate ordinary panic retains its diagnostic.
The concurrent child holds a credential scope on one thread while another
thread panics normally, proving the process hook consults thread-local state:
the ordinary canary remains useful and the credential canaries remain absent.
Nested and cross-thread restoration is also tested directly.

The adapter test records scope state from every raw boundary method and proves
all five caught call sites are marked and the marker is absent after return.

## Verification

Rust `1.95.0` was used for the final gates.

- Crash-handler suite: 9 passed.
- Exact credential panic tests: 2 passed.
- Keyring-entry suite: 10 passed.
- Exact all-call-site scope test: 1 passed.
- Full credentials suite under locked cloud features: 232 passed, 1 existing
  opt-in OS-keychain smoke ignored.
- Credential logging canary: 1 passed.
- IPC contract package: 41 passed; bin and doc targets passed.
- Generated credential contract: current.
- Locked cloud library/tests check: passed.
- Strict locked cloud all-target Clippy with `-D warnings`: passed after one
  scoped `redundant_closure` correction.
- `cargo fmt --all -- --check`: passed.
- Locked no-deps metadata: passed.
- `cargo tree --locked`: no `sentry-panic` package.
- Production source hook inventory: only `crash_handler/mod.rs` owns
  `set_hook`/`take_hook`.
- Configured `cargo audit`: exit 0 with only the two repository-allowed
  unmaintained warnings, `RUSTSEC-2023-0089` and `RUSTSEC-2025-0141`.
- `git diff --check`: passed.

Existing adapter tests still print their fixed synthetic panic canaries when
run without installing the application hook. The production-hook subprocess
test is the gate for real crash-file/stderr behavior.

## Remaining before e004 closes

This is the non-racy hook and native-entry foundation, not the whole Seed:

1. `audio-graph-34c9` must consume the scope around complete runtime dispatch
   and rerun its error/status/event/debug/panic surface canaries.
2. Final assembled Windows and macOS adapters must rerun packaged native panic
   canaries. Linux remains authorization-gated; no Linux production backend is
   enabled by this change.
3. `audio-graph-2aa8` must consume this Cargo baseline and retain the sole-hook
   inventory during final target assembly.

The Seed remains open until those integration and platform proofs are recorded.
