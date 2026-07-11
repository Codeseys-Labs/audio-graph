# Strict mixed-format reader discovery synthesis

Date: 2026-07-10

Parent Seed: `audio-graph-6896`

Act children: `audio-graph-9fc4`, `audio-graph-9eee`

## Outcome

The existing typed canonical-log v1 kernel can decode all four production
payloads without a framing change. Act may proceed, but not as one mechanical
replacement of `load_jsonl` and not against the older command/session files on
the kernel branch.

The discovery maps establish four blocking facts:

1. Payload-only `Vec<T>` loaders erase the distinction between a missing stream
   and an existing valid empty stream.
2. Current path lookup creates roots and directories; malformed session-index
   lookup can also create a backup during a nominal read.
3. Recovery still parses event rows directly, skips framed/malformed rows, and
   can persist incorrect recovered metadata.
4. The main checkout contains newer uncommitted Review isolation, projection
   authority, deletion safety, privacy wording, and frontend race guards that
   the clean kernel branch does not contain.

## Accepted registry

ADR-0037 freezes the production identifiers before fixtures or writers consume
them:

| Payload | Stream ID | Outer domain schema |
|---|---|---:|
| `TranscriptEvent` | `transcript_revisions` | 1 |
| `DiarizationSpanRevision` | `speaker_revisions` | 1 |
| `ProjectionPatch` | `projection_patches` | 1 |
| `DataMovementEvent` | `data_movement_events` | 1 |

Each outer version is an independent named constant. It versions the persisted
event payload and replay contract, not a ledger, speaker timeline, or
materialized cache. Movement v1 additionally validates its embedded session id
and embedded schema version.

## Act split

### `audio-graph-9fc4`: clean reader core

Own only the low-conflict persistence slice:

- one centralized stream registry;
- one generic `Strict` adapter returning `Missing` or a present typed snapshot
  with full records and verified head;
- resolve-only root and artifact paths for read operations;
- the four file repository/free loader seams;
- real-payload legacy-only/mixed fixtures plus shared fail-closed corruption
  and whole-tree non-mutation tests.

Compatibility `Vec<T>` methods may project payloads from the strict snapshot,
but file-specific snapshot methods retain presence/head for later consumers.
Surreal remains outside file framing. No runtime appender, quarantine, repair,
or writer-format switch is authorized.

### `audio-graph-9eee`: main-first consumers

Integrate only after the reader core is reviewed. Start from the newer main
semantics and preserve them hunk-by-hunk:

- load one transcript/speaker/projection snapshot per command;
- make present-empty transcript/projection streams authoritative;
- never fall back after canonical corruption;
- keep historical Review read-only with respect to active `AppState`;
- keep export internally consistent from one snapshot;
- make path/index/inventory lookup non-mutating for Review and export;
- retain frontend live/Review locks and stale-response guards.

Do not copy an older whole `commands.rs`, `sessions/mod.rs`, persistence module,
store, or component over the dirty integration checkout.

## Deferred ownership

- Orphan recovery and statistics move to `audio-graph-be7c`: one typed manifest,
  strict replay-derived counts, and no index entry for corrupt candidates.
- Directory barriers, exclusive repair, quarantine receipts, and subprocess
  crash proof remain `audio-graph-8e73`.
- Fresh-process end-to-end data-path conformance remains `audio-graph-2add`.
- Speaker-aware historical projection replay requires a focused follow-up
  before runtime projection bases may cite speaker revisions.

## Review stop conditions

- Any read creates a root/directory, backup, quarantine, temp file, or rewrite.
- Any corruption/version/context error becomes an empty success or fallback.
- Any presence decision uses decoded row count or a separate `exists()` race.
- Any production `CanonicalAppender` construction appears outside the kernel.
- Any integration overwrites the newer main-only Review/deletion/privacy/UI
  safeguards.

The three source maps in this directory are the detailed evidence for this
synthesis.
