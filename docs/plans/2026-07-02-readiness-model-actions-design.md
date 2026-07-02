# Design: Download/Delete model actions on provider-readiness cards

Date: 2026-07-02
Status: Design (read-only investigation; no code changed)
Scope: Add per-model Download/Delete controls to the provider-readiness rollup
cards for LOCAL model-backed providers, reusing the existing model-management
store actions and backend commands. Cloud providers must NOT get a button.

---

## 1. Classification

**canMapReadinessToModel = `needs-lookup`** — but the lookup already exists and
is a *pure, in-memory, frontend* map. No new backend field, no new command, no
new plumbing is required.

The join key is NOT a field on `ProviderReadiness`. It is
`ProviderDescriptor.local_models[].model_id`, which is byte-for-byte the same
string as `ModelInfo.filename` (both are the `models/mod.rs` filename constants).
The descriptor is already in memory on the frontend
(`PROVIDER_DESCRIPTORS.get(entry.provider_id)`), and the `ModelInfo[]` list is
already in the store (`useSettings().models`). So the "lookup" is a
`filename === model_id` array join over data the panel already holds.

### Why not `direct`

The tempting direct field is `ProviderReadiness.runtime.model_id`
(`src/types/index.ts:1206`, Rust `commands.rs:234`). It is unusable as the join
key for two reasons:

1. It is `None`/absent in exactly the `ModelMissing` state — the case where we
   need a **Download** button. See `moonshine_runtime_readiness_from_state`
   (`commands.rs:6756-6764`, `model_id: None`) and
   `diarization_clustering_runtime_readiness_from_state`
   (`commands.rs:6866-6874`, `model_id: None`). It is only populated in the
   `Healthy`/`LoadFailed` branches (`commands.rs:6785`, `6895`).
2. When present it can be a *composite* runtime id, not a download filename:
   `diarization_clustering_runtime_model_id()` returns
   `"<pyannote-dir>+<titanet-file>"` (`commands.rs:6802-6808`) — two files joined
   with `+`, which matches no single `ModelInfo.filename`.

`ProviderReadiness` has no `model_id`/`local_path`/`model_name` field at all
(Rust struct `commands.rs:248-269`; TS `src/types/index.ts:1209-1224`). The
CONTEXT note that pointed at "src/types/index.ts ~486-495" was actually pointing
at `ModelInfo` (`src/types/index.ts:478-487`), not `ProviderReadiness`.

### Why not `no-clean-map`

There is a clean map: the descriptor's `local_models` list is the canonical
declaration of which on-disk model files each local provider needs, and the
backend readiness probe itself keys off the identical `model.model_id`
(`local_model_readiness_summary`, `commands.rs:6629-6665`,
`models_dir.join(model.model_id)`) — the same `models_dir.join(filename)` that
`download_model`/`delete_model`/`list_models` use
(`models/mod.rs:481`, `587`, and the delete path). The equality is enforced by
construction and by tests (`provider_registry.rs:262-336`).

---

## 2. The verified join chain

```
ProviderReadiness.provider_id            (e.g. "asr.local_whisper")
  -> PROVIDER_DESCRIPTORS.get(provider_id)               (frontend, in memory)
  -> descriptor.local_models: LocalModelRequirement[]     (src/types/index.ts:1136-1140)
       each has .model_id  ===  ModelInfo.filename
  -> models.find(m => m.filename === model_id)            (useSettings().models: ModelInfo[])
  -> m.is_downloaded / m.is_valid  -> Download vs Delete
  -> downloadModel(m.filename) / handleDeleteClick(m.filename)
       -> download_model_cmd { modelFilename }  (store/index.ts:2369; commands.rs:3082)
       -> delete_model_cmd   { modelFilename }  (store/index.ts:2449; commands.rs:3236)
```

### Evidence that `model_id === filename`

Registry `local_models` literals
(`src-tauri/crates/provider-registry/src/lib.rs`):

| provider_id                | local_models[].model_id (constant)                                   | kind      |
|----------------------------|-----------------------------------------------------------------------|-----------|
| asr.local_whisper          | `WHISPER_MODEL_SMALL_EN` (lib.rs:1195)                                 | File      |
| asr.sherpa_onnx            | `SHERPA_ZIPFORMER_20M` (lib.rs:1201)                                   | Directory |
| asr.moonshine              | `MOONSHINE_SMALL/MEDIUM/TINY_STREAMING_EN` (lib.rs:1207-1220)          | Directory |
| diarization.sortformer     | `SORTFORMER_MODEL_FILENAME` (lib.rs:1225)                             | File      |
| diarization.clustering     | `DIAR_SEG_PYANNOTE_DIR` + `DIAR_EMB_TITANET_FILENAME` (lib.rs:1232-1239)| Dir+File  |
| llm.local_llama            | `LLM_MODEL_FILENAME` (lib.rs:1244)                                     | File      |
| llm.mistralrs              | `LOCAL_LLM_MODELS` (lib.rs:2287, same `LLM_MODEL_FILENAME`)            | File      |

Model catalog `MODELS[].filename` (`src-tauri/src/models/mod.rs:182-304`) uses
the **same constants** (`WHISPER_MODEL_SMALL_EN`, `SHERPA_ZIPFORMER_20M`,
`MOONSHINE_*`, `SORTFORMER_MODEL_FILENAME`, `DIAR_SEG_PYANNOTE_DIR`,
`DIAR_EMB_TITANET_FILENAME`, `LLM_MODEL_FILENAME`). Hence
`local_models[].model_id` is guaranteed to be a valid `download_model_cmd`
filename. `download_model` explicitly rejects any filename not in `MODELS`
(`models/mod.rs:583-584`), so a mismatch would be a hard error, not silent
corruption.

### Local vs cloud detection

A readiness entry is local model-backed iff its descriptor is a local-files
provider. Two equivalent frontend predicates, both already available:

- `PROVIDER_DESCRIPTORS.get(entry.provider_id)?.model_catalog === "local_files"`
  (`ModelCatalogPolicy`, `src/types/index.ts:889-895`), OR
- `(descriptor?.local_models.length ?? 0) > 0`.

Cloud providers (`asr.deepgram`, `asr.assemblyai`, `asr.soniox`,
`llm.openrouter`, `llm.cerebras`, `llm.aws_bedrock`, all `*.api`, gemini, etc.)
have `local_models: &[]` (lib.rs:1474, 1502, 1530, …) and a non-`local_files`
catalog policy, so they render **zero** model-action buttons. This is the
mechanism that satisfies "cloud providers must NOT get a download button" —
they simply produce an empty `local_models` array to iterate.

---

## 3. Existing pieces to reuse (no new state needed)

Everything the button needs is already threaded through `useSettings()` (the
`useSettingsController` return object), because the controller pulls it from the
store and `CredentialsPanel` already calls `useSettings()`:

| Need                     | Source (already in `useSettings()`)                                   | Def site |
|--------------------------|-----------------------------------------------------------------------|----------|
| `models: ModelInfo[]`    | store `list_available_models`                                         | useSettingsController.tsx:913, 3479; store/index.ts:2360 |
| `downloadModel(fn)`      | store action -> `download_model_cmd`                                  | useSettingsController.tsx:924, 3416; store/index.ts:2366-2369 |
| `handleDeleteClick(fn)`  | controller (2-click confirm) -> `deleteModel` -> `delete_model_cmd`   | useSettingsController.tsx:3306-3313, 3437 |
| `deleteModel(fn)`        | store action -> `delete_model_cmd`                                    | useSettingsController.tsx:925, 3410; store/index.ts:2446-2449 |
| `confirmDelete`          | controller state (which filename is armed for delete)                 | useSettingsController.tsx:3388 |
| `downloadProgress`       | store, `DownloadProgress \| null`                                     | useSettingsController.tsx:917, 3417; store/index.ts:2357 |
| `isDownloading`          | store bool                                                            | useSettingsController.tsx:916, 3461 |
| `isDeletingModel`        | store `string \| null` (filename)                                    | useSettingsController.tsx:918, 3460 |
| `modelStatus`            | store `ModelStatus \| null` (optional, for badge)                    | useSettingsController.tsx:914, 3478 |

**Download progress + confirm-delete reuse: YES.** The readiness card can reuse
`downloadProgress` and the two-click `confirmDelete` flow verbatim. The
progress event matches on `downloadProgress.model_id === model.filename`
(store keys `DownloadProgress.model_id` to the filename; TS note at
`src/types/index.ts:490`), and `CredentialsManager.tsx:199-207` shows the exact
match/gating logic to copy. There is a single global `downloadProgress` /
`isDownloading` (one download at a time), so a filename-equality guard is
required so the readiness card only lights up the row being downloaded — same as
`CredentialsManager` does today.

### Readiness (Download vs Delete vs nothing) per row

Use `ModelInfo.is_downloaded` / `is_downloaded && is_valid` directly — the
backend `list_models` already computes these correctly for File, Directory
(archive), and Component kinds (`models/mod.rs:422-434`, `verify_archive_dir`
`413-420`), including the truncated-file floor via `min_model_size_bytes`
(`models/mod.rs:175-179`). Do NOT reimplement readiness from the descriptor.
The exact tri-state rule is already in
`CredentialsManager.readinessForModel` (`CredentialsManager.tsx:137-156`):
`!is_downloaded -> NotDownloaded`, else `is_valid ? Ready : Invalid`. Show
Download when `!is_downloaded`, Delete when `is_downloaded`.

---

## 4. Mapping approach the Build stage should implement

Add a small presentational sub-component (call it `ReadinessModelActions`) that
`CredentialsPanel` renders inside each `visibleProviderReadiness` card, gated on
the provider being local. Pull the extra fields from the same `useSettings()`
call already at `CredentialsPanel.tsx:60-80`.

Pseudocode for the per-card block (inserted near the recovery/details block in
`CredentialsPanel.tsx`, ~line 190-211):

```ts
const descriptor = PROVIDER_DESCRIPTORS.get(entry.provider_id);
const localReqs = descriptor?.local_models ?? [];           // [] for cloud -> renders nothing
const rows = localReqs
  .map((req) => models.find((m) => m.filename === req.model_id))
  .filter((m): m is ModelInfo => Boolean(m));                // guard: skip unknown filenames
// then, per row, exactly the button + progress markup from CredentialsManager.tsx:238-283:
//   !m.is_downloaded -> <Button onClick={() => downloadModel(m.filename)} disabled={isDownloading}>
//   m.is_downloaded  -> <Button variant="danger" onClick={() => handleDeleteClick(m.filename)}
//                         disabled={isDeletingModel === m.filename}>  // confirmDelete === m.filename => "Confirm?"
//   progress: downloadProgress?.model_id === m.filename && status !== "complete" -> reuse describeDownloadProgress
```

The Build stage SHOULD lift/reuse the pure helpers already exported from
`CredentialsManager.tsx` — `describeDownloadProgress` (line 87) and the
button/progress JSX (238-283) — rather than duplicating them, to keep one source
of truth for progress/ETA formatting. `readinessForModel`
(`CredentialsManager.tsx:137-156`) can also be reused for the per-row badge if a
badge is desired.

### Design decisions to flag to Build

1. **Whisper card surfaces only `ggml-small.en.bin`.** `asr.local_whisper`'s
   `local_models` lists only `WHISPER_MODEL_SMALL_EN` (lib.rs:1195), even though
   the full catalog has 5 whisper variants (`models/mod.rs:182-227`). Iterating
   `local_models` therefore shows one Download/Delete button on the whisper
   readiness card (the model that provider actually requires). The other four
   variants stay in the full `#settings-models-section` (`CredentialsManager`).
   This is the correct scoping — the readiness card is "what this provider
   needs", not "the whole catalog" — but call it out so it is not read as a bug.
2. **Clustering shows two rows** (pyannote dir + TitaNet file); both must be
   present for the provider (`local_model_readiness_summary` requires
   `ready >= total`, commands.rs:6865). Render both as independent rows.
3. **Single global download slot.** `isDownloading`/`downloadProgress` are
   global; if a download is running (from this card OR the models section),
   Download buttons on other rows must be `disabled={isDownloading}` exactly as
   `CredentialsManager.tsx:245` does, to avoid concurrent downloads.
4. **i18n reuse.** The `settings.buttons.download/downloading/delete/
   confirmDelete/deleting` and `settings.models.downloadProgress*` keys already
   exist (used by `CredentialsManager`); reuse them, no new strings required.

---

## 5. Files the Build stage will touch / reference

- EDIT `src/components/settings/CredentialsPanel.tsx` — render the actions block
  per local readiness card; add `models, downloadModel, handleDeleteClick,
  confirmDelete, downloadProgress, isDownloading, isDeletingModel` (and
  optionally `modelStatus`) to the `useSettings()` destructure at line 60.
- REUSE (import, do not fork) `src/components/CredentialsManager.tsx` helpers
  `describeDownloadProgress` (87), and ideally factor the button/progress JSX
  (238-283) + `readinessForModel` (137-156) into a shared
  `ReadinessModelActions`/model-row component.
- REFERENCE `src/components/providerRegistryHelpers.ts:171-199`
  (`generatedModelCatalogForProvider`) — proves `descriptor.local_models` is
  already consumed on the frontend and `model_id` is the stable id.
- No backend change. `download_model_cmd` (commands.rs:3082) /
  `delete_model_cmd` (commands.rs:3236) / `list_available_models`
  (store/index.ts:2360) are reused as-is.

---

## 6. Answer summary

- Mapping key: `ProviderDescriptor.local_models[].model_id === ModelInfo.filename`
  (frontend array join over `PROVIDER_DESCRIPTORS` + `useSettings().models`).
- Classification: `needs-lookup`, but the lookup is a pure in-memory map that
  already exists — no new backend field/command.
- Local vs cloud: `descriptor.model_catalog === "local_files"` /
  `local_models.length > 0`; cloud providers have empty `local_models` and get
  no button.
- is_downloaded per row: read `ModelInfo.is_downloaded`/`is_valid` from
  `useSettings().models` (backend `list_models` already handles file/dir/
  component kinds).
- Progress + confirm-delete: reuse `downloadProgress` (filename-gated) and the
  `confirmDelete` two-click flow verbatim from `CredentialsManager`.
