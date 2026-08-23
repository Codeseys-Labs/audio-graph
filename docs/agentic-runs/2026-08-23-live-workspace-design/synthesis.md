# Synthesis — live workspace epic (audio-graph-a6b5)

Judge output over `design-a.md` (experience-first) and `design-b.md` (contract-first),
against `constraints.md`. Every load-bearing claim cited below was re-verified against
the working tree on 2026-08-22 before being relied on; §6 lists what was checked and
what was found wrong. Verdict up front: **B wins the contract, scheduler, and prompt;
A wins the renderer, the tone/a11y layer, the bento markup, and the i18n discipline.
The winning architecture is B below the store and A above it**, with four corrections
that neither design has alone.

---

## 1. Judgment

### 1.1 Design A — verified strengths

- **The typed-outline decision is correct and the reasoning holds.** Verified: no
  markdown renderer in `package.json` dependencies; `@radix-ui/react-popover` exists
  for the gutter disclosure. A re-parsed markdown tree destroying node identity is a
  real thrash mechanism, and A's per-node stable ids + `outlineToMarkdown()` copy path
  honors ratified decision 1 without a new trust-boundary dependency.
- **Four independent field findings, all verified true:**
  1. `materializedGraphToSnapshot` hardcodes `mention_count: 1`
     (`utils/materializedGraph.ts:163`) — any mention-ranking over the projection
     graph is uniform noise.
  2. The 5 `agent.*` status keys exist only as inline `fallback` strings in
     `AgentProposalsPanel.tsx`; neither `src/i18n/locales/en.json` nor `pt.json` has
     them, and `locale-parity.test.ts` structurally cannot catch it. Shipped pt bug.
  3. `useShellLayout` (1280/1024) and `layout.css:144` (1120) are two disagreeing
     layout authorities.
  4. `settings-chip-length-budget.test.ts:53` measures raw literal `.length` —
     the interpolation gap is real (a template rendering to 25 chars passes).
- **The phase-1/phase-2 bento separation is best-in-class.** A's seven markup
  requirements (§4.3) make phase 2 additive: `data-tile` grid-areas, CSS custom
  properties with fallbacks, empty-not-absent header slots, frozen `WorkspaceTileId`
  union with a contract test. B's bento section has none of this.
- **L1/L2/L3 laws** (no autoscroll on rewrite, announce-don't-chase, no claim without
  evidence) are the only interaction/a11y design in either document. B has nothing
  comparable.

### 1.2 Design A — violations and blind spots

- **A's S1/S2 chip table contradicts the law it cites (CONFIRMED).**
  `readinessAxisTone` (`readinessTone.ts:117-119` and its own doc comment) collapses
  an unverifiable `"ready"` to `"unchecked"` **for copy as well as tone** — "the law
  gates the claim, not just its color." So A's phase-1 rendering of neutral
  **"Up to date"** with `automaticProbeAvailable: false` is exactly the leak the law
  exists to stop. Honest phase 1 either renders "Not verified" perpetually (noise) or
  — the synthesis choice — makes a different, *observed* claim: a timestamp. §2.4.
- **A6 (ship the `WorkspaceLayoutPrefs` reader in phase 1) brushes the constraint.**
  "Persistence schema may be designed now but ships in phase 2." A reader that parses
  localStorage is persistence machinery. Cut: phase 1 keeps the frozen tile-id
  contract test and the read-rule spec as types + comments; the reader ships with the
  writer in phase 2. A's "phase-3 fifth tile" argument is real but is phase-2's
  problem to land.
- **O(patches × ops) replay in the view model.** `sessionProjectionEvents` is
  unbounded in the store; memoization bounds re-computation frequency, not cost. The
  synthesis requires the VM to fold **incrementally** (previous VM + newest patch),
  which A's own model shape supports; and W2's ingest filter removes the 37%
  byte-identical class at the source going forward, shrinking the input.
- **A's R4 demand (per-node section path, +480 prompt tokens) is superseded** by B's
  `heading_level` — under B's contract a note *is* a section, so `DocNode.sectionId`
  becomes derived (nearest preceding lower-depth note), and A's adapter absorbs it.
  A designed the seam for exactly this, so the cost is contained in
  `notesToOutline`.
- **Blind spots:** zero prompt/contract work (deliberate, but it means A's phase 1
  alone ships a nicer frame around the same flat 93-quote list — A says this
  honestly in §1.3); force-sim caps reasoned, not measured; the always-mounted agent
  tile must not break the `hasAgentActivity` consumer at `App.tsx:1042-1052`
  (get-started fallback exclusion — verified real; B caught it, A missed it).

### 1.3 Design B — verified strengths

- **The contract asymmetry is real and decisive (ALL CONFIRMED):**
  `load_projection_patches` goes through `load_strict_canonical_stream`
  (`canonical_reader.rs:163-168`); the fail-closed behavior is pinned by
  `commands.rs:12537`; the **only** `deny_unknown_fields` in the subsystem is on
  `ProjectionPatchDraft` (`projection_llm.rs:185`) — `ProjectionOperation` and
  `ProjectionPatch` tolerate unknown fields. A new op variant or third
  `ProjectionKind` loudly destroys whole-session replay in old builds; an additive
  optional field is free. **"No new ops, one `heading_level: Option<u8>`" is the
  correct reading of ADR-0045 and the e700 precedent** (`node_id_redirects` verified
  at `projections.rs:1866+` — tolerant resolution inside existing vocabulary).
- **The empty-patch trap is real (CONFIRMED).** `derive_coverage_heads`
  (`projection_scheduler.rs:1332+`) picks max-sequence per kind, blind to op count;
  `MaterializedNotes::apply_patch` commits `last_sequence` even with zero ops;
  no `operations.is_empty()` early-return exists anywhere in `src-tauri/src`; the
  strict schema's `"operations": {"type":"array"}` has no `minItems`, and
  `validate_projection_patch_draft`'s loop is vacuous on empty. Suppressing empty
  patches would leave the coverage head un-advanced and re-queue the lane forever.
  This must be an explicit acceptance criterion, or someone "cleans it up."
- **The scheduler correction is real (CONFIRMED).** `ttft_estimate_ms` and
  `coalesce_span_threshold` are consumed **only** by `coalescing_reason`
  (`projection_scheduler.rs:932-944`), which picks a diagnostic label on the
  `Coalesced` decision. `observe_ledger` (`:478-566`) implements exactly
  one-in-flight-plus-coalesce. The recon's "TTFT-aware pacing" is a mislabel; the
  real failure mode of a heavier prompt is the document lagging speech, not lane
  saturation, and "~650ms headroom" is a coupled measurement, not a budget. B's
  reframing (protect `coalesced_updates`, watch tick rate rise after the delta
  contract) is the correct latency posture and the synthesis adopts it wholesale.
- **`render_notes_snapshot` recency-ordering** and its self-flagging doc comment
  (`projection_llm.rs:1028-1040`) verified — it is disqualifying for a document, and
  it explains the field data's 0 `reorder_note` uses as a prompt failure, not a
  contract gap. The −~900 tokens/tick outline replacement is the only design in
  either document that makes the prompt *smaller* while adding structure.
- **`MaterializedNotes::SCHEMA_VERSION` no-gate verified** (`persistence/mod.rs:1407`
  is a bare `load_json`); not bumping it is right. Migration table (§1.7) is the
  standard the constraint asked for.
- **7462 subsumption** is correctly argued: the delta contract *is* the fix; keep the
  metric (§2.4's table) as this epic's acceptance gate.

### 1.4 Design B — violations and blind spots

- **§1.5(b)'s seam claim is FALSE as stated (verified).** B says the no-op filter's
  home, `trusted_projection_patch_from_model_json`, "already has the live
  materialized notes available via the same path `projection_patch_prompt_messages`
  uses." It does not: `ProjectionPatchBuildContext` (`projection_llm.rs:194-208`)
  carries sequence/route/prompt metadata only, and the executor call site
  (`llm/executor.rs:941`, inside `projection_outcome_from_output`) has `job` +
  `ledger`, no notes. The filter needs materialized notes **threaded** to the
  admission seam (or the filter moves to the persist site in `speech/mod.rs`, before
  the patch is written). Two consequences: the ticket is M, not S; and the comparison
  target must be pinned as **the materialized state at admission time** (what the
  patch will apply against), not the prompt-time snapshot — the in-flight window
  makes those differ.
- **B's shared-selector KG plan contradicts its own recency computation.**
  B extracts `materializedGraphToSnapshot(...) ?? graphSnapshot` as the one shared
  selector — but the snapshot's `GraphNode` (`types/index.ts:372-384`) has **no
  `updated_by_sequence`**, which B's own "3 highest distinct sequences" focus rule
  requires. The selector must expose the materialized graph alongside the snapshot,
  or the focus mode reads materialized directly (A's position, for A's verified
  reason). Synthesis: one selector returning both; focus consumes materialized,
  legacy-only sessions degrade to `last_seen` ranking.
- **B rewrites `NotesPanel` in place.** `NotesPanel` is also the Sessions replay
  lens and owns `useNotesSynthesis`; an in-place rewrite couples the live surface to
  the replay surface and turns every live-document iteration into a replay-lens
  regression risk. A's move — new `LiveDocument` workspace components, `NotesPanel`
  untouched in phase 1 — is strictly safer and costs one adapter.
- **B has no answer to churn the source-fix can't reach:** tags-only churn (35%)
  passes B's byte-identical tuple filter (tags differ); pre-R2 recorded sessions and
  the rollout window still replay full-churn logs. A's view-layer `contentHash`
  (tags excluded) handles all of it with zero contract risk. These are **different
  layers, not a double-fix**: W2's filter fixes the log; A's hash decides what
  *pulses and counts* — presentation. 7462's metric lives with W2 only.
- **No interaction/a11y design at all.** No scroll policy, no announcements, no
  reduced-motion, no empty/failure matrix. B's bento is "a grid change" with the
  verified-inconsistent 1120px reflow inherited as-is.
- **Two product calls correctly flagged but left as recommendations** (ontology
  sections out of the live tile; `synthesize_notes` output not rendered inline).
  Both change ADR-0014's surface and are ratification gates here (R4), not ticket
  footnotes.

### 1.5 Where they agree (adopted without further argument)

No third lane; no new polling; the word "Live" appears nowhere; tone routes through
`readinessChipTone`; uniform edge strokes with hover-only, `escapeHtml`-guarded
relation labels (9366 open); node color by the closed `entity_type` enum; agent
approve path untouched (`approve_agent_proposal`, ADR-0013/4b52); no global pending
badge (SHELL-R3 respected — in-tile count only); feed rendering capped without
capping the store array; 586b banner row noted, not designed; 64e3/fa56 tolerated
silently (no dead jump affordances, no inconsistency banner).

---

## 2. The winning architecture

**Contract & backend = B, corrected.** One additive `heading_level: Option<u8>`
(`#[serde(default, skip_serializing_if)]`) on `UpsertNote` + `MaterializedNote`;
`None` means "asserted no structure," never "level 2." Body is B's validated plain
grammar (lines + `- ` bullets, ≤2 indent levels, no inline markup, no HTML path),
enforced by a clamp-not-refuse normalizer sibling of
`normalize_projection_patch_draft_ontology` at the fresh-ingest-only seam.
`validate_operation` untouched for structure. No `SCHEMA_VERSION` bump. Prompt
deltas A/B/C as B wrote them (section-not-card guidance; one `EVIDENCE_GUIDANCE`
sentence; document-order outline replacing `render_notes_snapshot`, −~900
tokens/tick). Ingest no-op filter with the **plumbing correction** (§1.4): notes
threaded to the admission seam, comparison against admission-time state, empty
patches persist. Two lanes, zero scheduler change, B's corrected latency posture.

**Renderer & workspace = A, adapted.** Typed outline VM (`liveDocumentModel.ts`)
with two adapters; under the ratified contract `notesToOutline` maps note→section
(`heading_level ?? legacy depth`) and parses the W1 body grammar into bullets —
A's synthetic `${id}#${i}` line-splitting is replaced by the grammar parser, so
frontend and any future exporter agree with the Rust normalizer by construction.
Legacy mode (all-`None`) renders old sessions flat — replay compat at the view
layer on top of replay compat at the wire, which is the belt-and-suspenders ADR-0045
posture. `contentHash` excludes tags; store artifact stays byte-faithful
(`ProjectionReplayArtifactReport` parity untouched). Gutter+popover provenance;
L1/L2/L3 laws; A's bento markup contract and grid; A's i18n budget including the
5 missing `agent.*` keys and the interpolating chip-budget sibling test.

**The recency chip, resolved (fixes A's law contradiction with B's observed claim).**
Phase 1 renders a **neutral observed fact** — "as of HH:MM:SS" (B) — plus A's
`turnsBehind` escalation: ≥3 turns behind → caller-owned `"behind"` status through
the tone law → warning "−N turns." `"Up to date"`/success **never renders in
phase 1**. W3's additive `basis_currency_at_apply` is the evidence that upgrades
the chip: `Current` basis → success "Up to date"; `AppendedTail` → neutral. One
component, three honesty tiers, each earned. `turnsBehind` derives from
`asrSpanRevisions` `is_final && end_of_turn` (verified fields, same predicate the
scheduler gates on).

**KG strip.** One shared selector exposing `{materialized, snapshot}`; focus set =
B's honest definition (nodes touched by the last 3 distinct `updated_by_sequence`
values, active only) + A's mechanics (cap 12, 3-tick hysteresis, both-endpoints
edge rule, re-heat only on id-set change, no canvas mount in feed mode/off-screen).
`canvas` mode is the shipped `KnowledgeGraphViewer` verbatim; `feed` is the a11y /
low-power / Suspense-fallback path. Labels say "recently updated," never
"currently discussed."

**Agent tile.** B's zero-new-state extraction (merge → store selectors, queue/feed
split preserving `actionableProposalIds`) + B's `admitToQueue` predicate as 104f's
slot + A's chrome (queue-on-top/feed-below, approve-fail stays in queue with
`role="alert"` + retry, debounced sr-only announcements, `statusClass()` deleted
into `ag-chip[data-tone]` via the tone law — approved-without-outcome demotes to
neutral). A's quality classifier ships as a separate gated ticket behind the
Signal/All toggle. Tile always mounted in the bento (R3); `hasAgentActivity`
survives for the get-started exclusion.

---

## 3. Ticket cut

Lanes: **max one Rust ticket in flight** (W1→W2→W3 strictly serialized). Frontend
tickets run parallel to the Rust lane; W4 unblocks the rest. Legacy mode means no
frontend ticket is blocked on Rust.

**Global tripwires (every implementer):**
- **0922 CSS trap:** when replacing rules in the unlayered `styles/index.css`
  barrel, *delete* the old BEM/utility properties (they beat `@layer` recipes);
  verify in the production bundle, not dev.
- **Tone law:** every freshness/status chip routes through
  `readinessChipTone`/`readinessAxisTone`; an unverifiable "ready" loses its copy,
  not just its color. The word "Live" is banned from every new surface.
- **i18n:** add-only en+pt; pt chip strings ≤18 chars measured **after**
  interpolation; no key ships as `defaultValue`-only.
- **Prompt-prefix discipline:** `PROJECTION_STABLE_PREFIX_MESSAGE_COUNT = 2`; no
  runtime branching in messages [0]/[1]; constants may change once at rollout.
- **Replay:** no accepted patch silently discarded (ADR-0045); fresh-ingest-only
  normalization; fixtures for pre-change logs are part of done, not follow-up.

| # | Title | Lane | Size | Deps | Gates |
|---|---|---|---|---|---|
| **W1** | `heading_level` contract field + body-grammar normalizer + replay fixtures | Rust | M | — | — |
| **W2** | Prompt deltas A/B/C + document outline + ingest no-op filter + metrics | Rust | L | W1 | R5 |
| **W3** | Additive `basis_currency_at_apply` on emitted patch → frontend | Rust | S | W1 (lane-serialized after W2) | — |
| **W4** | Bento grid + `WorkspaceTile` contract + breakpoint realignment | FE | M | — | R1, R2 |
| **W5** | `liveDocumentModel` + `LiveDocument` (sections, legacy mode, dedupe, provenance) | FE | L | W4 | R4 |
| **W6** | Recency chips + `liveWorkspaceTone` + chip i18n + interpolating budget test | FE | M | W4, W5 | — |
| **W7** | KG strip: shared selector, focus canvas, canvas/feed modes | FE | M | W4 | — |
| **W8** | Agent tile: selector extraction, admission seam, tone chips, missing `agent.*` keys | FE | M | W4, W6 | R3 |
| **W9** | Queue quality classifier behind Signal/All toggle | FE | S | W8 | R6 |
| **W10** | L1/L2 polish: change anchor, sticky-follow, pulse + reduced-motion | FE | S | W5 | — |

### W1 — `heading_level` + body grammar (Rust, M)
Add `heading_level: Option<u8>` to `ProjectionOperation::UpsertNote`
(`projections.rs:1473`) and `MaterializedNote`, copied through `upsert_note`.
Doc comment: `None` = no structure asserted, never a default depth. Add
`normalize_projection_patch_draft_doc_structure` beside the ontology normalizer in
`parse_projection_patch_draft` (`projection_llm.rs:666`): clamp `heading_level` to
2..=4, normalize body markers (`*`/`+`→`-`, indents→0/2/4, strip `#` and inline
emphasis), never truncate or drop a line. **Ships dark — no prompt change.**
*Acceptance:* (1) pre-doc fixture log replays to byte-identical `MaterializedNotes`;
(2) a serde test proves a new-shape record deserializes under the pre-W1
`ProjectionOperation` (unknown-field tolerance pinned); (3) hand-authored strict
schema (`projection_llm.rs:552-561` region) gains `("heading_level",
nullable_integer())` AND a new parity test asserts variant-field sets match between
schemars and strict schemas for Notes; (4) TS `ProjectionOperation` gains
`heading_level?: number | null`; (5) empty-ops patch advances the coverage head
(test through `derive_coverage_heads` + `apply_patch`).
*Tripwires:* no `deny_unknown_fields` anywhere new; do **not** bump
`MaterializedNotes::SCHEMA_VERSION` (no reader gates it — verified); structure
never enforced in `validate_operation` (clamp, don't refuse); the three schema
surfaces (schemars / strict / TS) move in lockstep or the strict route silently
forbids what the prompt demands.

### W2 — prompt + delta contract (Rust, L)
Rewrite the Notes `operation_guidance` arm (single constant, keep the
no-runtime-branching posture of `projection_llm.rs:850-859`); append B's one
`EVIDENCE_GUIDANCE` sentence (R5); replace `render_notes_snapshot` with the
document-order outline (`DOC_OUTLINE_*` budget constants, previews for 8 most
recently changed, degradation drops previews first, every id shown-or-counted —
preserve 253c's guarantee). Ingest no-op filter dropping `UpsertNote` ops whose
`(title, body, tags, heading_level)` tuple is byte-identical to the
**admission-time** materialized note — with the plumbing this actually needs
(thread `Option<&MaterializedNotes>` to `projection_outcome_from_output` /
`trusted_projection_patch_from_model_json`, or filter at the persist seam in
`speech/mod.rs`; `ProjectionPatchBuildContext` does not carry notes — verified).
Add per-kind `Truncated` counter; surface `coalesced_updates`.
*Acceptance:* byte-identical re-upserts 0%; tags-only churn ≤5%; upserts/patch
≤2.0; `reorder_note` + `delete_note` >0 in a live session; ≥15% of ticks emit
`operations: []` **and those patches persist**; p50 notes generation ≤3.1s /
p95 ≤3.8s; variable-region block ≤6000 chars; notes `Truncated` = 0; ticks/min and
`coalesced_updates` measured before/after (B §3.2: a faster lane runs *more*
ticks — expect it, don't "fix" it).
*Tripwires:* **do not add an `operations.is_empty()` skip anywhere** — suppressing
empty patches un-advances the coverage head and re-queues the basis forever
(verified trap); keep `ProjectionPromptShape` field names (`notes_snapshot_*` feed
the ADR-0025 counts-only ledger); one prompt-cache invalidation at rollout, then
byte-stable; ADR-0038 — no runtime max_tokens escalation, emitting less is the only
truncation lever; filter is fresh-ingest-only, never `apply_patch`.

### W3 — `basis_currency_at_apply` (Rust, S)
Additive, defaulted, `skip_serializing_if` field carrying
`ProjectionRuntimeApplyResult`'s existing `basis_currency_at_apply` (`state.rs:403`)
out on the emitted patch/event to the frontend. Same compat class as W1 row-3
(old builds ignore it). *Acceptance:* serde round-trip; old-reader tolerance test;
frontend type updated. *Tripwire:* lane-serialized after W2 (one Rust lane);
this is the **only** thing that may ever turn a recency chip green.

### W4 — bento grid + tile contract (FE, M)
A §4.1's grid (areas `transcript / graph+document / agent`, 1px `--border-color`
gap) + all seven phase-2 markup requirements + frozen `WorkspaceTileId` union with
contract test + responsive tiers per R1/R2. `canvas` row-swap via one
`data-graph-mode` attribute on the container. **No layout-prefs reader in phase 1**
(schema + read rules documented as types/comments only; reader+writer ship
together in phase 2).
*Acceptance:* ratified fixed layout at ≥1280; tiers behave per R1/R2; zero JS
viewport reads in tiles; every tile is `role="region"` with labelled header and own
scroll container; contract test freezes tile ids.
*Tripwires:* 0922 — `.workspace-panel--capture` (`layout.css:116-161`) is
**replaced, old properties deleted**, verified in the production bundle; grid-area
only via `[data-tile]`, never inline; leave an unclaimed named row for 586b's
future notice banner; `useShellLayout` keeps owning rail/aside only.

### W5 — living document (FE, L)
`liveDocumentModel.ts` (pure, unit-tested): `notesToOutline` mapping
note→section via `heading_level` (clamped `prev+1`, cap 4→depth 2 bullets), body
parsed with the **W1 grammar** (shared spec, not ad-hoc splitting); legacy mode for
all-`None` sessions; `contentHash` (djb2, **tags excluded**) with **incremental
fold** — previous VM + newest patch, no full `sessionProjectionEvents` replay;
`outlineToMarkdown` copy path. `LiveDocument` components: app-owned h2, no
fabricated headings for unsectioned nodes, sticky-forever section expansion,
gutter `·N` + Popover provenance (revisions, seq, tags, evidence-when-present),
`DocRefusalNotice` (phase-1 proxy: accepted patch whose ops all no-op),
empty/skeleton/loaded-session states per A §1.7. `NotesPanel` untouched
(Sessions lens keeps it).
*Acceptance:* byte-identical and tags-only upserts produce no pulse, no revision
bump, no announcement; store artifact byte-faithful
(`ProjectionReplayArtifactReport` parity test untouched and green); an old
recorded session renders in legacy mode with zero errors; copy produces valid
markdown; VM unit tests cover both adapt paths.
*Tripwires:* no markdown dependency, no HTML path anywhere (XSS — the grammar
renders to React text nodes only); dedupe lives in the VM, **never** in
`applyProjectionNotesPatch` or the store artifact; `resetSessionView` clears
`docChangeCursor`/pin but user prefs survive; the revision badge will read ~93
where the old badge read 379 — landing note, expected, not a regression.

### W6 — recency chips + tone module (FE, M)
`liveWorkspaceTone.ts` wrappers over `readinessChipTone` (the "ready" sentinel
mapping lives only here); `selectLaneRecency(kind)` computing `turnsBehind` from
`asrSpanRevisions` (`is_final && end_of_turn`, `received_at_ms` after last
same-kind patch); chips per §2's resolved policy: neutral "as of HH:MM:SS",
warning "−N turns" at ≥3, success "Up to date" **only** when W3's
`basis_currency_at_apply === Current` is present; loaded session → `render:false`.
The 11 chip i18n keys + the **interpolating** sibling budget test
(`workspace-chip-length-budget.test.ts`, interpolate `count:99` etc. before
asserting ≤18).
*Acceptance:* with no W3 data, no success tone and no "Up to date"/"Live" string
can render (unit test); pt lengths pass post-interpolation.
*Tripwires:* the law gates copy, not just color — do not render "ready"-class
copy at neutral tone; one shared computation for notes+graph chips.

### W7 — KG strip (FE, M)
Shared `useActiveGraphSnapshot` exposing `{materialized, snapshot}` (single
fallback rule, consumed by all three modes). `graphFocus.ts`: focus = nodes
touched by last 3 distinct `updated_by_sequence` (active only), cap 12, 3-tick
hysteresis, both-endpoints edge rule; legacy-only sessions rank by `last_seen`.
`GraphFocusCanvas` (no zoom/pan, `cooldownTicks=40`, re-heat only on id-set
change, >8/12 swap suppresses re-heat); `canvas` = existing `KnowledgeGraphViewer`
verbatim; `feed` = textual change list (render-cap 100; **never** cap the store
array) and the Suspense/chunk-failure fallback. Total-size counter from the full
graph. Mode persisted `ag.graphStripMode`; pin is graph-local (no fake
entity→note jump — no such edge exists in the contract).
*Acceptance:* strip renders live within one graph patch of mount; no re-heat on
weight-only changes; feed mode mounts no canvas; counter = full-graph totals.
*Tripwires:* never rank by `mention_count` (hardcoded 1 —
`materializedGraph.ts:163`); uniform edge strokes, relation labels hover-only
through `escapeHtml` (123-type vocabulary, 9366 open); node color only from the
closed `entity_type` enum; "recently updated", never "currently discussed".

### W8 — agent tile (FE, M)
Extract `mergeLiveAssistCards`/`liveAssistCardFromProposal` into store selectors;
queue = actionable ∩ `admitToQueue` (shipped `() => true`, doc comment names
104f — the entire 104f integration is that one function body); feed = the rest,
newest first, heading "Recent activity". Queue max 3 + in-place disclosure;
approve-fail keeps the row with `role="alert"` + retry; approved rows animate to
feed (reduced-motion instant); outcome details in overflow popover. Delete
`statusClass()` (`AgentProposalsPanel.tsx:37-48`) → `ag-chip[data-tone]` via
`agentOutcomeChipTone` (approved with null outcome → neutral). Add the 5 missing
`agent.*` keys to en+pt (**split into its own S ticket if W8 slips — it's a
shipped pt bug independent of this epic**). Tile always mounted per R3; keep
`hasAgentActivity` for the get-started exclusion (`App.tsx:1042-1052`).
One debounced sr-only queue announcement, never per-row alerts.
*Acceptance:* approve path unchanged (`approve_agent_proposal`, ADR-0013/4b52);
nothing in the feed is unreachable; no proposal ever optimistically removed on a
failed approve; locale files carry every rendered key.
*Tripwires:* 0922 — delete `statusClass`'s properties, don't shadow; **no global
badge** (SHELL-R3 reach reduction stands — in-tile count only); loaded session →
feed-only.

### W9 — queue quality classifier (FE, S, gated R6)
A's `classifyQueueEntry` (confidence <0.5, normalized-title dupe collapse with
`×N`, locale-safe sentence-shape test — **no English word lists**) as the
`admitToQueue` body, behind the Signal/All toggle (persisted
`ag.agentQueueFilter`). Low-signal rows go to the feed, reachable via overflow.
*Tripwire:* when 104f lands a backend quality field, it replaces this function's
body — one line, no tile change.

### W10 — L1/L2 polish (FE, S)
`DocChangeAnchor` (IntersectionObserver over changed nodes, self-dismissing,
`scrollBehavior()` reduced-motion aware); sticky-follow **only** for
tail-appends within 100px of bottom (`LiveTranscript.tsx` idiom); `.ag-doc-refined`
1.5s pulse + `prefers-reduced-motion` block; rate limits (1/node/1.5s; >6
changed nodes → header count only); debounced 2s sr-only "N passages refined".
*Tripwire:* a mid-document rewrite must never move the viewport — that is the law
this ticket exists to enforce; test it.

---

## 4. Ratification gates (before the gated ticket dispatches)

| # | Decision | Recommendation | Blocks |
|---|---|---|---|
| **R1** | Delete the 1120px capture reflow tier; align the bento to `useShellLayout`'s 1280/1024 (one layout authority). Changes behavior for 1121–1279px windows. | **Yes.** Two disagreeing authorities (verified) are worse than either boundary. | W4 |
| **R2** | Compact (<1024) tile order: graph/document/agent/**transcript last** — the document takes the fold. Alternative: transcript-first continuity. | **Yes** — ratified decision 1 says the document is the primary surface; today's ≤1120 order already puts notes first, so the delta is smaller than it looks. | W4 |
| **R3** | Agent tile always mounted (empty-state instead of the `hasAgentActivity` appearance gate); the gate variable survives for the get-started exclusion. | **Yes.** A mid-conversation tile pop-in is a reflow under the reader's hands, and phase-2 show/hide needs an existing target. Not a badge re-litigation. | W8 |
| **R4** | The bento document tile ships **without** NotesPanel's client-derived ontology sections and **without** inline `synthesize_notes` output (kept as an action/export). ADR-0014 gets updated to record the living-document supersession. | **Yes.** The KG strip sits directly above the document; the ontology block duplicates it. One question, one answer per surface. Sessions lens unchanged. | W5 |
| **R5** | Append B's one `EVIDENCE_GUIDANCE` sentence naming `grounded_inference` as the synthesis default. Touches the ADR-0037-shared constant (also used by the repair prompt); shifts the measured 93/93 `verified_quote` distribution. | **Yes** — the 93/93 rate is an ordering artifact; the sentence re-ranks without weakening any class or touching the judge. Watch judge pass-rate in W2's metrics. | W2 |
| **R6** | Ship phase-1 queue-quality heuristics (W9) vs. seam-only raw stream until 104f. | **Yes, ship W9** — field data says the queue fills with fragments on day one, and the Signal/All toggle is the escape hatch that makes a heuristic acceptable. | W9 |
| **R7** | Close seed 7462 as "fixed by the living-document delta contract," metric re-homed to W2 acceptance (not closed-as-duplicate-and-forgotten). | **Yes.** Do-not-double-fix constraint satisfied by making W2 own the number. | — (queue hygiene, with W2's landing) |

---

## 5. Deliberately not in this cut

- **Phase 2 bento:** show/hide/resize/arrange, the `WorkspaceLayoutPrefs`
  reader+writer, header-slot controls. Markup contract is prepaid in W4; nothing
  else ships.
- **Auto-apply with undo** (non-goal; needs its own ratification).
- **Per-section multi-span provenance** — `EvidenceAnchor` is single-span; widening
  it changes what the ADR-0037 judge proves. Needs its own ADR (B §2.2's flag,
  endorsed).
- **Closed relation ontology / edge legend or filters** (9366 owns it; strip renders
  vocabulary-agnostically until then).
- **Entity→document jump** — no entity→note edge exists in the contract; faking it
  with substring matching is a lie. `SeekTimeline`'s `related_edge_ids` is the
  precedent when someone designs it.
- **Hand-editing the document** (phase 1 read-only, model-owned per non-goals;
  B's note that a user edit is an ADR-0037 provenance-class question stands).
- **Third projection lane, any scheduler restructure, spacing floors** — measure
  tick-rate after W2 first (ADR-0029 posture).
- **`InvalidateNote` Rust/TS divergence** (Rust hard-deletes, TS ignores) —
  pre-existing, needs its own seed; named so it isn't blamed on this epic.
- **64e3/fa56 inconsistency surfacing** — tiles tolerate transcript gaps silently
  (no dead jump affordances); a banner would punish every session for another
  lane's bug.
- **586b degradation banner** — W4 reserves the grid row, nothing more.
- **Outline collapse past ~50 sections** — 6000-char budget fits ~40–45 headings;
  measure real section counts post-W2 before designing compaction.

---

## 6. Verification appendix (what the judge checked)

Confirmed in code: `deny_unknown_fields` only at `projection_llm.rs:185`;
strict reader path (`canonical_reader.rs:163-168`) + pin (`commands.rs:12537`);
`derive_coverage_heads` ops-blind; empty-ops apply commits `last_sequence`; no
`operations.is_empty()` early return; strict schema `operations` has no
`minItems`; `ttft_estimate_ms`/`coalesce_span_threshold` consumed only by the
`coalescing_reason` label; `render_notes_snapshot` recency-sort + self-flagging
doc comment; `load_materialized_notes` = bare `load_json` (no version gate);
ingest normalizer seam + e700 `node_id_redirects`; `readinessAxisTone` demotes
unverifiable "ready" **including copy**; `mention_count: 1` hardcode
(`materializedGraph.ts:163`); snapshot `GraphNode` lacks `updated_by_sequence`;
5 `agent.*` keys absent from both locale files; 1280/1024 vs 1120 mismatch;
chip budget test measures raw literals; `statusClass()` bypasses the chip recipe;
store upsert unconditional; no markdown dep; `asrSpanRevisions` fields;
`hasAgentActivity` feeds the get-started exclusion (`App.tsx:1042-1052`).

Found wrong and corrected in this synthesis: **B §1.5(b)** — the admission seam
does *not* already have materialized notes (`ProjectionPatchBuildContext`,
`llm/executor.rs:941`); W2 carries the plumbing. **A §8/S1** — phase-1 neutral
"Up to date" copy violates the tone law's copy-gating; resolved via the observed
timestamp + turnsBehind + W3 upgrade ladder. **B §4.2** — the shared
snapshot-only selector cannot compute B's own sequence-based recency; selector
exposes the materialized graph too.
