# Credential Native Crate Move Map

[ADR-0054](../adr/0054-isolate-the-credential-native-boundary.md) records the
decision. This map compares master `3e01cd5a` with #573 head `b8f29b5a` at
their merge base. Reconcile it against the merged #573 tree before moving
anything. The file count supports the crate boundary. A signature-preserving
move would expose private gate types through the app's existing lease API,
so change A needs approval for the lease bridge described below. Native
macOS evidence for the dependency-info check is still required.

## Source disposition

The source column is relative to `src-tauri/src/credentials/`. Destinations
are relative to `src-tauri/crates/credential-native/`. A dash in B means the
path from A carries forward. The table accounts for all 32 tracked files
under the app's credential directory at the mapped base.

| App source | Sealed change A | Sealed change B |
| --- | --- | --- |
| `adapters/native_interaction.rs` | `src/native_interaction.rs` | Same file. Make it a leaf and remove the old source-inventory tests. The replacement digest test lives in the app. |
| `adapters/keyring_entry.rs` | `src/keyring_entry.rs` | — |
| `adapters/macos_keychain_ffi.rs` | `src/macos_keychain_ffi.rs` | — |
| `adapters/windows_credential_manager_ffi.rs` | `src/windows_credential_manager_ffi.rs` | — |
| `adapters/native_interaction/explicit_recovery/macos_keychain.rs` | `src/native_interaction/explicit_recovery/macos_keychain.rs` | `src/macos_keychain.rs` |
| `adapters/native_interaction/explicit_recovery/windows_credential_manager.rs` | `src/native_interaction/explicit_recovery/windows_credential_manager.rs` | `src/windows_credential_manager.rs` |
| `adapters/linux_secret_service_recovery.rs` | `src/native_interaction/explicit_recovery/linux_secret_service_recovery.rs` | `src/linux_secret_service_recovery.rs` |
| `adapters/linux_secret_service_recovery/tests.rs` | `src/native_interaction/explicit_recovery/linux_secret_service_recovery/tests.rs` | `src/linux_secret_service_recovery/tests.rs` |
| `adapters/linux_secret_service_wire.rs` | `src/linux_secret_service_wire.rs` | Same path. Replace both wire macros with the send function and typed methods. |
| `adapters/linux_secret_service_wire/crypto.rs` | `src/linux_secret_service_wire/crypto.rs` | — |
| `adapters/linux_secret_service_wire/crypto/tests.rs` | `src/linux_secret_service_wire/crypto/tests.rs` | — |
| `adapters/linux_secret_service_wire/linux/mutations.rs` | `src/linux_secret_service_wire/linux/mutations.rs` | — |
| `adapters/linux_secret_service_wire/linux/recovery_target.rs` | `src/linux_secret_service_wire/linux/recovery_target.rs` | — |
| `adapters/linux_secret_service_wire/linux/tests.rs` | `src/linux_secret_service_wire/linux/tests.rs` | — |
| `adapters/linux_secret_service_wire/linux/tests/mutation_tests.rs` | `src/linux_secret_service_wire/linux/tests/mutation_tests.rs` | — |
| `adapters/linux_secret_service_wire/linux/tests/qualification_tests.rs` | `src/linux_secret_service_wire/linux/tests/qualification_tests.rs` | — |
| `adapters/linux_secret_service_wire/linux/tests/qualification_tests/inventory.rs` | `src/linux_secret_service_wire/linux/tests/qualification_tests/inventory.rs` | Delete. |

In A, remove both `#[path]` attributes by putting Linux recovery and its
test module at their conventional lexical paths. The existing gate,
recovery, and platform module nesting stays in place until B. Add the
package manifest, `src/lib.rs`, the closed failure module, and a pinned
`README.md`. Register the moved modules privately in `lib.rs`, update
their app-qualified paths to crate-local imports, and re-export only the
surface below. In B, add the crate-local `clippy.toml`. The current inner
gate helpers become private items of the leaf itself, with only its
`#[cfg(test)]` module as a child. Adapter siblings can name the borrowed
capability types and use the sanctioned invocation operations. They cannot
read fields, mint grants, or mutate the gate state directly.

The other 15 files stay in the app.

| App path | Role after the move |
| --- | --- |
| `adapters/authority_journal.rs` | Journal codec, namespace, repair, and mutation lock. Import the common namespace constant. |
| `adapters/authority_journal/held_directory.rs` | OS-specific held directory and durable publication. |
| `adapters/macos_acl_ffi.rs` | Journal descriptor ACL clearing. Keep it by the journal. This is filesystem FFI and is never called by the Keychain adapter. |
| `adapters/mod.rs` | Register the remaining journal and coordinator modules and host the new app digest and test. Split the existing canary test helper. |
| `adapters/native_keyring.rs` | Store assembly, journal and record coordination, repair, and the app-facing native session. Call the new entry boundary rather than the gate. |
| `domain.rs` | Records, authority journal, projections, and public error mapping. Re-export the moved failure enum and keep `into_public` as an app extension trait. |
| `fake.rs` | App service and activation doubles. |
| `filesystem_policy.rs` | Path policy interface and selectors. |
| `filesystem_policy/linux.rs` | Linux filesystem policy. |
| `filesystem_policy/macos.rs` | macOS filesystem policy and held-descriptor qualification. |
| `filesystem_policy/windows.rs` | Windows filesystem policy and held-handle qualification. |
| `mod.rs` | Legacy backend, credential module declarations, and legacy tests. |
| `runtime.rs` | Worker, queue, runtime admission, and events. Add the marker call in the later Linux slice. |
| `service.rs` | Store and mutation-session ports, journal ordering and scoped refusals. |
| `test_support.rs` | App's deterministic service token source. |

The macOS ACL ownership is an explicit scope question for A. Moving all
three existing FFI files literally would either export the ACL helper from
`credential-native` or move the journal's held-directory layer with it.
The map keeps the filesystem FFI in the app and moves the two credential
store FFI files. Confirm this reading of “native FFI cores” before A.

## Public surface and lease bridge

Today `native_keyring.rs` takes the process gate before the authority
journal lock. It stores `NativeInteractionLease<'static>` alongside that
lock in `NativeCredentialMutationSession` until read-back or drop. The
current `KeyringBoundary` trait also names `EntryLocator`,
`ForbidPrompt`, and `MutationInvocation` in method signatures. Widening
those types to let the app keep these signatures would violate the decided
surface. Reacquiring for each keyring call would break the transaction's
exclusion and its read-back and latch behavior.

Proposed bridge for A. Keep the current capability-taking trait private
under the name `PlatformKeyringBoundary`. Keep `EntryLocator` construction,
`KeyringEntryAdapter`, and mutation preparation and verification in the
native crate. The exported `KeyringBoundary` represents one leased entry
session. The ordinary factory returns a reusable acquisition closure.
Concretely its success type is
`Arc<dyn Fn() -> Result<Box<dyn KeyringBoundary>, CredentialStoreFailure> + Send + Sync>`.
The closure acquires the one process gate and returns a crate-owned
session. The session has no `Send` requirement. The app stores it under
the existing non-`Send` mutation session and retains the lock order.
The app can implement the trait for its own fake. Such an implementation
receives no real lease, capability, or native transport.

The following is the proposed A/B surface. The columns identify the
consumer of every item and method. The store, service, and runtime files
here are under `src-tauri/src/credentials/`.

| Export or method | App consumer |
| --- | --- |
| `production_keyring_boundary()` | `adapters/native_keyring.rs` opens the native store. Call before opening journal files, so dormant Linux still refuses before I/O. |
| `KeyringBoundary` | `adapters/native_keyring.rs` stores the leased trait object and implements its test double. |
| `KeyringBoundary::read_authority()` | `adapters/native_keyring.rs` validates the marker on open, initialization, and bound reads. |
| `KeyringBoundary::write_authority(bytes)` | `adapters/native_keyring.rs` initializes the marker with the existing exact verification. |
| `KeyringBoundary::read_active(set)` | `adapters/native_keyring.rs` reads active records and checks mutation read-back. |
| `KeyringBoundary::write_active(set, bytes)` | `adapters/native_keyring.rs` replaces an active record or writes a tombstone. |
| `KeyringBoundary::read_staging(operation, set)` | `adapters/native_keyring.rs` reads staging, checks read-back, and inspects repair candidates. |
| `KeyringBoundary::write_staging(operation, set, bytes)` | `adapters/native_keyring.rs` writes staging. |
| `KeyringBoundary::delete_staging(operation, set)` | `adapters/native_keyring.rs` cleans up and verifies absence during mutation or repair. |
| `KeyringBoundary::read_staging_guard(set)` | `adapters/native_keyring.rs` checks ownership and repairs orphan guards. |
| `KeyringBoundary::write_staging_guard(set, bytes)` | `adapters/native_keyring.rs` claims staging ownership. |
| `KeyringBoundary::delete_staging_guard(set)` | `adapters/native_keyring.rs` removes a guard and verifies absence. |
| `KeyringBoundary::ensure_healthy()` | `adapters/native_keyring.rs` checks before journal and record work. |
| `KeyringBoundary::latch_commit_unknown()` | `adapters/native_keyring.rs` reports ambiguous publication, mismatched read-back, and a dropped session with pending read-back. |
| `production_recovery_facade()` | Intended native recovery composition in `runtime.rs`. The current native facade constructor has no app caller. #573 still tests recovery through `CredentialRuntimeBackend`. |
| `NativeRecoveryFacade`, `NativeRecoveryFacade::diagnose_or_unlock(target, cancellation)` | `runtime.rs` in future native backend composition. Fields and the constructor from a supplied recovery implementation stay private. |
| `RecoveryTarget::CredentialStore` | `runtime.rs` in future native backend composition. |
| `RecoveryOutcome::{Ready, RecoveryRequired}` | `runtime.rs` interprets the result. `service.rs` assigns the selected set's scope. |
| `CancellationToken`, `CancellationToken::new()`, `CancellationToken::cancel()` | `runtime.rs` in future native backend composition. Only the crate needs `is_cancelled`. |
| `CredentialStoreFailure` | `domain.rs` re-exports it. `native_keyring.rs`, `authority_journal.rs`, `held_directory.rs`, `fake.rs`, `service.rs`, and `runtime.rs` continue to consume the closed enum via the app domain. |

That is **six named types or traits, two free constructors, and 15 methods**
in A/B, excluding enum variants and trait implementations such as
`Debug`. IDs in signatures use the existing IPC contract types. Reads
continue to carry owned secret bytes in zeroizing buffers. Identity,
attributes, object paths, raw errors, gate handles, and permit factories
remain private. Approval of the bridge is a prerequisite to calling A a
behavior-preserving relocation.

### Later Linux boundary and activation

Use `Missing` for an absent read and success for a definite absent delete.
A committed mutation succeeds. `NotCommitted(code)` preserves its closed
code. Map `InteractionRequired` to `Locked` and shared-store recovery.
`Unsupported`, `Locked`, and `Unavailable` take the corresponding
store admission paths. Map user dismissal and caller cancellation to
`Cancelled` without a new closure. Map a pre-send deadline or an
interactive deadline to `Unavailable`. After mutation dispatch, reconcile
before reporting `CommitUnknown`. A stalled gate or an interactive panic
reports `StalledWorker`. These two latch outcomes are store-terminal.
Set-record recovery-class errors after journal adoption use #573's set
scope. Authority and collection errors use store scope.

Keep a recovery verification that still finds the target locked as
`RecoveryOutcome::RecoveryRequired` with set scope in the coordinator.
`CredentialStoreFailure` has no `RecoveryRequired` variant. A later set
may try its ordinary read. If the shared collection remains locked and
that read observes it, the independent `Locked` result closes store
admission.

Have the native session prepare recovery from the selected record kind
and existing typed IDs. The later slice can add four role-specific
`prepare_locked_*` methods to the trait for authority, active, staging,
and staging guard. They return a one-use facade with the opaque target
inside it when an exact locked target is found. The app re-derives the
role and IDs from the failed operation and passes the window and deadline.
It acquires a short ordinary session for this lookup, drops the session
and any journal lock, and then invokes `diagnose_or_unlock`. The facade
acquires its own lease. The recovery trait keeps its closed target and
receives no locator argument.

Add `credential_native::mark_linux_credential_worker()` at entry to
`runtime_worker_loop`, before constructing `RuntimeWorker`. Expose that
entry only in the Linux slice and require its thread-local marker in
`drive` and inside `recover`, in addition to the existing executor
guards. Tests install a scoped marker on the thread that runs the work.
The marker verifies thread context. The app's single production call
site is a review obligation because Rust cannot restrict a public
dependency function to one app function.

Arming authorizes a separate change to the leaf gate. Its platform dispatch
then wires the Linux ordinary boundary and the gated recovery constructor.
The generic Windows/macOS path and Linux's target-aware recovery share the
existing capability minting path. Bind the new code to the reviewed digest
in that activation commit. The proposal requires no secret or locator in
the sealed recovery trait.

## Shared code and app ripple

| Dependency today | Disposition |
| --- | --- |
| `CredentialStoreFailure` and `into_public` in `credentials/domain.rs` | Move the enum to `credential-native/src/failure.rs`. Keep the existing IPC projection in an app extension trait used by `domain.rs`, `adapters/native_keyring.rs`, `service.rs`, and `runtime.rs`. |
| `RedactedPanicOp`, `RedactedPanic`, the scope, and the unwind catch in `crash_handler/mod.rs` | Move to the std-only `src-tauri/crates/panic-redaction/src/lib.rs`. App and native crate both depend on that same package. The app re-exports its existing names and uses the leaf's read-only current-operation accessor. Keep the hook, crash file paths, and diagnostic reporting in the app. One `ACTIVE_OP` thread-local, including nested catches and panicking payload destructors. |
| `CREDENTIAL_SERVICE` in `keyring_entry.rs` | Put the existing constant in `crates/ipc-contract/src/credential_contract.rs` for the native locators and the journal namespace and mutation lock. This shares the value without changing serialized contracts or duplicating the namespace. |
| `log` and the app logger | Keep the logger and its install in the app. The native crate uses `log` and emits the existing content-free messages. |
| `src/test_log_capture.rs` | Keep the app helper. Copy only the capture helper as `credential-native/src/test_log_capture.rs` under `cfg(test)` with native crate target filtering. Each test binary installs one logger. No production dependency or test feature. |
| `panic_hook_canary` in `credentials/adapters/mod.rs` | Keep the app subprocess report test support. Move native test payload and destructor fixtures into a crate-local test helper. Native tests exercise the shared scope and use a test-local hook. App subprocess tests exercise the real report hook against the shared leaf. |
| `sessions::TEST_HOME_LOCK`, `user_data::DATA_DIR_ENV`, and test directory helpers | Leave app home and data-dir handling in place. Native subprocess fixtures set their own environment and temporary paths through `Command`. They can use a crate-local child environment constant. No in-process override of the app's home or global data directory. |
| Filesystem helpers and IPC identity types | Journal access to `fs_util` and `filesystem_policy` stays in the app. Both packages use the existing IPC contract crate for operation and set IDs. The native crate has no dependency back to the app. |

The complete outside-credentials ripple for the proposed map is below.

| File | Why it changes |
| --- | --- |
| `src-tauri/Cargo.toml` | Add both workspace packages and app dependencies. Remove direct zbus 6 and the Linux client-only dependency entries. Keep the app default member and native SDK edges. |
| `src-tauri/Cargo.lock` | Regenerate the locked package graph for the two packages and relocated dependency edges. |
| `src-tauri/src/crash_handler/mod.rs` | Extract the shared scope, forward existing imports, and read the same thread-local from the app hook. |
| `src-tauri/crates/ipc-contract/src/credential_contract.rs` | Add the shared namespace constant. |
| `.github/workflows/ci.yml` | Explicit package checks and relocated exact opt-in filter. |
| `scripts/check-zbus6-features.py` | Add ownership and test-support checks to the existing feature checks. |
| `scripts/check-credential-native-inputs.py` | New dependency-info validator. |

That is **three app-owned files**, or **four existing files in
`src-tauri/`** if the IPC package is counted as app ripple. Even counting
the three CI and script paths gives seven. The approximately fifteen-file
fallback condition is **not met**. New crate files and the documentation
in this PR are outside that tally. Tracks that keep app callers stable
include the `crash_handler` re-export and the domain failure re-export.
`commands.rs`, `state.rs`, `lib.rs`, `fs_util`, `user_data.rs`, and the
existing native smoke workflow need no change under this map.

## Test split and CI

- Move the gate's lease, recovery, cancellation, and uncertainty tests,
  the entry adapter's tests, both FFI policy and behavior suites, the Linux
  crypto known-answer tests, scripted wire and recovery peers, and headless
  qualification as native crate unit tests. Keep real app crash report and
  legacy tests with the app. Put the scope restoration and payload-drop
  tests in the redaction leaf. These tests assert behavior without requiring
  any app-only scope or duplicate thread-local.
- Today's app `native_keyring.rs` tests inject an isolated gate, poison
  it, count waiters, and compare borrowed prompt addresses. After A, keep
  the app journal and recovery tests there with a fake leased boundary and
  per-session call traces. Move the actual gate assertions into crate unit
  tests. In particular, split the tests for a poisoned gate, shared gate
  serialization, and the single-lease ordinary policy trace. Retain an app
  test for lease lifetime across journal work and pending read-back, and a
  native test for serialization and permit identity. App tests need no
  internal gate access after the split.
- In A, replace the app's gate-only digest test with the full crate seal.
  Move the gate and macOS/Windows lexical assertions that consume both app
  and native source to an app-only `adapters/native_seal_transition_tests.rs`,
  reading the two roots at test runtime. Retarget the Linux inventory's
  expected recovery, wire, and test paths to the relocated native tree.
  Keep native behavioral tests in their destination modules. A native
  `include_str!` of the app's `native_keyring.rs` or `adapters/mod.rs`
  would violate the dep-info check. In B, delete the transition module,
  the gate and Linux scanners, and the tokenizer fixtures and text
  inventories in the macOS/Windows policy and WinCred support files.
  Retain FFI behavior tests and the safe policy modules' unsafe-code
  restrictions. Convert fake peer bus-driver round trips to the shared
  send helper with test-only typed driver routes, so the production lint
  needs just one expectation.
- Use Linux target dependencies for zbus 6 (existing defaults off with
  `default-rt,proxy`), zruntime (`helper` without tracing), the Linux
  wire's Tokio context check, and session crypto. Keep crypto crates as
  non-Linux dev-dependencies for known-answer tests. Use macOS target
  dependencies for Security Framework, its sys crate, and Core Foundation.
  Use the Windows target edge with Foundation and Security Credentials for
  WinCred. Keep platform-independent behavioral doubles enabled on test
  builds and native calls on their own OS. `rustix` stays in the app for
  the journal and filesystem policy. Native test dependencies include
  readily reusable `log`, `uuid`, `hex`, and Linux Tokio test features.
  There is no exported test-support feature.
- Keep the default, all-features, cloud, default plus optional, and cloud
  plus optional Linux profiles in `check-zbus6-features.py`. The optional
  trio is `diarization,sherpa-streaming,surrealdb-embedded`.
  Use the app's manifest dependency entries to reject all direct zbus
  declarations, including renamed, optional, target, build, and dev
  entries. Use resolved package IDs and direct `resolve.nodes[].deps`
  edges for the unique zbus 6 dependent. Check the declaration and the
  resolved features for test support. With cfg-only tests the stricter
  check can reject test support even in metadata's dev and all-features
  union. Continue to reject zbus 6 `tokio` and `tracing` and zruntime
  `tracing`. zbus 5 remains a transitive legacy dependency pending #209.
  [Cargo metadata format](https://doc.rust-lang.org/cargo/commands/cargo-metadata.html)
- Leave the workspace default member alone. Add explicit native package
  invocations to the Linux, macOS, and Windows Rust jobs. Run
  `cargo clippy --locked -p credential-native -p panic-redaction --all-targets -- -D warnings`
  and `cargo test --locked -p credential-native -p panic-redaction` on each
  OS. Keep both app profiles. Run the dependency-info checker on both
  native library and native test compilations on each OS. On Linux keep
  `dbus-daemon` and retarget the opt-in step to
  `cargo test -p credential-native --lib` and the exact ignored name
  `linux_secret_service_wire::linux::tests::qualification_tests::gnome_keyring_headless`.
  Keep the `--ignored --exact --list` predicate and the daemon requirement.
  The existing OS-native smoke workflow keeps its legacy app and
  filesystem filters.
- Test the boundary with a small downstream fixture depending only on
  `credential-native`. Require ordinary and recovery signatures to
  type-check as documented, and compile-fail attempts to resolve zbus,
  construct either native capability, or access the private gate state.
- Expect one small crate unit-test executable plus the std-only leaf per OS
  and one extra normal/test metadata check. Both share existing dependency
  builds. The work does not require another full Tauri or local-ML build.
  The first change invalidates the app graph cache. Record cold and warm
  CI timings in A, since this map does not measure the new packages.

## Reviewed digest and compiler input evidence

Hash every regular file under `credential-native/`, including the
manifest, lint config, README, tests and fixtures. Put no build output in
that directory. Reject symlinks and junctions. Sort crate-relative UTF-8
paths rendered with `/` by byte value. Use an explicit digest format
version and length-prefix each path and byte payload. Hash file bytes
directly. The repository's `.gitattributes` keeps tracked text as LF in
working trees. This avoids normalizing Rust string or fixture bytes during
verification. Keep the sealed SHA-256 and its test outside the pinned
directory in the app's `credentials/adapters/mod.rs`. A bare digest edit
does not establish that a reviewer saw the inputs.

For the compiler input check, use pinned Rust 1.95.0 and fresh target
directories outside the crate. Run locked `cargo check -p credential-native
--lib --message-format=json` and the corresponding `--tests` invocation.
Identify the exact package ID, library target, and test profile from the
artifact messages. Use their hashed metadata filenames to pick the
associated `.d` files from that invocation. Require the expected
artifact for each mode. Reject absent, stale, or unparseable dep-info.

Use the rustc-generated input rules after the artifact rule block.
Windows artifact targets can contain spaces and drive colons. Even a
single Windows `.d` can mix separators for module and include paths.
Strip the single terminal rule colon, decode escaped spaces, and resolve
relative input paths from the Cargo workspace root. For absolute paths use
the platform-native parser. Resolve symlinks or junctions before a
component-based containment check. Require the crate root source and
membership in the reviewed digest file set. Reject unknown dep-info forms
instead of guessing. Test absolute and relative paths, mixed Windows
separators, escaped spaces, equivalent dot components, an out-of-tree
module, text and binary includes, symlink escapes, and missing artifacts.
Rustc's dependency info is a compiler input list for these invocations.
It cannot attest runtime file reads, proc-macro side effects, or paths in
an uncompiled target or feature profile.
[Cargo build cache and dependency info](https://doc.rust-lang.org/cargo/reference/build-cache.html)

The existing WSL app test build produced
`/tmp/audio-graph-152-slice6-target/debug/deps/audio_graph-986ce661190bd925.d`.
It lists these phony input rules at lines 58–60, with the same spellings
in the prerequisite lists:

```text
src/credentials/adapters/native_interaction.rs:
src/credentials/adapters/native_interaction/explicit_recovery/../../linux_secret_service_recovery.rs:
src/credentials/adapters/native_interaction/explicit_recovery/../../linux_secret_service_recovery/tests.rs:
```

The last two are the two existing `#[path]` declarations. A separate
uncommitted scratch Cargo workspace on native Windows and WSL Linux
confirmed on 1.95.0 that both production and unit-test compilation emit
external `#[path]` and `include_str!` paths. The test compilation also
emitted the external text-free `include_bytes!` input. Parsing the phony
rules and resolving them with native paths identified the escapes,
including filenames with spaces. A direct native Windows rustc probe
also emitted drive-qualified absolute prerequisites. Separate Windows
and Linux rustc probes exposed both a relative `include!` escape and
an `include!` through a directory symlink. The latter required resolving
the symlink because the path in the input list looked crate-local.

**Platform conclusion.** The mechanism worked on Windows and Linux for
the fixtures above. Reliability on native macOS is unproven. Run the
same positive and negative cases, including a symlink escape, in A's
native macOS job. That and the complete checker tests on Windows and
Linux are prerequisites for accepting A. A reproducible miss or a
format that cannot be checked reliably on any platform triggers the
ADR fallback. Current evidence does not establish that condition.

## #573 dependencies and open questions

The fetched [#573 diff](https://github.com/Codeseys-Labs/audio-graph/pull/573/files)
includes #570's durability and repair changes as well as scoped refusals.
It changes no file mapped to the new native crate. The following app
assumptions can change when those branches land or are rebased.

| Observed in the fetched diff | Recheck before A or the Linux slice |
| --- | --- |
| `native_keyring.rs` retains both locks in ordinary mutation sessions and adds repair that reads guards and can delete staging and guard entries. | Confirm the session lifetime and order across open, initialize, normal and repair paths. Match the leased trait methods to the final repair callers. Do not move authority policy or turn repair into a native credential call. |
| `authority_journal.rs` and `held_directory.rs` add held-root qualification, publication and ownership repair. macOS and Windows policy gain held-descriptor/handle constructors. | Keep adoption, handle provenance, staging sweep and root-sync semantics in the app. Recheck the macOS ACL dependency and the shared namespace name on the merged tree. |
| `service.rs` adds `ScopedRefusal`, the `*_scoped` methods and set-specific recovery rules after journal adoption. | Confirm the helper for constructing set-scoped `RecoveryRequired`. Preserve store scope for authority problems and shared lock outcomes. |
| `runtime.rs` changes admission, repair and event emission, adds `ExternallyCommitted`, and retains the dedicated worker loop. | Recheck the worker-loop marker insertion point, the runtime backend recovery entry, and the handling of recovery refusals. The native crate does not own event cursors, repair admission, or IPC. |
| `fake.rs` and service/runtime tests gain scoped recovery and repair fixtures. | Keep them app-side. Compare the final tests with the native-keyring test split to preserve cross-layer lease evidence without a public test-only permit or gate API. |

Before implementation, decide the lease bridge and filesystem ownership
question above. In A, check that both consumers resolve the same redaction
package ID. Verify that the real app hook sees the leaf's scope and that
native panic fixtures observe their operation in that same leaf. In B,
check Clippy's configured raw-method and macro paths
against pinned dependencies on all targets. Expectations are reviewable
exceptions, and the digest must cover lint config and sources together.
The code move remains blocked on #573. The mapped size favors the crate
boundary, but the exact pure-move claim and three-platform input check
still need resolution.
