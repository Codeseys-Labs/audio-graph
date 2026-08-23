# Live workspace epic (audio-graph-a6b5): design constraints

Conductor-authored constraint set for the design panel. Sources: four maintainer ratifications (2026-08-22), field triage wf_7a0094c0-ec9 (session c95d21e6), UI recon (ui-recon-field3), standing ADRs.

## Ratified decisions (fixed, not up for redesign)

1. **Notes** become one living markdown document per session. Model-owned. Headings and nested bullets. Patched in place every projection tick. The card list goes away as the primary surface.
2. **Live KG** renders above the notes with a user-choosable display mode. Default is a focused graph strip auto-centered on recently mentioned entities with a total-size counter and click-to-expand. Full canvas and a textual change feed are optional modes.
3. **Bento workspace**, whole live-session surface, phased. Transcript, live KG, notes document, and agent queue are tiles. Phase 1 ships a fixed default layout (transcript left, KG strip above notes right, agent queue right sidebar). Phase 2 ships show/hide, resize, arrange, with persistence. Settings modal is out of scope.
4. **Agent tile** shows an actionable approve/reject proposal queue on top and a compact activity feed below. Approvals stay explicit. Auto-apply-with-undo is deferred and needs its own ratification.

## Recon facts (design against these, verified 2026-08-23)

- `AgentProposalsPanel` already exists: card list with per-kind approve/ask/dismiss, reading `s.agentProposals` merged with `liveAssistCards`. Mounted at App.tsx:506-513 as a full-width strip BELOW notes+transcript, gated on `hasAgentActivity`. The pending-count badge was removed deliberately (SystemDrawer.tsx:17-23 documents the reach reduction). The current right-hand aside is `SpeakerPanel`.
- Capture layout is a 2-column grid (`.workspace-panel--capture`, layout.css:116-161): NotesPanel col 1, LiveTranscript col 2, proposals strip spanning both, single-column below 1120px.
- `KnowledgeGraphViewer` (ForceGraph2D, lazy) mounts in exactly one place: SessionsBrowser replay lens. Zero live-session mount. Both graph data lanes (projection `materializedProjectionGraph` preferred, legacy `graphSnapshot` fallback) already update live; the gap is mounting only.
- Notes are ALREADY diff-patched: `applyProjectionNotesPatch` (store/index.ts:402-469) applies `upsert_note`/`delete_note`/`reorder_note` with sequence gating; the list is keyed by note id. A separate `MATERIALIZED_NOTES_UPDATE` handles wholesale resync.
- Update cadence is event-driven off ASR finalization: `observe_projection_schedulers_for_asr_revision` gates on final/end-of-turn, calls `ProjectionSchedulers::observe_ledger`. Two independent lanes (notes/graph), TTFT-aware pacing (`ttft_estimate_ms` default 1200, overwritten by observed), `coalesce_span_threshold: 2`, attempt budget 3, deferred retry 60s. Any new cadence work hooks `ProjectionSchedulers`; no second polling mechanism.
- Event map: useTauriEvents.ts:8-37; names at :135-153 must match src-tauri/src/events.rs.

## Field data (triage wf_7a0094c0-ec9, session c95d21e6)

- Notes emission today: 93/93 final notes are single-utterance verbatim quote captures (`claim_class=verified_quote`), full-replace upserts (379 upsert, 0 delete, 0 reorder), body length oscillates rather than accretes. 72% of repeat upserts carry zero net information (37% byte-identical, 35% tags-only churn). The living document needs (a) a diff/append mutation primitive at the contract level or a document-section op vocabulary, and (b) a shift from per-utterance quote capture to topic-level synthesis. Neither exists.
- Projection lanes run near-saturated: ~15.4-15.8 ticks/min, generation latency ~81% of median tick interval (~650ms headroom). A third lane or heavier prompts eat that headroom; design must state its token/latency budget.
- ~82 of ~195 applies carried `MissingCurrentSpan` staleness (basis lags live spans by >=1 revision). The KG strip's "live" claim is bounded by this; the tile should communicate recency honestly (T2 tone-law spirit).
- Relation vocabulary is exploded (123 types / 73 single-use, seed 9366 has the distribution). KG strip rendering should not assume a clean edge vocabulary yet.
- Question entries are minted from utterance fragments (seed 104f). The agent tile inherits this input quality problem; design the tile so queue-entry quality gating (104f's fix) slots in without a tile redesign.

## Standing constraints

- Replay compatibility is blocker-class (ADR-0045: no accepted patch silently discarded; e700 precedent). Any new patch-op vocabulary must version cleanly: old sessions with note-card ops must still replay, new doc ops must not break old builds' readers if feasible, and the migration story for existing sessions must be explicit (tolerate or convert, never error).
- Prompt discipline: PROJECTION_STABLE_PREFIX_MESSAGE_COUNT=2 cache-stable prefix; variable content after. STRUCTURED_OUTPUT_MIN_MAX_TOKENS=4096 floor; ADR-0038 forbids runtime escalation after Truncated. ADR-0025: movement facts carry counts only, never content.
- ADR-0013 governs conversation-mode write sites; the agent tile's approve path goes through the existing approval commands (approve_agent_proposal), which 4b52 made timestamp-safe.
- Frontend: i18n add-only en+pt, pt chips <=18 chars; unlayered styles/index.css barrel beats @layer recipes (0922 trap — delete BEM properties when replacing, verify in production bundle); readiness/status chips route through the T2 tone law (readinessChipTone).
- Phase 1 of bento is a FIXED default layout. No drag/resize machinery in phase 1 tickets. Persistence schema may be designed now but ships in phase 2.
- One Rust lane at a time on this box; tickets should keep Rust and frontend work separable where possible.

## Interacting open seeds

- 104f question-fragment gating (feeds agent tile input quality).
- 7462 notes-lane redundancy (may be subsumed by the doc model's emit-delta contract; do not double-fix).
- 9366 relation vocabulary design (shapes KG strip edge rendering).
- 64e3 transcript tail loss, fa56 abandoned deferred retry (lifecycle bugs, independent lanes, but the doc/KG can currently cite text the transcript lacks — tiles should tolerate that inconsistency gracefully until fixed).
- 586b diarization fallback notice (a degradation banner could be a workspace-level surface; note the hook, do not design it here).

## Non-goals

- Auto-apply agent proposals (deferred, needs ratification).
- Settings modal changes; mobile/wearable layouts.
- Closed relation ontology (9366 owns it).
- Editing the living document by hand (co-authoring is a later product decision; phase 1 is read-only model-owned).
