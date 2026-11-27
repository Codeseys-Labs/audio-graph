# ADR-0051: Screen Snapshots Evidence Storage, v1 Basis Isolation, and Offline OCR Baseline

- **Status:** proposed (deliberated 2026-11-26 via multi-model review panel: `deliberate-sol` + `deliberate-opus`; resolves Phase 0 architecture choices for `audio-graph-40ad` / `audio-graph-058f`)
- **Date:** 2026-11-26
- **Context / Issues:** `audio-graph-40ad`, parent `audio-graph-058f`, extending retention matrix `audio-graph-c282`; companion design [`docs/designs/conversation-aware-screen-snapshots.md`](../designs/conversation-aware-screen-snapshots.md).
- **Related ADRs:** ADR-0027 (file-canonical durable store), ADR-0031/0042 (projection basis hash versioning), ADR-0043 (canonical stream registry), ADR-0044 (flat session control plane).

---

## Context

[`docs/designs/conversation-aware-screen-snapshots.md`](../designs/conversation-aware-screen-snapshots.md) specifies opt-in shared-pane screen snapshots with local OCR, timeline markers, and later conversation-driven retroactive promotion and hybrid search. Two Phase 0 contract decisions gate any capture or persistence code (`audio-graph-40ad`):

1. How are screen snapshots persisted and isolated from v1 speech-only `ProjectionBasis` hashes?
2. Which offline OCR backend can ship permissively across Windows (NSIS + standalone `.exe`), macOS, and Linux without OS-packaging traps?

---

## Decision

### 1. Screen evidence persistence & strict v1 basis isolation

- **Immutable asset blobs + typed manifest inventory; explicit revision authority:**
  - Image blobs (and derived OCR/thumbnails) are content-addressed (`sha256`, byte length) immutable files managed by the session artifact manifest and runtime inventory (export, recovery, purge, retention under `audio-graph-c282`).
  - Phase 1 ("dumb capture, smart retrieval": manual + visual-change keyframes of a user-selected shared pane, local OCR, timeline markers) persists immutable snapshot records referenced by `(session_id, snapshot_id, revision)` with durable write-and-sync before the control-plane commit is published; recovery never surfaces an unpublished staged file as **Saved**.
  - If Phase 2+ requires ordered cross-snapshot replay semantics beyond immutable attachment records, explicitly amend ADR-0043 with a framed `screen_snapshot_revisions` stream (`SCHEMA_VERSION = 1`) rather than overloading any existing stream.
- **Zero impact on v1 speech-only hashes by construction:**
  - `ProjectionBasis`, `transcript_events_hash_v1`, the compacted-prefix digest, and speech-only replay remain **completely untouched**. Screen snapshots, OCR text, and frame timestamps never enter a v1 speech basis.
  - Future screen-aware projections (Phase 2/3) wrap the full existing basis inside a separately versioned `ScreenEvidenceBasis` listing exact sorted `(snapshot_id, asset_sha256, crop_rev, ocr_rev)` for every frame actually read, dispatched by an explicit patch payload version—never by advancing the ADR-0042 speech semantics floor.
- **Two mandatory pre-activation storage blockers (verified in source):**
  1. **Delete-ordering & predecessor guard (`sessions/mod.rs:764`):** Current deletion unlinks known files before retiring control-plane state. Before writing screen artifacts to disk, ship a guard-aware predecessor that refuses unsupported screen-enabled sessions before reading, exporting, mutating, or unlinking content.
  2. **Windows durability qualification (`canonical_durability.rs`):** The current canonical durability gate explicitly refuses Windows first-create/rename for manifest install. Qualify that Windows durability path (or define an explicitly approved, honestly labeled durability tier) before advertising durable **Saved** snapshots on Windows.
- **Explicit capture grant & privacy:** Starting audio, enabling accessibility (`918c`), selecting a provider, or reopening a session must never grant screen capture. Capture is strictly bounded to the previewed shared-content pane, and `ScreenContent` is a separate data class (local-only by default; hosted vision requires the Provider Addition Content-Egress Checklist).

### 2. Single permissive offline CPU OCR baseline: Tesseract 5 + official `tessdata_fast`

- **Baseline engine:** Ship **Tesseract 5** (LSTM-only mode, Apache-2.0) + **Leptonica** (permissive BSD-style license) + pinned official **`tessdata_fast`** (`eng.traineddata`, Apache-2.0) behind a thin in-process Rust C-ABI adapter running on a bounded worker thread over in-memory crop pixels (no subprocess, no temp files, no external `PATH` dependency, no implicit download).
- **Why not `Windows.Media.Ocr`:** Microsoft documents `Windows.Media.Ocr` only for desktop apps with MSIX package identity, whereas AudioGraph ships NSIS installers and a standalone `.exe` (and requires no OS OCR language-pack install).
- **Why not `ocrs` weights:** Despite permissive code, `ocrs`'s pretrained weights are licensed **CC-BY-SA-4.0**, which is incompatible with a permissive baseline distribution (even via optional download).
- **Comparators / fallback:** Keep **Apple Vision** (`VNRecognizeTextRequest`) as a measured comparator on macOS only (not a second required shipping backend unless it shows a material measured advantage on the slide corpus). If Tesseract misses the slide/table accuracy gate, evaluate a pinned **PP-OCR ONNX** pair (Apache-2.0, auditing dictionaries and runtime separately).

---

## Acceptance Criteria for Phase 0 Ratification (`audio-graph-40ad`)

1. **v1 regression:** Existing `fnv1a64:4eb27818db1f8b3d` golden and speech-only replay outputs remain bit-identical with screen snapshots created, revised, OCR-updated, expired, or purged.
2. **Compatibility & crash recovery:** Predecessor binary refuses screen-enabled sessions before content read/unlink; injected crash/ENOSPC at staging, blob write, and manifest commit never exposes a half-saved frame; single-frame purge strips derivatives and leaves a truthful unavailable ref; full-session purge removes all managed artifacts or reports retryable residuals.
3. **Packaged offline OCR proof:** Clean, network-blocked Windows 11 NSIS + standalone `.exe`, macOS 14+, and Linux AppImage/`.deb` pass the same pinned slide canary (exact words, decimals, signs, units, in-bounds boxes) with pinned versions/hashes in the SBOM/notices.
4. **Consent & zero-egress proof:** Off/cancel/revoke/stale-epoch fixtures write zero pixels or OCR; crop canaries never escape the approved rectangle; local-only OCR fixtures write zero content bytes to the network.
