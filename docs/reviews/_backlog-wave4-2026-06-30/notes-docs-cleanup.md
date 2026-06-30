# Backlog Wave-4 — lane `docs-cleanup` — notes

Date: 2026-06-30. Base: `8962dab` (verified; FieldRow.tsx + bedrock.rs content-egress guard present).
Worktree-isolated; no `.github/**` edits; commit-per-item.

## eee9 (LOW task) — `'error'` credential source overstated as live-emitted

**Decision: DOCUMENT (preferred lower-risk option), do not emit.**

Grounding (read the code, not just the docs):
- Live IPC path is `commands::load_credential_presence_cmd`
  → `try_load_credentials_with_source()` (on failure returns
  `AppError::CredentialFileError`, NOT a presence row) → on success
  `credential_presence_from_snapshot` builds rows with
  `source: snapshot.source_for(key)`.
- `CredentialSnapshot::source_for` (`credentials/mod.rs:171`) returns only
  `"missing"` (when not present) or a real backend source from `key_sources` /
  the snapshot source (`os_keychain`, `imported_file`, `file_fallback`,
  `file_override`, `credentials_yaml`). It **never** returns `"error"`.
- The sole `"source": "error"` literal (`credentials/mod.rs:1783`) is inside a
  `#[cfg(test)]` smoke test asserting an error payload omits plaintext.
- `credentialSourceContract.test.ts` strips the `#[cfg(test)]` module
  (`extractBackendSources`, L67-68) before extracting the backend vocabulary, so
  `"error"` is **not** part of the asserted live source set. The test only
  requires that backend-*emittable* sources have labels.

Conclusion: `'error'` is a defensive UI-only fallback label, never produced on
the live path today. Lowest-risk fix is to stop the docs/code from implying it is
live-emitted, while keeping the label + `LOCALIZED_CREDENTIAL_SOURCES` entry +
both i18n locale keys so (a) the contract-test parity stays intact and (b) a
future backend that surfaces a per-key read error gets a localized string instead
of a raw passthrough. No behavioral change, no coverage removed.

Changes:
- `src/components/ProviderReadinessPanel.tsx` — comment over
  `LOCALIZED_CREDENTIAL_SOURCES` documenting that `error` is a defensive fallback
  not emitted on the live path (and why the `#[cfg(test)]` literal doesn't count).
- `docs/SETTINGS_DESIGN.md` — source table row for `error` changed from
  "The credential store could not be read (IPC failure)." to
  "**Defensive UI fallback — not emitted on the live IPC path.**" plus a prose
  paragraph explaining the live path returns `AppError::CredentialFileError`.

Gate (eee9 touches `.tsx` → JS gate required): `bun run typecheck` clean,
`bun run check` clean (130 files), full `bun run test` 668/668 across 50 files
(two independent runs, both exit 0). Focused run of
`credentialSourceContract.test.ts` + `ProviderReadinessPanel.test.tsx` +
`locale-parity.test.ts` = 22/22.

Commit: `9497820`.

## a79d (LOW task) — refresh diarization docs for SpeakerTimeline ledger + Clustering backend

Grounding: read `diarization/mod.rs` (`Simple` / `Sortformer` /
`Clustering` `DiarizationBackend`, `SORTFORMER_MAX_SPEAKERS = 4`,
`overlap_speaker_for_segment`), `projections.rs` (`SpeakerTimeline`,
`DiarizationSpanRevision`, `validate_diarization_basis`, provider-neutral
`span_id`, `provider_speaker_id` provenance-only, `channel` field),
`speech/mod.rs` (`make_diarization_config`, `maybe_spawn_clustering_diarization`,
`diarization_span_revision_for_transcript`), `diarization/worker.rs` +
`diarization/stabilize.rs`, `models/mod.rs` (`ModelStatus` =
whisper/llm/sortformer only; clustering models pyannote-seg-3.0 + TitaNet
registered but no `ModelStatus` field), and ADR-0017 + the Wave-3
`ARCHITECTURE.md` "Speaker Timeline and Diarization Normalization" section
(canonical four-concept structure, mirrored here).

Changes:
- `docs/designs/provider-architecture.md`:
  - "Speaker labels" product row (~L88) now names the three local backends and
    states all paths normalize into the provider-neutral `SpeakerTimeline`
    revision ledger (eb6c); status note adds the pending clustering accuracy gate.
  - New `#### Speaker diarization and the SpeakerTimeline ledger` subsection under
    §1 ASR, covering the four concepts (local diarization / provider diarization /
    metadata join / research-gated physical multi-channel projection), aligned
    with ARCHITECTURE.md / DATA_FLOW.md / ADR-0017.
  - OpenAI Realtime "Diarization fallback" bullet now links the new subsection.
- `docs/SETTINGS_DESIGN.md`:
  - "Current code note" diarization-mode line now explains the `diarization`
    setting selects Simple / Sortformer / Clustering and that all outputs +
    provider labels feed the `SpeakerTimeline` ledger (eb6c).
  - `ModelStatus` TS block annotated: `sortformer` is the only diarization-model
    readiness field today; Clustering models download but are not yet a
    `ModelStatus` field (P2 UI work, ADR-0017). The type itself is unchanged
    because the Rust `ModelStatus` (models/mod.rs:355) has no clustering field —
    not overstated.
  - Sortformer model-card mock annotated that Sortformer is the ≤4 backend, the
    Clustering backend is feature-gated/not-yet-surfaced, and all backends feed
    the ledger.

Consistency check: the four diarization concepts now read consistently across
`provider-architecture.md`, `SETTINGS_DESIGN.md`, `ARCHITECTURE.md` (Wave 3),
`DATA_FLOW.md`, and ADR-0017. All new relative links resolve (verified;
ADR-0017 / speaker-channel-routing / ARCHITECTURE / DATA_FLOW all present).

Gate (a79d is docs-only, no `.ts/.tsx`): markdown well-formed; all newly-added
relative links resolve to real files; claims grounded in the code/symbols listed
above.

## Out-of-scope problems found (filed as newSeeds)
- Pre-existing broken link `docs/SETTINGS_DESIGN.md` L1040 →
  `[`src/App.css`](../src/App.css)` — `src/App.css` does not exist. Untouched by
  this lane (it is in the File Changes Summary table, unrelated to the diarization
  /credential rows). Filed as a newSeed.
