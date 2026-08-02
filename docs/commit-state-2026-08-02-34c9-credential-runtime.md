# Commit state: audio-graph-34c9 credential runtime

Date: 2026-08-02

## Checkout and custody

- Seed: `audio-graph-34c9`
- Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/34c9-credential-runtime`
- Branch: `work/audio-graph-34c9-credential-runtime`
- Accepted base and pre-commit HEAD: `8be073fecd8db650548c3d28734ebdebae26e379`
- Owned final footprint: `src-tauri/src/credentials/runtime.rs` and this document.
- Compile-only overlay: `src-tauri/src/credentials/mod.rs` temporarily declared the runtime. Its base SHA-256 was `7e84fa9031b44a23da22f2126935e8fb3d84d5cb5ad33ed59786e9c92f5cd3e9`; the overlay SHA-256 was `7a330548d46c3ed2c48acb64c942fc334ef8e22a450127c803d1ba64e8948fec`. The overlay is restored byte-for-byte before commit because `audio-graph-4c64` owns committed module visibility.
- No Cargo files, adapter files, service/domain files, root composition, Tauri state/commands, frontend, generated IPC, CI, or Seeds files are owned or committed here.
- `sd sync` is intentionally not run from this isolated implementation branch.

## Implementation

`runtime.rs` adds the backend-owned credential v2 execution boundary:

- A dormant, cached, zero-I/O status projection with distinct `Dormant`, `Opening`, `Uninitialized`, `Ready`, `RecoveryRequired`, `Unavailable`, and `Stalled` lifecycle states.
- One bounded channel (capacity one), one named `credential-v2-runtime` standard thread, and atomic `Idle` / `Busy` / `TimedOut` / `TerminalStalled` admission. Only the worker finalizer can reconcile and reopen admission.
- Non-cancelling async deadlines. A caller timeout projects a temporary stall but does not cancel, retry, dequeue, or release the synchronous native operation. A late definite result updates the cache and ordered service event cursor before reopening admission.
- Opaque request transport for whole service replace/delete/use and settings activation prepare/commit/recovery values. The runtime does not duplicate private activation fields.
- A payload-free explicit diagnosis/unlock backend port. Cached unavailable or recovery-required states cannot be reopened by an ordinary operation.
- A content-free event sink. Service-owned epochs are read once after each operation, checked for gaps/regressions/contiguity, cached, and forwarded exactly once before release. Commit uncertainty, event failure, channel failure, reply closure, or worker panic fails terminally.
- Secret drafts without `Debug`, `Clone`, or serialization. Draft allocations are zeroizing, schema-checked, consumed into backend values before channel transit, and never included in runtime errors or channel diagnostics.
- Production operation/revision IDs from canonical lowercase RFC 4122 UUID v4 values; deterministic IDs remain injectable for tests.
- Admission/cache ordering keeps the status mutex held while reopening the atomic gate so a newly admitted operation cannot be overwritten as idle.

## TDD evidence

The first exact RED command exited 101 because `credentials::runtime` did not yet provide the runtime seam (`CredentialEventSinkFailure`, `CredentialReplaceDraft`, `CredentialRuntime`, backend/event/recovery types). No unrelated failure appeared.

The first behavioral GREEN ran `timed_out_replace_remains_admitted_until_late_commit_and_emits_once`: 1 passed, 0 failed, 1655 filtered. The fake native commit remained live after caller timeout; a second replace was rejected before another store entry; release advanced epoch and emitted once; a third call was admitted only after finalization.

Additional focused RED/GREEN slices proved lifecycle initialization, payload-free diagnosis, pre-admission secret schema validation, ordered replace/delete events, opaque resolve transport, and cached recovery gating. The resolve RED failed only on missing `CredentialRuntime::resolve_for_use`; its GREEN returned an opaque stored lease eventlessly. The recovery-gate RED showed an ordinary second request could reopen the fake locked backend; its GREEN kept open/recovery call counts unchanged until explicit diagnosis.

## Immutable-review correction evidence

The blocked candidate `96e81413982f2cea8acae8a459c7a5c71872ee5c` is preserved by tag `review-blocked/audio-graph-34c9-runtime-gates-96e8141`. The immutable review artifact `/tmp/audio-graph-artifacts/2026-08-02/audio-graph-34c9-review.md` had SHA-256 `e7c70ce67c025c254a48ca159aee5b85b5b8967568a9cd4d90e8dd7fdd69c06e` and required four correction gates.

Three runtime-owned findings were corrected test-first:

- **Timeout/release serialization.** The new deterministic release barrier holds the status mutex while the caller reaches its timeout transition. Both RED runs failed with `left: Stalled` and `right: Ready`. The minimal GREEN moves `Busy -> TimedOut` under the same status mutex used by release, so a release that owns the mutex first publishes `Ready`/`Idle` and the late timeout observes `Idle` without overwriting the cache. Focused GREEN: 1 passed, 0 failed, 1,666 filtered.
- **Post-ready closed failure gate.** A table-driven test exercises `Locked`, `AccessDenied`, `StoreUnavailable`, logical `RecoveryRequired`, `CorruptRecord`, `UnsupportedSchema`, and `AmbiguousMatch` from a genuinely ready service. Both RED runs showed every first failure reprojected as `Ready`/`Unknown`/`Idle`; ordinary second calls either returned `Missing` after additional store I/O or retained a false-ready recovery state. GREEN caches the exact content-free error, drops the closed service, projects the matching unavailable/recovery lifecycle, performs zero further ordinary open/store/recovery I/O, and reopens only through payload-free diagnosis. Focused GREEN: 1 passed, 0 failed, 1,667 filtered.
- **Opaque activation preflight.** Both RED runs returned `InvalidCredentialSet` only after opening the backend: `(InvalidCredentialSet, Ready, Idle, 1, 0, [])` instead of `(InvalidCredentialSet, Dormant, Idle, 0, 0, [])`. GREEN uses one shared target/auth/material shape predicate for replace drafts and the whole opaque activation request, validates by reference before admission, and forwards the original request unchanged. Focused GREEN: 1 passed, 0 failed, 1,668 filtered.

The combined corrected runtime suite passed 14 tests with 0 failures and 1,655 filtered. A fresh strict all-target locked cloud Clippy gate passed with `-D warnings` after the three runtime-owned corrections.

The fourth finding, application panic-hook containment, is owned by blocker `audio-graph-e004`. This workstream will not claim its final GREEN until rebased onto the accepted e004 implementation, the worker call site uses its scoped credential-boundary API with `catch_unwind` remaining outside that boundary, and the production-hook runtime regression gate passes.

## Verification before overlay removal

All Rust compile/test commands used `CARGO_TARGET_DIR=/tmp/audio-graph-target-34c9` unless the command was metadata or the configured audit.

- Runtime tests: 11 passed, 0 failed, 1655 filtered.
- Native adapter tests: 63 passed, 0 failed, 1603 filtered. Scripted panic-canary output is expected by existing adapter tests.
- Full credential tests: 242 passed, 0 failed, 1 existing OS-keychain smoke test ignored, 1423 filtered.
- Logging redaction test: 1 passed, 0 failed, 1665 filtered.
- IPC contract crate: 41 passed, 0 failed; bin and doc-test targets also passed.
- `bun run check:credential-contract`: passed; generated credential contract is current. The final rerun used the isolated target directory.
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check`: passed.
- Strict all-target Clippy with `-D warnings`: first exposed `large_enum_variant` for the open outcome; boxing the worker-owned service resolved it. Final run passed.
- Locked cloud check (`-p audio-graph --lib --tests --no-default-features --features cloud`): passed.
- Locked cloud metadata: passed and wrote a 24,533-byte no-deps JSON envelope to `/tmp/audio-graph-34c9-metadata.json`.
- Configured audit from `src-tauri`: exit 0 with exactly the two allowed unmaintained warnings, `atomic-polyfill` (`RUSTSEC-2023-0089`) and `bincode` (`RUSTSEC-2025-0141`).
- Pre-removal `git diff --check`, accepted-base HEAD check, and Cargo.toml/Cargo.lock equality check: passed.
- Post-removal module SHA-256 is the exact base value `7e84fa9031b44a23da22f2126935e8fb3d84d5cb5ad33ed59786e9c92f5cd3e9`; the locked cloud check, format check, diff check, and Cargo/module equality checks passed again with the runtime intentionally unreachable until `audio-graph-4c64` declares it.

## Integration and remaining validation

- `audio-graph-4c64` must add the committed root-module declaration from the reviewed runtime blob.
- `audio-graph-2aa8` owns the concrete native factory, payload-free recovery bridge, and composition.
- `audio-graph-8849` remains a semantic regate input; runtime variants deliberately carry its service request/prepared types opaquely.
- Re-run the runtime and native matrix after `8849`, `c2be`, `4c64`, and `2aa8` fan-in. Windows/macOS/Linux behavior remains an integration/CI gate, not evidence produced by this Linux-only worktree.
