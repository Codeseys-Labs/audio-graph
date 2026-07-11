# Seed b481 durability implementation review

- Date: 2026-07-10
- Seed under verdict: `audio-graph-b481`
- Candidate snapshot: `E:/CS/github/audio-graph-canonical-log` (uncommitted)
- Review lens: expected-base/newline/exact-suffix uncertain recovery, v1 fixture/canonicalization, durability receipt boundaries, and the locked rsac prerequisite.

## Verdict

**Final re-verdict: Seed b481 satisfies its durability acceptance and is ready to close from this review lane.** The focused fix resolves all three prior P1 gaps. No actionable P0, P1, or P2 finding remains inside b481's bounded kernel scope. The broader runtime durability blockers remain assigned to `audio-graph-8e73` and the parent Seed; this verdict does not authorize runtime writers or strengthen the file-only receipt.

## Recovery case audit

| Case | Implementation result | Test evidence | Review |
|---|---|---|---|
| Complete pending frame after write/flush/sync uncertainty | Full parse finds the exact event commitment at the pending sequence, requires it to be the stream head, then performs a fresh flush + `sync_all` before `AlreadyAccepted` (`canonical_log.rs:1006-1073`). | `uncertainty_retry_requires_fresh_sync_before_already_accepted` (`canonical_log.rs:2281-2307`). | Fail-closed. The test still lacks the Seed's strict-reopen assertion; see P1-1. |
| Partial pending frame | A repairable tail reaches `recover_exact_pending_suffix`; the original base is reparsed and compared by byte length, semantic head, and newline state, and only a strict byte prefix of the immutable attempted frame may be quarantined/truncated (`canonical_log.rs:1013-1018`, `1095-1190`). | Empty-base half write and legacy-base half write both recover and strict-reopen (`canonical_log.rs:2331-2402`). | Fail-closed for covered cuts. Boundary coverage is incomplete; see P1-1. |
| Zero-byte write | The rescanned cache remains exactly at the captured base, so retry is allowed without destructive repair (`canonical_log.rs:1079-1084`). | No explicit zero-byte fault exists: `MemoryLockedFile` always writes at least one byte when `short_writes` is set (`canonical_log.rs:2207-2217`). | Logic is sound; missing regression proof is P1-1. |
| Legacy separator only | The pending frame includes the separator newline when the base lacks one (`canonical_log.rs:891-906`). A separator-only suffix makes the whole file parse successfully but changes byte length/newline state, so the fast retry guard fails and the exact-prefix recovery path verifies, quarantines, truncates to the original base, syncs, and retries (`canonical_log.rs:1079-1087`, `1139-1190`). | The legacy test writes half the frame, not exactly the separator (`canonical_log.rs:2366-2402`, `2207-2217`). | Logic fixes the original defect; the distinct parse-success branch is untested. |
| Foreign unterminated suffix | A suffix that is not a byte prefix of the attempted frame returns `RecoveryRequired(ConcurrentModification)` before quarantine or truncate (`canonical_log.rs:1149-1158`). | Byte-for-byte no-mutation and no-receipt assertion (`canonical_log.rs:2440-2470`). | Fail-closed. |
| Same-length base replacement | The reparsed base head must equal the captured head; a semantic replacement fails before suffix mutation (`canonical_log.rs:1116-1147`). | Different same-length legacy payload returns `RecoveryRequired`, preserves all bytes, and creates no receipt (`canonical_log.rs:2404-2438`). | Fail-closed within the Seed's semantic-head contract. File identity remains a later `audio-graph-8e73` concern. |
| Strict mode | A corrupt/partial tail cannot call exact-suffix repair because that dispatch is gated on `QuarantineUnterminatedTail`; it returns `RecoveryRequired(Stream(...))` with the appender still poisoned (`canonical_log.rs:1006-1024`). A zero-byte uncertainty may safely retry, and a complete event may safely reconcile after a fresh barrier. | Current memory appenders always select `QuarantineUnterminatedTail` (`canonical_log.rs:2248-2279`). Existing `Strict` calls are reopen/read validation, not poisoned-appender strict-mode tests (`canonical_log.rs:2353-2360`, `2391-2398`). | Code is fail-closed; the mode matrix is not directly proven. |

## Durability and dependency boundaries verified

- No new receipt claims parent-directory or manifest durability. `FileDataAndMetadataSynced` still explicitly excludes newly-created parent-directory persistence (`canonical_log.rs:218-227`), and quarantine receipts still say typed-manifest consumption/deletion parity is not implemented (`canonical_log.rs:761-765`). Seed b481 therefore remains a kernel-only slice and does not weaken the `audio-graph-8e73` gate.
- The rsac prerequisite is now reproducible in this snapshot: all three target dependencies pin v0.4.1 commit `7956e6ef24a44672d502e72b0500efb27530e3b9` with target-specific features and defaults disabled (`src-tauri/Cargo.toml:70-94`); `Cargo.lock` resolves rsac 0.4.1 to the same full source revision (`src-tauri/Cargo.lock:8430-8434`); `.gitignore` no longer excludes the application lockfile (`.gitignore:8-11`).
- Read-only verification passed: `cargo +1.95.0 metadata --locked --format-version 1 --no-deps` accepted the graph, and `cargo +1.95.0 tree --locked --no-default-features --features cloud -e features -i rsac` selected only `rsac/feat_windows` on this host. The lockfile SHA-256 is `C1248BDE7D41EB60D6F88F727DD796CAF050A2E3D02DB8403932801BF49288E5`, matching the Seed evidence.
- The expanded focused gate was already recorded as 17 passed, 0 failed. An independent rerun was intentionally stopped while the conductor's strict Clippy process held the shared target-directory lock; this review does not claim a second test execution.

## Initial actionable findings — resolved by the focused fix

### Resolved P1 — The recovery fault matrix omitted zero-byte, separator-only, and direct strict-mode cases

The implementation branches differently for these boundaries, but the test double cannot select them. `short_writes` always persists `(frame.len() / 2).max(1)` (`canonical_log.rs:2207-2217`), every memory appender is created with `QuarantineUnterminatedTail` (`canonical_log.rs:2248-2279`), and the successful full-frame reconciliation test does not strict-reopen the stream (`canonical_log.rs:2281-2307`). The current legacy regression therefore proves a half-frame suffix, not the separator-only parse-success route (`canonical_log.rs:2366-2402`).

The focused fix was required to replace the boolean/count short-write plan with an exact write-length plan and table-drive at least:

1. `0` bytes, `1` byte, the end of the frame prefix/length header, `frame.len() - 1`, and the complete-frame-with-sync-error case;
2. empty/newline-terminated base and legacy non-newline base, with `1` byte explicitly proving separator-only repair;
3. the same cases under `Strict`, proving partial/separator/foreign suffixes remain byte-for-byte unchanged with no quarantine receipt, while zero-byte retry and complete-frame reconciliation remain allowed;
4. strict reopen after every recovery that returns `Accepted` or `AlreadyAccepted`, including the full-frame sync-failure path.

### Resolved P1 — The golden fixture did not freeze the required head and under-sampled the declared scalar contract

The new fixture is valuable: it builds objects in different insertion orders, checks idempotence, compares the complete frame literal, and pins payload and record hashes (`canonical_log.rs:1948-2031`). Recursive object-key sorting is applied before writer serialization and again before reader hashing (`canonical_log.rs:777-785`, `1555-1560`, `1686-1714`). However, b481's acceptance explicitly requires an exact **head** fixture, and the test never asserts `loaded.head`. Its payload also covers only integer, boolean, and null scalars; the code declares string/number representation part of v1 (`canonical_log.rs:1686-1689`) while still delegating those encodings to `serde_json::to_vec` (`canonical_log.rs:1711-1714`).

The focused fix was required to:

1. assert the exact `CanonicalStreamHead { sequence, event_id, record_hash }` produced by the fixture;
2. add a second immutable scalar fixture representative of real records: escaped/control and non-ASCII string content, signed and unsigned integer boundaries used by schemas, and a finite non-integral number; pin the exact frame, payload hash, record hash, and head;
3. keep these constants independent of writer/hash helpers so coordinated drift cannot bless itself.

### Resolved P1 — The two architectural decisions required by b481 were not ADR'd in the initial snapshot

The wave plan puts “ADRs governing the two architectural choices” in Workstream A's file scope (`docs/agentic-runs/2026-07-10-canonical-durability-wave/wave-plan.md:36-50`), and the Seed acceptance requires accepted ADRs. The repository ADR index currently ends at ADR-0026 (`docs/adr/README.md:28-35`, `55-62`); the current git status contains no new ADR or index edit.

The focused fix was required to author and index accepted decisions for:

1. v1's recursive object-key ordering plus the exact scalar/serializer compatibility boundary and version-bump rule; and
2. uncertain recovery's immutable base length/head/newline capture, exact pending-frame-prefix rule, strict-mode behavior, and explicit deferral of file identity, directory-entry durability, and manifest-first quarantine to `audio-graph-8e73`.

No P2-only code issue was found within b481's bounded scope.

## Closure checklist and final verdict

- Recovery code: **passes durability review for the declared semantic-base/exact-suffix contract**.
- Receipt language: **passes; no new parent-directory or manifest overclaim**.
- Key-canonical v1 writer/reader: **passes**, subject to the missing head/scalar fixtures above.
- Locked rsac prerequisite: **passes in the snapshot**. Direct tag verification shows annotated tag `v0.4.1` at `8271ff71f385ce42b9161a7c3eb6adcfefa28dc1` peeling to the pinned commit `7956e6ef24a44672d502e72b0500efb27530e3b9`. The untracked `src-tauri/Cargo.lock` and matching manifest/ignore changes must be included in the eventual commit or this prerequisite disappears.
- Initial Seed verdict: **not ready to close pending the three P1 proof gaps**. This line is historical and is superseded by the final focused-fix re-verdict below.

## Focused-fix re-review

All initial P1 findings are resolved:

1. **Exact write cuts and mode matrix:** `FaultPlan` can now inject exact short-write lengths, partial-write errors, read failures, flush/sync failures, and truncate failures (`canonical_log.rs:2356-2450`). The matrix covers empty, newline-terminated legacy, and unterminated legacy bases; cuts at zero, first byte/separator, framed-header boundary, and missing-final-newline; and both `QuarantineUnterminatedTail` and `Strict` (`canonical_log.rs:2660-2759`). Quarantine mode repairs only proven pending prefixes; strict mode repeatedly leaves every nonzero partial suffix poisoned and byte-for-byte unchanged. Every successful branch strict-reopens and verifies record/head count (`canonical_log.rs:2517-2535`, `2722-2756`).
2. **Initial and recovery poison transitions:** zero-byte and full-frame write errors plus initial flush/sync uncertainty retain poison, reject a different event, reconcile only the identical commitment, clear poison only after success, and strict-reopen (`canonical_log.rs:2587-2657`). Recovery read, recovery flush, recovery sync, quarantine creation, truncate, and post-truncate sync failures preserve the appropriate source state and poison fence; later identical reconciliation strict-reopens (`canonical_log.rs:2761-2970`). Same-length base substitution, foreign suffix, and strict foreign suffix remain repeatably non-mutating and poisoned (`canonical_log.rs:3045-3171`).
3. **Exact v1 fixtures:** the key-canonical fixture now pins the complete frame, payload hash, record hash, and exact stream head (`canonical_log.rs:2046-2139`). A second immutable fixture freezes escaped/control and non-ASCII strings, signed/unsigned integer boundaries, a finite fraction, complete frame bytes, both hashes, and head (`canonical_log.rs:2141-2206`). The locked feature graph was checked and includes `serde_json/preserve_order`, so the fixtures exercise the explicit recursive sorter rather than accidentally relying on a sorted map.
4. **Recursive duplicate rejection:** framed v1 JSON is first parsed through a recursive visitor that rejects duplicate object members before typed-envelope conversion or semantic hashing (`canonical_log.rs:322-416`, `1631-1634`). Top-level payload and nested-object duplicates both fail as `InvalidJson` in strict mode (`canonical_log.rs:3253-3317`). This matches ADR-0035's explicit framed-v1 rule; legacy compatibility remains outside that rule.
5. **Accepted decisions:** ADR-0035 freezes recursive key ordering, pinned serializer scalar bytes, duplicate rejection, exact fixtures, and the format-version rule (`0035-define-canonical-log-v1-payload-commitments.md:40-66`). ADR-0036 freezes expected base length/head/newline state, exact pending-prefix recovery, zero-byte/complete-event handling, strict behavior, poison transitions, and the explicit runtime durability deferrals (`0036-bind-uncertain-canonical-recovery-to-append-identity.md:39-73`). Both are accepted and indexed (`docs/adr/README.md:41-42`) in `E:/CS/github/audio-graph-wt-adr-canonical-v1`.
6. **Executed evidence:** the final locked focused gate completed with **23 passed, 0 failed, 0 ignored, 1451 filtered**; assertions took 0.27 seconds after a 3 minute 48 second locked build. The rsac v0.4.1 full-revision/lock prerequisite and file-only durability caveat remain unchanged.

## Final re-verdict

**Seed `audio-graph-b481` is ready to close from the durability-review perspective.** No actionable P0/P1/P2 item remains within its declared scope. The conductor still owns recording the final non-review gates and integrating the code/ADRs/lockfile together. Runtime `Accepted`, public destructive recovery, parent-directory durability, manifest-first quarantine, file identity, and cross-platform crash proof remain explicitly out of scope and blocked by `audio-graph-8e73` / `audio-graph-90f3`.
