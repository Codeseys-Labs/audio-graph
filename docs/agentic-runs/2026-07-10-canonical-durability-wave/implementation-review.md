# Seed b481 adversarial implementation review

- Reviewed snapshot: `E:/CS/github/audio-graph-canonical-log` (uncommitted)
- Seed scope: `audio-graph-b481` only — v1 commitment stability and uncertain-recovery identity
- Runtime adoption, destructive-reader locking, parent-directory durability, quarantine manifests, and subprocess crash proof remain explicitly out of scope.
- Initial verdict: **changes requested**. Superseded by the dated final re-verdict below after the one focused fix round.

## What is correct in the implementation

- `PendingAppend` now binds the attempted frame to the original byte length, semantic stream head, and newline state (`canonical_log.rs:602-613`, `canonical_log.rs:908-918`).
- Recovery reparses the exact original base slice and requires byte length, head, and newline equality before considering a retry (`canonical_log.rs:1095-1147`). A non-empty repair suffix must be strictly shorter than, and byte-for-byte a prefix of, the immutable attempted frame before any quarantine or truncation occurs (`canonical_log.rs:1149-1190`).
- The no-suffix path retries without mutation, a complete validated pending event crosses a fresh flush/sync barrier before `AlreadyAccepted`, and every mismatch returns `RecoveryRequired` without clearing `self.poisoned` (`canonical_log.rs:990-1092`, `canonical_log.rs:1149-1158`). These transitions are coherent under the kernel's cooperative-writer assumptions.
- The legacy-without-newline separator case is handled: the attempted frame includes the separator newline (`canonical_log.rs:891-906`), while recovery slices from the pre-separator `base_byte_len`, so the separator plus partial frame is proved as one pending-frame prefix (`canonical_log.rs:1098-1158`).
- JSON object keys are recursively normalized across objects nested in arrays and objects (`canonical_log.rs:1686-1708`). Append canonicalizes before wire serialization (`canonical_log.rs:768-784`, `canonical_log.rs:869-906`), payload hashing canonicalizes defensively (`canonical_log.rs:1711-1714`), and framed reads normalize before recomputing and returning payload state (`canonical_log.rs:1535-1601`). Array order and scalars remain unchanged.
- The exact fixture constants are internally consistent and independently reproducible. A separate .NET SHA-256 calculation of the documented length-delimited fields produced payload hash `86263c62...17f3a`, record hash `736878be...3c6a`, and a 477-byte (`0x1dd`) envelope, matching `canonical_log.rs:2007-2026`.
- The recovered short-write tests now persist the mock backing bytes and strict-reopen them, proving the successful recovery leaves a parseable one-event stream and a parseable mixed legacy/framed two-event stream (`canonical_log.rs:2331-2401`).
- The same-length substitution and foreign-suffix tests assert `RecoveryRequired`, byte-for-byte no mutation, and no quarantine receipt (`canonical_log.rs:2404-2470`). Both tests genuinely fail the previous length-only recovery: the old implementation truncated/retried these inputs.
- The application dependency is reproducible in this snapshot: `Cargo.toml` pins rsac v0.4.1 to commit `7956e6e...e3b9` for Windows/macOS/Linux, `Cargo.lock:8431-8434` resolves that exact git commit, the lock SHA-256 is `C1248BDE...88E5`, and `.gitignore` no longer excludes the application lockfile. The rsac v0.4.1 capture test fixture was updated for its additive `requires_user_consent` field.
- `cargo +1.95.0 fmt --all -- --check` passed in this review lane. No source-level Rust 1.95 or obvious Clippy hazard was found in the new generic bounds, comparisons, recursive normalization, or test code.

## Findings

### P1 — the canonicalization test passes on the old code in the locked feature graph

The test tries to create reverse insertion order with `serde_json::Map` (`canonical_log.rs:1951-1975`) and then proves idempotency plus exact output (`canonical_log.rs:1977-2030`). However, the locked cloud graph has `serde_json` `default/std/raw_value/unbounded_depth` and **does not** enable `serde_json/preserve_order`; `Map` is therefore a `BTreeMap`, so both supposedly different inputs are already recursively key-sorted before `canonicalize_json_value` runs. The previous implementation, which directly hashed/serialized `Value`, produces the same payload hash and exact frame under this graph. The golden constants freeze today's bytes, but this test does not prove the new code is independent of future Cargo feature unification—the exact failure mode b481 exists to close.

Make the test build exercise `serde_json/preserve_order` deliberately, for example by enabling that feature through a test-only dev dependency (features unify for `cargo test`) or a dedicated fixture crate/gate. Keep reverse insertion at both top and nested levels, assert the same exact frame, and demonstrate the test fails when the explicit recursive normalizer is removed. Also assert the exact `CanonicalStreamHead` (`sequence`, `event_id`, `record_hash`); the current fixture asserts frame, payload hash, record hash, and payload but not the Seed's named exact head fixture (`canonical_log.rs:1998-2030`).

### P1 — ADR-0035 and ADR-0036 are absent, so the normative contract is not accepted or reviewable

The wave plan puts both governing ADRs in b481 scope (`wave-plan.md:36-58`), and the Seed acceptance requires accepted decisions for v1 commitments and uncertain recovery identity. The candidate ADR index ends at ADR-0026 (`docs/adr/README.md:24-62`); there is no `0035-*.md`, `0036-*.md`, or index entry in this snapshot.

Add and accept both ADRs before closure. ADR-0035 must freeze recursive Unicode-key ordering, array/scalar behavior, number/string serialization ownership, duplicate-key policy, read normalization versus rejection, feature-invariant testing, golden reblessing rules, and the fact that no previous prototype v1 runtime data shipped. ADR-0036 must state the base tuple, the exact-prefix-only mutation rule, complete-event semantic acceptance rule, poison retention/clear conditions, legacy separator handling, and which quarantine/lock/directory guarantees are explicitly deferred to `audio-graph-8e73`.

### P1 — poison/recovery transition coverage is not complete

The mock exposes short writes, flush failures, and sync failures (`canonical_log.rs:2182-2245`), but tests set only `short_writes` and `fail_syncs`; `fail_flushes` is never exercised. There are no injected write-error-with-zero/full bytes, recovery-read error, recovery flush/sync retry, quarantine creation failure, truncate failure, or post-truncate sync failure tests. The mismatch tests also do not assert that `recovery_required()` remains true, that a different event stays rejected, or that repeated identical recovery remains non-mutating (`canonical_log.rs:2404-2470`).

Add a table-driven transition gate. For each initial and recovery phase, assert the exact redacted outcome/phase, `self.poisoned` retention, rejection of another event, byte/receipt mutation rules, and that poison clears only after `Accepted` or fresh-barrier `AlreadyAccepted`. Quarantine/manifest crash durability remains 8e73, but the in-memory state transitions and zero-mutation mismatch invariant belong to this helper and are cheap deterministic b481 tests.

### P2 — the wave plan records the superseded dependency and gate shape

`wave-plan.md:60-74` runs unlocked Cargo commands and says the manifest still resolves a dirty sibling rsac. The current implementation instead pins the v0.4.1 git revision and has a release lockfile. Update the recorded gates to include `--locked`, record the exact rsac revision and lock hash, and remove the dirty-sibling caveat so future reruns use the evidence actually reviewed.

## Old-code regression check

| New claim/test | Would fail the previous kernel? | Evidence |
| --- | --- | --- |
| Recursive key canonicalization / exact fixture | **No, not under the current locked cloud graph.** Both `Map` inputs are already BTree-sorted, and old payload hashing emits the same fixture. | `canonical_log.rs:1948-2032`; locked `cargo tree -e features` has no `serde_json/preserve_order`. |
| Legacy base without newline recovers once | **Yes.** The old parser reported the partial frame after the inserted separator at `base_len + 1`, while old recovery required `valid_up_to == base_byte_len`; it returned recovery-required instead of accepted. | `canonical_log.rs:2366-2401`; new base slicing is `canonical_log.rs:1103-1118`. |
| Same-length base substitution is unchanged | **Yes.** Old recovery compared byte length only, quarantined the pending-looking tail, and retried against the substituted base. | `canonical_log.rs:2404-2438`; new head/newline comparison is `canonical_log.rs:1139-1147`. |
| Foreign unterminated suffix is unchanged | **Yes.** Old recovery quarantined any repairable suffix at the base length; new recovery rejects a suffix that is not a pending-frame prefix. | `canonical_log.rs:2440-2470`; prefix proof is `canonical_log.rs:1149-1158`. |
| Strict reopen after successful partial recovery | **Stronger than old coverage.** It validates the new final bytes but does not independently force every poison phase. | `canonical_log.rs:2331-2401`. |

## Ship verdict for audio-graph-b481

**Changes requested — do not close or ship b481 yet.** The recovery-identity implementation itself is sound for this bounded kernel and the fixed hashes are independently correct. Closure needs three focused changes: add/accept ADR-0035 and ADR-0036, run canonicalization tests under an insertion-preserving `serde_json` map so they fail the old implementation (and assert the exact head), and complete deterministic poison-transition coverage. This verdict does not evaluate or authorize runtime adoption.

## Final re-review — 2026-07-10

The one authorized fix round resolves every blocking and follow-up finding above.

- The test graph now deliberately enables `serde_json/preserve_order` through the dev dependency (`src-tauri/Cargo.toml:303-309`). A locked feature-tree inspection shows `serde_json/indexmap -> serde_json/preserve_order [dev-dependencies]`, so the reverse-insertion test at `canonical_log.rs:2047-2138` now exercises the normalizer and genuinely fails the old direct-serialization implementation.
- The object fixture asserts exact frame bytes, payload hash, record hash, typed payload, and the complete stream head (`canonical_log.rs:2096-2138`). The scalar fixture independently freezes signed/unsigned integer limits, fractional encoding, Unicode, escapes, exact hashes/frame bytes, and head (`canonical_log.rs:2141-2205`).
- `UniqueJsonValueVisitor` rejects duplicate decoded member names at every recursively visited object, including objects reached through arrays (`canonical_log.rs:322-416`). Framed parsing performs that unique-member pass before conversion, normalization, or semantic hashing (`canonical_log.rs:1604-1658`). The top-level and nested-payload mutation fixtures prove a duplicate whose last value would otherwise preserve the hash is rejected as redacted `InvalidJson` (`canonical_log.rs:3253-3318`).
- The cut matrix covers empty, newline-terminated legacy, and unterminated legacy bases in both repair and `Strict` modes at zero bytes, separator/prefix boundaries, JSON start, final-byte-minus-one, and complete-event sync uncertainty (`canonical_log.rs:2660-2758`). Every successful reconciliation is strict-reopened; every nonzero partial in `Strict` remains poisoned and byte-for-byte unchanged. The dedicated repeated-foreign-suffix strict test confirms the stable fail-closed transition (`canonical_log.rs:3139-3171`).
- Initial write-error (zero and full), flush, sync, short-write, recovery-read, recovery-flush, recovery-sync, quarantine-create, truncate, and post-truncate-sync transitions now assert the exact phase, poison retention, different-event rejection, permitted mutation/receipt behavior, eventual clear condition, and strict reopen (`canonical_log.rs:2356-2970`). Same-length substitution and foreign suffixes additionally prove repeated identical retries remain non-mutating and another event remains rejected (`canonical_log.rs:3045-3137`).
- ADR-0035 and ADR-0036 are accepted and indexed in the dedicated ADR worktree. ADR-0035 freezes recursive key order, array/scalar behavior, duplicate-key rejection, reader normalization, preserve-order testing, golden reblessing, and serializer-version rules (`0035-define-canonical-log-v1-payload-commitments.md:40-83`). ADR-0036 defines the complete base tuple, exact-prefix mutation rule, semantic complete-event acceptance, strict-mode behavior, poison clear conditions, and explicitly deferred runtime durability boundaries (`0036-bind-uncertain-canonical-recovery-to-append-identity.md:39-89`). These ADR changes must land before or atomically with the kernel slice.
- The wave plan now records `--locked` test/Clippy commands, rsac v0.4.1 commit `7956e6e...e3b9`, and lock SHA-256 `C1248BDE...88E5` (`wave-plan.md:60-78`).
- Final locked focused gate reported by the conductor: **23 passed, 0 failed, 1,451 filtered; 0.27 seconds of assertions after the locked build**. The review found no remaining compile/Clippy hazard in the changed production paths; the conductor remains responsible for recording the standard final locked fmt/Clippy gates before closure.

### Final ship verdict for audio-graph-b481

**Approved for the bounded Seed b481 kernel and ADR slice.** No actionable P0, P1, or P2 finding remains within b481's commitment/recovery-identity scope. This approval does **not** authorize a runtime writer: strict mixed-format reader migration (`audio-graph-6896`) and directory/manifest/one-handle/subprocess durability (`audio-graph-8e73`) remain hard downstream gates.
