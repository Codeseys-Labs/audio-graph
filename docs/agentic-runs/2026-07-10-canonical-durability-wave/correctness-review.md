# Canonical durability kernel correctness review

- Candidate: `E:/CS/github/audio-graph-canonical-log/src-tauri/src/persistence/canonical_log.rs`
- Module export: `E:/CS/github/audio-graph-canonical-log/src-tauri/src/persistence/mod.rs:33`
- Review mode: read-only candidate inspection; this document is the only review-owned file.
- Seed: `audio-graph-90f3`
- Verdict: **unsafe-to-adopt** (the focused suite may still run; runtime adoption is blocked)

## Review standard

The kernel is being checked independently across public API contracts, framing and sequence validation, legacy/framed interoperation, idempotency and commitment binding, hash determinism, recovery mutation, poison/retry state transitions, diagnostics, resource bounds, and cooperative concurrency. Findings below distinguish kernel-test blockers from runtime-adoption blockers and longer-term hardening.

## Evidence inventory

- The module explicitly says it does not replace runtime writers yet and calls writer quiescence, parent-directory durability, and quarantine lifecycle registration integration blockers (`canonical_log.rs:1-23`).
- The public reader permits strict validation or destructive quarantine of an unterminated tail, with caller-owned writer coordination (`canonical_log.rs:444-507`).
- The appender holds a cooperative exclusive file lock for its lifetime, scans and validates once at open, and rescans only during uncertain-append recovery (`canonical_log.rs:557-745`, `canonical_log.rs:917-1137`).
- The current test module contains 13 tests (`canonical_log.rs:1822-2285`). Coverage gaps and executable proposals are listed below after contract review.

## Findings

No P0 defect was found inside the declared cooperative-single-writer model. The P1 findings below are nevertheless adoption blockers because the Seed's runtime boundary includes crash recovery, legacy-writer retirement, and stable on-disk compatibility.

### P1 blocking — poisoned recovery validates length, not the original base head or pending suffix

`PendingAppend` records only `base_byte_len`, not the base head/file identity or a digest of the exact base prefix (`canonical_log.rs:602-611`). A normal append guards only `file.len() == base_byte_len` (`canonical_log.rs:917-930`). During recovery, a repairable tail is truncated whenever `valid_up_to == base_byte_len`; the tail is never required to be a prefix of the pending frame (`canonical_log.rs:1008-1069`). If no pending event is found, the retry is allowed when only the rescanned byte length matches (`canonical_log.rs:1079-1136`), without installing or comparing the rescanned head.

Under the module's ideal assumption that every legacy/external writer has been quiesced, the only changed suffix is the appender's own attempted frame and this works. Outside that assumption, a same-length base replacement can make the cached pending frame reference the old head, after which `attempt_pending` can sync and return `Accepted` even though a fresh load rejects `PreviousHashMismatch`. An unrelated unterminated foreign suffix can likewise be quarantined and discarded as if it were the appender's partial frame. The module acknowledges that OS locking cannot stop a writer that ignores the lock (`canonical_log.rs:14-19`), so runtime migration must either make that assumption mechanically true or strengthen recovery.

Required before adoption: bind `PendingAppend` to an expected base head/hash and file identity, compare the rescanned base cache with the original cache, and only auto-truncate a recovery suffix that is byte-for-byte a prefix of the pending frame. A mismatch must remain poisoned with `RecoveryRequired(ConcurrentModification)`.

### P1 blocking — destructive reader recovery has a read-to-truncate race and does not honor the appender lock

The public reader reads by path (`canonical_log.rs:465-477`) and later creates a quarantine and truncates by path (`canonical_log.rs:484-504`, `canonical_log.rs:373-390`). It acquires no lock. Its documentation assigns quiescence to the caller (`canonical_log.rs:444-447`), while the appender's exclusive lock exists only on the appender handle (`canonical_log.rs:557-584`). A writer can therefore append after the reader's snapshot and before `set_len`, and the reader can truncate bytes it never copied to quarantine. It can also run destructive recovery while a cooperative `CanonicalAppender` is live because it never attempts the same lock.

Required before adoption: strict reads may remain lock-free, but `QuarantineUnterminatedTail` should take an exclusive locked handle, reread and validate through that handle, then quarantine/truncate that same file identity. At minimum, the destructive surface must be private behind a runtime-owned quiescence token rather than a generally callable enum option.

### P1 blocking — quarantine-before-truncate is not a crash-durable or lifecycle-atomic transaction

The quarantine file is `create_new`, written, flushed, and file-synced (`canonical_log.rs:394-435`), but its parent directory is never synced before the source is truncated and synced (`canonical_log.rs:373-390`, `canonical_log.rs:692-710`, `canonical_log.rs:1039-1069`). A crash can therefore lose the new directory entry after the only source copy was truncated. The module correctly disclaims parent-directory durability and calls lifecycle registration an integration blocker (`canonical_log.rs:21-23`); that disclaimer must remain a hard no-ship gate, not merely documentation.

There is a second atomicity problem: a quarantine write/flush/sync error can leave a partial private file with no receipt (`canonical_log.rs:414-435`), and a later source truncate/sync error can leave a complete duplicate with no returned receipt on the reader/open paths (`canonical_log.rs:379-390`, `canonical_log.rs:692-703`). In poisoned recovery, the receipt is pushed before truncation succeeds (`canonical_log.rs:1039-1060`), so retries can accumulate duplicate artifacts. Receipts live only in an in-memory vector drained by an optional call (`canonical_log.rs:759-764`), and dropping the appender can lose lifecycle knowledge.

Required before adoption: sync the quarantine parent directory before source truncation; give every created quarantine a typed durable manifest entry (including incomplete/failed attempts) before mutation; make retry idempotent; and prove session deletion/export/retention parity. Permission hardening should be mandatory for these content-bearing artifacts rather than best effort.

### P1 blocking — the v1 payload digest is not a format-stable canonical JSON digest

`payload_digest` hashes `serde_json::to_vec(Value)` (`canonical_log.rs:1631-1633`). The current locked cloud graph is stable today: `cargo +1.95.0 tree --locked --no-default-features --features cloud -e features` shows `serde_json` with `default`, `std`, `raw_value`, and `unbounded_depth`, but **not** `serde_json/preserve_order`; the visible `preserve_order` entries belong to `schemars`. Consequently, `serde_json::Map` is key-sorted in this build and equivalent object insertion orders converge. The on-disk v1 contract nevertheless delegates canonicalization to an unfrozen Cargo feature graph and `serde_json`'s `Value` serializer. A future dependency can unify in `serde_json/preserve_order`, or a permitted serializer/version change can alter number/string encoding, causing existing out-of-lexical-order payloads to recompute differently. Whitespace and some duplicate-key byte changes are already normalized away, so the format is semantic-ish rather than byte-exact without explicitly specifying which normalization is authoritative. The record and event hashes otherwise use sound length-delimited domain separation (`canonical_log.rs:1658-1772`).

No fixed v1 frame/hash fixture exists: the round-trip test uses the same implementation for write and read (`canonical_log.rs:1822-1865`), so coordinated drift passes. This blocks freezing v1 even though the present locked graph is deterministic. Define the payload commitment explicitly (recommended: recursively sorted canonical JSON with a pinned number/string encoding, or a separate stable binary commitment encoding), enforce the required `serde_json` feature invariant, and add immutable fixture bytes plus expected payload/record/event hashes so Cargo feature or serializer drift fails before release.

### P1 follow-up before large or externally supplied logs — aggregate resource use is unbounded

Only an individual framed JSON envelope is capped at 64 MiB (`canonical_log.rs:45-46`, `canonical_log.rs:1462-1479`). Both reader paths load the complete file into one `Vec<u8>` (`canonical_log.rs:364-370`, `canonical_log.rs:521-526`); parsing retains every `Value`, then typed conversion or validation allocates again (`canonical_log.rs:1197-1262`). Legacy rows and aggregate file/record/event-index size have no bound (`canonical_log.rs:1276-1369`). Poisoned recovery clones the full pending frame (`canonical_log.rs:986-990`), which can transiently duplicate a 64 MiB event.

For a trusted, session-rotated local MVP this is not a current kernel-test blocker, but it is a predictable OOM/startup-latency failure mode. Add configured maximum log bytes, legacy-row bytes, record count, metadata count, and tail quarantine bytes; prefer streaming validation and an index strategy that does not require payloads plus the entire file simultaneously. All limit errors must remain content-redacted.

### P2 — poison-state rejection occurs after metadata and payload work

`append` normalizes metadata, serializes the full payload, and hashes the commitment before it checks whether a different event is blocked by poison (`canonical_log.rs:766-817`). Thus a non-identical event can return `InvalidEventId` or `PayloadSerialization` rather than the documented `AppenderPoisoned`, and can force large allocation while the writer cannot accept it. Check a differing event ID against the poisoned event before payload serialization; identical IDs still need the commitment comparison.

### P2 — redacted errors are sound, but adjacent diagnostics and artifacts need an explicit logging rule

`CanonicalLogError` contains only operation/reason/index/termination metadata and its `Display` is content-free (`canonical_log.rs:163-215`). The diagnostic test covers a payload-like tail and session ID (`canonical_log.rs:2267-2285`). Public records intentionally expose `T`, and quarantine receipts expose a full path (`canonical_log.rs:83-118`), so they are not safe to log wholesale. In addition, best-effort `set_owner_only` is invoked for source/quarantine files (`canonical_log.rs:425`, `canonical_log.rs:576`); the shared helper logs its full path on failure. Add a module-level rule/test that receipts and record `Debug` values never enter production logs, and test private stream/event/basis identifiers as well as payload text.

### P2 — one test leaks its locked temp directory on Windows

The idempotency test calls `cleanup` while `appender` still owns the open locked file (`canonical_log.rs:1868-1895`); `cleanup` ignores `remove_dir_all` failure (`canonical_log.rs:1801-1805`). Drop the appender before cleanup and make cleanup failures visible after handles are closed.

## Public contract assessment

Every public item in the module is covered by the grouped inventory below.

| Public surface | Assessment |
| --- | --- |
| `CANONICAL_LOG_FORMAT_VERSION`, `CanonicalRecordEncoding` (`canonical_log.rs:39-54`) | v1 is named clearly. Never change the meaning/hash algorithm behind value `1`; add a new parser/writer variant for future formats. Golden fixtures are required to enforce this. |
| `CanonicalBasisHead`, `CanonicalBasisHeadVector`, `CanonicalEventMetadata::new` (`canonical_log.rs:56-81`) | Basis maps are deterministically ordered and stored basis objects deny unknown fields. Construction is permissive by design; append-time validation sorts/deduplicates causal IDs and validates IDs/hashes (`canonical_log.rs:1566-1615`). Self-causality and semantic existence of referenced heads are not checked; runtime owns those graph semantics. |
| `CanonicalRecord`, `CanonicalStreamHead`, `CanonicalTailQuarantineReceipt`, `CanonicalLogSnapshot` (`canonical_log.rs:83-118`) | Returned record/head fields reflect validated chain state. `CanonicalRecord<T>` can expose private payload through `Debug`, and the receipt exposes paths; neither should be classified safe-to-log. Receipt durability/lifecycle is blocking as above. |
| `CanonicalTailRecovery` (`canonical_log.rs:120-124`) | `Strict` is sound. The destructive variant is too easy to call without the required lock/quiescence transaction and should not be runtime-public in its current form. |
| `CanonicalIoOperation`, `CanonicalCorruptionReason`, `CanonicalLogError` (`canonical_log.rs:126-215`) | Error taxonomy covers current parser and file operations without content. It lacks parent-directory sync/manifest operations because those steps do not exist yet. |
| `CanonicalDurability`, `CanonicalAppendDurabilityReceipt` (`canonical_log.rs:217-237`) | The three receipt levels are carefully worded. `FileDataAndMetadataSynced` explicitly does not claim a durable new directory entry. `appended_bytes` needs API documentation clarifying that recovered `AlreadyAccepted` reports `0` even if the uncertain original call wrote the record. |
| `CanonicalAppendRejection` (`canonical_log.rs:239-251`) | Definite rejections occur before this appender intentionally writes, except `ConcurrentModification` collapses a pre-write length-read I/O failure into the same variant (`canonical_log.rs:917-929`). That is safe to retry but loses operational diagnosis. |
| `CanonicalAppendPhase`, `CanonicalAppendUncertaintyReason`, `CanonicalAppendUncertainty` (`canonical_log.rs:253-276`) | They represent post-attempt uncertainty without content. Recovery quarantine collapses create/write/flush/sync into one phase, which is safe but less actionable. |
| `CanonicalAppendRecoveryReason`, `CanonicalAppendRecoveryRequired` (`canonical_log.rs:278-289`) | Correctly keeps the appender poisoned on undecidable stream, concurrency, or event conflicts. Current tests never exercise any `RecoveryRequired` branch. |
| `CanonicalAppendOutcome` (`canonical_log.rs:291-303`) | `#[must_use]` is appropriate. Under the cooperative model, only a complete write + flush + sync returns `Accepted`, and identical recovery performs a fresh barrier before `AlreadyAccepted`. Runtime must not advance live state for `Rejected`, `OutcomeUncertain`, or `RecoveryRequired`. |

| Contract | Assessment |
| --- | --- |
| `load_canonical_stream` | Strict structural/hash/schema validation is fail-closed. Destructive tail recovery is not safe for runtime use until it owns locking and a crash-durable quarantine transaction. |
| `CanonicalAppender::open` | Context validation, one full scan, typed schema validation, initial flush/sync barrier, and lifetime lock are coherent (`canonical_log.rs:640-745`). New-file parent-directory durability is explicitly absent. |
| `head`, `cached_event_count`, `recovery_required` | Accurate for cooperative writes. The cache can be stale after an ignored-lock same-length mutation; adoption must eliminate or detect that case. |
| `take_quarantine_receipts` | Correct drain semantics, but voluntary in-memory draining is insufficient for artifact retention/deletion authority. |
| `append` outcomes | Stable IDs bind session, stream, schema, normalized causal metadata, basis heads, and payload commitment. Full write + flush + `sync_all` precedes `Accepted`; uncertain operations poison the writer. The base/suffix and canonical-payload findings above prevent a general durability claim. |
| Legacy/framed replay | Blank legacy rows do not consume sequence, legacy rows cannot follow framed rows, framed sequence/previous/payload/record hashes are validated (`canonical_log.rs:1276-1547`). A legacy-only file has derived rather than stored integrity/context and must be treated as migration input, not authenticated canonical history. |
| Errors/diagnostics | Core error `Debug`/`Display` are content-redacted. Paths/records/receipts require separate logging discipline. |

## Test proposals

The existing 13 tests cover happy-path framing, same-appender idempotency conflicts, a mixed legacy prefix, typed-prefix-before-tail mutation, appender tail repair, one sync uncertainty, one short write, same-process locking, cached-head performance, unknown envelope fields, and a narrow redaction case (`canonical_log.rs:1822-2285`). They are useful smoke tests but do not satisfy Seed `audio-graph-90f3` fault and compatibility acceptance.

### Review of every current test

| Test | What it proves | Material gap |
| --- | --- | --- |
| `framed_round_trip_binds_context_metadata_and_hash_chain` (`canonical_log.rs:1822-1865`) | Two self-written records reload with context, causal/basis metadata, and previous-hash linkage. | Writer and reader share all hash helpers, so it cannot catch format drift or an independently wrong hash. It does not mutate/tamper any field. |
| `stable_event_id_is_idempotent_but_any_commitment_change_conflicts` (`canonical_log.rs:1867-1895`) | Same-handle exact replay is idempotent; payload and causal changes conflict. | No reopen, basis change, reordered-causal normalization, receipt hash assertion, or file-length assertion. It also cleans up before dropping the Windows lock. |
| `legacy_blank_lines_are_ignored_and_prefix_can_be_extended` (`canonical_log.rs:1897-1930`) | Blank legacy rows do not consume sequence and an unterminated valid legacy row can be extended by v1. | No fixed synthetic ID/hash assertion, context/trust-boundary test, legacy-after-v1 rejection, or typed-invalid legacy test. |
| `unterminated_corrupt_tail_is_quarantined_after_typed_prefix_validation` (`canonical_log.rs:1932-1953`) | A typed-valid legacy prefix survives and the exact tail reaches quarantine. | Does not assert source bytes/length after truncation, strict-mode no mutation, directory durability, manifest ownership, or failure points. |
| `appender_repairs_tail_through_its_exclusively_locked_handle` (`canonical_log.rs:1955-1979`) | Windows-compatible truncation through the locked read/write handle works and the stream can be extended. | No injected truncate/sync failure, receipt durability, source identity replacement, or subprocess lock proof. |
| `typed_invalid_prefix_prevents_recovery_mutation_and_appender_open` (`canonical_log.rs:1981-2012`) | The standard typed schema fails before source mutation on reader and appender open. | Does not inventory orphan quarantine files or exercise a stateful/custom deserializer's second decode after mutation. |
| `uncertainty_retry_requires_fresh_sync_before_already_accepted` (`canonical_log.rs:2102-2128`) | A complete frame whose first sync fails stays poisoned; exact retry rescans and crosses a fresh barrier. | No reopen/fresh-process proof, flush failure, sync failure repetition, or appended-byte semantics assertion. |
| `poisoned_appender_rejects_next_event_until_same_event_recovers` (`canonical_log.rs:2130-2150`) | A valid different event is rejected while poisoned, exact retry recovers, and later event proceeds. | Does not test invalid/nonserializable/oversized different input, same ID with changed commitment, or `RecoveryRequired`. |
| `short_write_recovery_quarantines_tail_and_retries_same_event` (`canonical_log.rs:2152-2185`) | A half-frame write is classified uncertain, quarantined, retried, and cached once. | It never reparses the final backing bytes as a complete stream and does not prove the tail matched the pending frame, source sync/manifest durability, or short writes of 0/full-minus-1. |
| `competing_appenders_are_excluded_by_os_file_lock` (`canonical_log.rs:2187-2205`) | A second same-process appender cannot lock the same path, and lock release permits reopen. | Same-process semantics are not a cross-process/cross-platform lock gate; it does not cover destructive reader recovery ignoring the lock. |
| `normal_appends_use_cached_head_without_full_rescan` (`canonical_log.rs:2207-2221`) | 128 cooperative appends do not reread and produce a monotonic cached head. | No same-length mutation/file replacement check, memory bound, or periodic final structural validation. |
| `unknown_envelope_fields_are_rejected` (`canonical_log.rs:2223-2265`) | `deny_unknown_fields` rejects a modified top-level envelope. | No unknown basis field, duplicate known field, payload duplicate-key semantics, or other tamper reasons. |
| `diagnostics_do_not_include_payload_or_identifier_content` (`canonical_log.rs:2267-2285`) | One corrupt-tail error omits the raw tail and supplied session ID from `Debug` and `Display`. | No stream/event/causal/basis/path/OS-error corpus, no append outcomes, and no guard against callers logging records or receipts. |

Add the following executable tests before runtime adoption:

1. **Same-length base substitution during poison recovery.** Extend `MemoryLockedFile` with a no-bytes write error. Open on a valid base, produce `OutcomeUncertain`, replace the backing base with different valid bytes of identical length, and retry. Expected: `RecoveryRequired(ConcurrentModification)` and no write. Also load the file after every accepted recovery and assert the chain validates.
2. **Foreign suffix must not be auto-truncated.** After a no-bytes write error, append an unterminated suffix that is not a prefix of `pending.frame`. Expected: recovery required, source unchanged, and no quarantine presented as the pending append's repair.
3. **Destructive-reader lock race.** Hold a live `CanonicalAppender`, inject an unterminated tail, and call reader quarantine recovery. Expected after the fix: `LockContended`/typed recovery-required and byte-for-byte no mutation. Add a child-process version on Windows, macOS, and Linux; the current same-process lock test (`canonical_log.rs:2187-2205`) is insufficient.
4. **Quarantine transaction fault matrix.** Inject create, permission, partial write, flush, file sync, parent-directory sync, source truncate, and source sync failures. At every point assert either the complete source remains or a durable, manifest-owned quarantine exists; assert no untracked candidate remains. Retry must not create duplicate logical receipts.
5. **Subprocess kill points.** Kill after frame partial write, complete frame before sync, quarantine file sync, quarantine directory sync, source truncate, and source sync. Reopen in a fresh process and prove the outcome is exactly accepted-once, recoverable tail, or loud recovery-required—never silent loss.
6. **Golden v1 compatibility fixtures.** Check in immutable legacy-only, mixed legacy/v1, and v1-only byte fixtures with out-of-lexical-order nested object keys, Unicode, integer boundaries, `-0.0`, escaped controls, causal IDs, and multi-stream basis heads. Assert exact event ID, payload hash, record hash, frame bytes, and head on all platforms and supported versions.
7. **Hash tamper matrix.** Independently alter session, stream, schema, sequence, event ID, causal order/content, basis head, previous hash, payload, payload hash, record hash, frame length, terminator, duplicate event ID, unknown top-level field, and unknown basis-head field. Assert the precise redacted reason and no recovery mutation for interior/newline-terminated corruption.
8. **Idempotency across reopen.** Append, drop, reopen, retry exact payload with causal IDs in a different input order, and expect `AlreadyAccepted(ValidatedExistingRecord)` with no bytes written. Change each commitment component one at a time and expect `EventIdConflict`.
9. **Full fault outcome matrix.** Exercise write `ENOSPC`, write error with 0/partial/full bytes, short write, flush failure, sync failure, recovery read/flush/sync/truncate/quarantine failures, and initial-open sync failure. No post-write fault may return `Accepted`; no different event may pass a poisoned writer.
10. **Limits.** Test exact maximum and maximum+1 frame, legacy row, aggregate log, metadata counts, and quarantine tail. Assert bounded allocation behavior and content-redacted errors.
11. **Strict/no-mutation matrix.** Missing/empty files, valid unterminated legacy row, valid framed row missing only commit newline, invalid final row with newline, interior corruption, whitespace-only tail, and typed-invalid prefix. Compare file bytes and directory inventory before/after.
12. **Diagnostics corpus.** Put distinct secrets in payload, session, stream, event ID, causal ID, basis stream/event IDs, file path, and OS error text; format every error/outcome intended for logging and assert none appear. Explicitly exclude record/receipt `Debug` from safe-to-log types.

## Unknowns that change the plan

- **Canonical payload semantics:** Are JSON object member order and number lexical form intended to be meaningful? If not, recursive canonicalization is required before v1 can freeze. If yes, the API must accept canonical bytes rather than generic `T` serialization and document this surprising rule.
- **Writer retirement mechanism:** What runtime component proves all legacy `append_jsonl`, buffered writer threads, external processes, and destructive readers are stopped before the appender opens? Without a mechanical handoff, the base/suffix and reader-race findings remain blocking.
- **Artifact manifest transaction:** Can the typed manifest record a quarantine durably before source truncation, and does session deletion enumerate orphan/failed quarantine attempts? If not, quarantine recovery must remain disabled in production.
- **Filesystem durability matrix:** Which parent-directory sync mechanism and documented weaker Windows level are accepted for new source and quarantine files? `File::sync_all` alone cannot close this acceptance item.
- **Operational size envelope:** Maximum capture duration, event rate, and acceptable reopen latency determine whether streaming/bounded parsing is required for MVP or can be the immediately following hardening slice.
- **Legacy trust boundary:** A legacy-only file stores no session/stream/schema or hashes; those are derived from caller context (`canonical_log.rs:1402-1451`). Decide whether migration needs a one-time signed/hashed anchor before those rows are treated as canonical history.

## Verdict

**Unsafe-to-adopt.** The kernel is coherent enough to execute its focused tests, and no P0 flaw was found under its explicitly declared cooperative-single-writer assumptions. It must not become a runtime writer or freeze the v1 format until the four blocking areas are resolved: expected base/suffix validation, lock-owned destructive recovery, directory-durable manifest-owned quarantine, and format-stable payload commitments with golden fixtures. The resource-bound and diagnostics items should be scheduled before logs are treated as unbounded or externally supplied.
