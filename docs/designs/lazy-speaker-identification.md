# Lazy speaker identification

Status: Phase 1 backend implemented under [ADR-0050](../adr/0050-lazy-speaker-identity-relabels.md). The People panel and later evidence/voiceprint phases are separate work.

The active-session command surface is `list_session_speaker_clusters`, `relabel_speaker_cluster`, `mark_cluster_is_self`, `merge_speaker_clusters`, and `unbind_speaker_cluster`. The list returns a session-scoped cluster snapshot; mutations commit a single speaker-log batch before publishing revisions and return the number of graph edges reattributed. A channel prior is only a suggestion and is never written as a binding.
Parent epics: `audio-graph-3588` (speaker timeline) and `audio-graph-5011` (local worker).
Related: `audio-graph-9509` (identity ladder), `9c45`, `262a`, `dcd3`, `1f56`, `1fbd`, `eebf`,
`1586`, `ca06`, `8848`, `4796`, `53cf`, `c237`, `dd19`.
Research and citations: [`docs/research/speaker-identification-2026-11-21.md`](../research/speaker-identification-2026-11-21.md).

## 0. Summary

> Anonymous now. Named when the evidence is good enough. Revisable at any time.

- **Diarization is unchanged.** It keeps emitting stable anonymous clusters immediately
  (`speaker-c-3`, "Speaker 4", `deepgram-1`, …).
- **A new backend module, `src-tauri/src/identity/`,** keeps a per-session map from each anonymous
  **cluster** to a **binding**: `Anonymous`, then `Suggested`, then `Bound`, or `UserConfirmed`.
  Evidence feeds it from these sources:
  - the user
  - the capture source (mic means "me")
  - a closed roster of candidates (calendar, manual, and later platform UI)
  - self-introductions and addressed names in the transcript, judged by System-1 `choice`/`noul`
    questions on the existing decision contract
  - opt-in local voiceprints
- **A binding change is applied only as a relabel batch.** For each span of the cluster, it writes
  a new `DiarizationSpanRevision` with the **same `span_id` and `speaker_id`**, a **new
  `speaker_label`**, and an additive `identity` stamp. The batch flows through the existing spine:
  `SpeakerTimeline::apply_event` → `SpeakerLabelRemap` → graph reattribution, plus the planned
  notes invalidation/re-projection consumer (`262a`) and the timeline re-fold (`dcd3`).
- **Nothing else moves:**
  - No new canonical stream (ADR-0043).
  - No rewrite of `TranscriptEvent`s, so ADR-0042 transcript hashes are untouched.
  - No change to projection basis semantics beyond citing the diarization spans a prompt
    actually used.
  - Voiceprints live in a separate opt-in store: encrypted, on the device, outside session
    artifacts, and never sent off it.

This copies the Zoom pattern described in the research: generic tags first, matching against a
closed set of invitees or users paired to the room, host edits after the meeting that "sync
across ... captions, live transcripts, AI Companion, and cloud recording transcripts", and "most recent change is applied".
Here it is rebuilt as event-sourced revisions.

| Zoom behavior (research §A) | AudioGraph equivalent in this design |
|---|---|
| Generic "Speaker 1" tags in the live transcript | Unchanged anonymous cluster labels |
| Match enrolled voices only against invitees or paired users | Closed candidate set from the roster plus the local user (§4.3) |
| Room audio processed on the device and deleted at meeting end | Session centroids held in memory only and dropped at session end (§3.5) |
| Host edits a tag after the meeting; edits sync everywhere; latest wins | `UserConfirmed` relabel batch with fan-out to graph, notes, and timeline; the user's edit always wins (§5) |
| Users can delete their reference voice | `delete_known_person` and `purge_voiceprints` with crypto-shred (§3.4, §10) |

## 1. Current state (code-grounded, HEAD of this branch)

- **Anonymous clusters already exist.**
  - `diarization/stabilize.rs:46-167` `SpeakerRegistry` maps the per-window local clusters onto
    stable global ids. It uses L2-normalized centroids and greedy cannot-link assignment, with
    `DEFAULT_SIM_THRESHOLD = 0.6` (`:63`).
  - `diarization/worker.rs:450-525` emits `StableSegment { global_speaker, window_start_sample }`.
  - `speech/mod.rs:1976-2079` (`run_clustering_emit_loop`) turns each segment into a
    `DiarizationSpanRevisionPayload` with these properties:
    - `provider: "local_clustering"`, `timeline_id: "session"`, `source_id: None`
    - `revision_number: 1`, `stability: Provisional`
    - **empty `basis_asr_span_ids` / `basis_transcript_segment_ids`** (Seed `ca06`)
    - `span_id = diarization_span_revision_id(provider, timeline, start, end, speaker_id)`
      (`:368-383`), so **the speaker id is part of the span identity**
- **The retcon spine exists; persistence is best-effort.**
  - `SpeakerTimeline::apply_event` (`projections.rs:792-835`) replaces a span's revision by
    `span_id`. It rejects stale or conflicting revisions. When a non-empty label changes, it
    returns a `SpeakerLabelRemap` (`:837-850`).
  - `dispatch_diarization_span_revision` (`speech/mod.rs:459-496`) passes every remap to
    `graph.supersede_entity(old, new, ts, 1.0)`.
  - `emit_and_dispatch_diarization_span_revision` (`:498-593`) attempts to persist accepted revisions
    (`:570-580`, Seed `719d`, closed) and emits `GRAPH_DELTA` / `GRAPH_UPDATE`. Append failures
    are logged and swallowed (`:595-615`); the in-memory revision is then lost on reload.
  - Lock order is ledger (scoped and dropped), then timeline, then graph (`:199-230`).
- **Readers resolve speakers through the timeline, with gaps.**
  - `timeline.rs:119-186` `build_session_timeline` joins through `speaker_attribution_index`
    (`:216-240`), which keys on the spans' **basis ids**. With `ca06`'s empty anchors, local
    clustering spans never join.
  - `projection_llm.rs:1758` `pinned_typed_facts` reads the **inline** `TranscriptEvent` label, so
    the LLM never sees a corrected label (Seed `1586`).
- **The projection basis cites no diarization spans today.**
  `TranscriptLedger::current_projection_basis` (`projections.rs:1112-1120`) calls
  `ProjectionBasis::from_transcript_events`, which passes `&[]` speaker spans (`:396-398`). The
  diarization basis is opt-in (`validate_diarization_basis`, `:879-932`). Historical replay
  validates each patch against speaker history bounded by its `created_at_ms`
  (`replay_accepted_patches_with_history`, `:3681`).
- **Notes have no speaker consumer.** `ProjectionOperation` (`projections.rs:1825-1957`) has no
  speaker-correction consumer yet (`262a`, which depends on `4796`).
- **The manual merge bypasses the timeline.** `commands.rs:4510-4560` `merge_graph_entities_impl`
  calls `supersede_entity` directly. A speaker rename done that way is never written to the
  speaker log, so it is lost on reload and invisible to the transcript and timeline.
- **Graph keys are names.** `TemporalKnowledgeGraph.name_index` is a lowercased name index
  (`graph/temporal.rs:59-60`). `GraphEntity.speakers: Vec<String>` stores the labels of speakers
  who mentioned an entity (`graph/entities.rs:10-29`). `supersede_entity` folds the superseded
  node's state into the canonical node and re-points every name-index key (`temporal.rs:547-591`).
  It returns 0 and **changes nothing if the canonical name does not exist yet** (`:424-427`).
- **Release builds compile no neural diarizer** (Seed `8848`), so shipped labels come from the
  Simple heuristic.

[INFERENCE, from reading the code] A real defect follows from these facts. AssemblyAI v3 span ids
are per turn and do not include the speaker (`asr/assemblyai.rs:430-432`). So a speaker revision
of **one** turn produces a remap. The dispatcher then merges the **whole** old speaker entity into
the new one. See §5.4 and the bug Seed.

## 2. Goals and non-goals

Goals:

- **G1.** Anonymous clusters stay as immediate as today. Identity adds no latency to the live path.
- **G2.** A name is bound only when the evidence passes a threshold. Below the threshold, the
  binding is at most a UI-only *suggestion*.
- **G3.** Every binding can be revised: bind, rename, unbind, merge, and split. A revision retcons
  the transcript view, notes, the live graph, and the timeline without duplicate rows.
- **G4.** The user's edit always wins and is never auto-reverted.
- **G5.** Biometric data is local-only, explicitly opted in, model-pinned, and purgeable.
- **G6.** Windows, macOS, and Linux behave the same, and per-OS extras sit behind capability
  probes.
- **G7.** Projection basis hashes and accepted-patch replay are not broken (§5.5).

Non-goals:

- Face recognition
- Cloud voice-ID APIs (Speechmatics identifiers, pyannoteAI voiceprints)
- Separated PCM lanes as ASR input
- Org-wide shared voiceprints
- The realtime speech-to-speech agent mode, which keeps the product split

## 3. Data model

### 3.1 Identifiers

```rust
/// Anonymous, session-scoped cluster identity. Never a person.
/// Text form: "{provider}:{timeline_id}:{speaker_id}", e.g. "local_clustering:mic-1:speaker-c-3".
pub struct ClusterRef { pub provider: String, pub timeline_id: String, pub speaker_id: String }

/// Opaque local person id ("person:<uuid-v4>"). Minted when the user names someone or a roster
/// entry is accepted. Carries no biometric content.
pub struct PersonRef(pub String);
```

Rules:

- `speaker_id` on a span stays the **anonymous cluster id from its first emission**. It is never
  overwritten, because it is part of `span_id` (`speech/mod.rs:368-383`).
- **The canonical cluster after a merge or split** lives in the identity stamp (`cluster_ref`),
  not in `speaker_id`.
- **The display identity** is `speaker_label`. While anonymous, it is the diarizer's label
  ("Speaker 4"). Once bound, it is the person's display name.
- **Labels are unique within a session.** The graph is keyed by lowercased name, so if two bound
  persons share a display name, the later one gets a suffix ("Alex (2)"). The text forms `Speaker N`
  and `speaker N` are reserved: a cue such as "I am speaker 0" (from the `9509` field inventory) is
  never accepted as a name.

### 3.2 Identity stamp: an additive field on the speaker revision

Add the field to `projections::DiarizationSpanRevision` (`projections.rs:614-659`), to
`events::DiarizationSpanRevisionPayload` (`events.rs:320-358`), and to the TypeScript payload type.
Use `#[serde(default, skip_serializing_if = "Option::is_none")]`:

```rust
pub identity: Option<SpeakerIdentityStamp>,

pub struct SpeakerIdentityStamp {
    pub schema_version: u32,                 // SPEAKER_IDENTITY_STAMP_VERSION = 1
    pub cluster_ref: String,                 // canonical ClusterRef text after merges
    pub person_ref: Option<String>,          // None => anonymous
    pub state: SpeakerBindingState,          // Anonymous | Bound | UserConfirmed
    pub label_source: SpeakerLabelSource,    // Diarizer | Provider | IdentityBinding | User (1fbd)
    pub binding_id: String,                  // sha256("{session}:{cluster_ref}") prefix
    pub binding_revision: u64,               // monotonic per binding_id
    pub confidence: Option<f32>,             // fused app-origin posterior; None for User
    pub evidence: Vec<IdentityEvidenceRef>,  // content-free (kind, weight, decision ids, span ids)
    pub batch: RelabelBatchRef,              // { batch_id, kind, scope, index, len }
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decision_provenance: Vec<crate::decision::route::DecisionRecord>, // only on batch index 0
}
pub struct IdentityEvidenceRef {
    pub kind: IdentityEvidenceKind,
    pub weight: f32,
    pub decision_record_id: Option<String>,
    pub basis_span_ids: Vec<String>,
}
```

- The stamp is **content-free**. Names stay in `speaker_label`, whose `Debug` output is already
  redacted (`projections.rs:661-696`), and `DecisionRecord` holds only state digests (ADR-0049 §3).
  `Debug` on the stamp must redact `person_ref` as well.
- `Suggested` is **never persisted**. It reaches the UI as an ephemeral `SPEAKER_IDENTITY_SUGGESTION`
  event, which avoids the label churn `1586` observed.
- Compatibility: ADR-0043 v1 payloads "may ignore unknown payload object members", and
  `DiarizationSpanRevision` derives plain `serde::Deserialize`. An optional member is therefore
  read-compatible. Whether it needs a `SPEAKER_REVISIONS_SCHEMA_VERSION` bump is an ADR decision
  (§12).

### 3.3 Session identity state (derived, never its own stream)

```rust
// src-tauri/src/identity/mod.rs
pub struct SessionIdentityLedger {
    session_id: String,
    clusters: BTreeMap<ClusterRef, ClusterIdentity>,
    persons: BTreeMap<PersonRef, SessionPerson>,   // display_name, roster sources, known_person_id
}
pub struct ClusterIdentity {
    pub canonical: ClusterRef,                   // after merges
    pub binding: Binding,                        // { state, person, binding_revision, label }
    pub suggestion: Option<Suggestion>,          // UI-only
    pub evidence: EvidenceAccumulator,           // in-memory; rebuilt lazily after reload
}
```

- The durable truth is the latest identity stamp per `binding_id` in the speaker timeline.
  `SessionIdentityLedger::from_timeline(&SpeakerTimeline)` rebuilds the ledger on reload
  (`persistence::replay_speaker_timeline`, `persistence/mod.rs:771`).
- `IdentityResolver` is a cheap read view: `ClusterRef → (label, stamp)`. Emitters consult it
  (§5.3).

### 3.4 Cross-session voiceprint store (opt-in; Phase 3)

The store is a user-level file at `app_data_dir()/identity/voiceprints.agvp`, resolved through
the Tauri path API. It is **not** a session artifact, so it never appears in the
`session_artifact_manifest` kinds, exports, or org promotion.

```rust
// src-tauri/src/identity/voiceprint_store.rs
pub struct VoiceprintStoreV1 { pub persons: Vec<KnownPerson> }
pub struct KnownPerson {
    pub person_id: String,                 // "person:<uuid>"; equals the PersonRef when linked
    pub display_name: String,
    pub is_self: bool,
    pub voiceprints: Vec<Voiceprint>,
    pub consent: ConsentRecord,
    pub created_at_ms: u64,
    pub last_matched_at_ms: Option<u64>,
}
pub struct Voiceprint {
    pub model_pin: EmbeddingModelPin,      // { model_id, sha256, dim }
    pub centroid: Vec<f32>,                // L2-normalized
    pub exemplars: Vec<Vec<f32>>,          // <= 8, for robustness across conditions
    pub clean_speech_secs: f32,
    pub source: VoiceprintSource,          // ExplicitEnrollment | ConfirmedSession { session_id }
    pub created_at_ms: u64,
    pub last_matched_at_ms: Option<u64>,    // this voiceprint only; never resets created_at_ms
}
pub struct ConsentRecord {
    pub kind: ConsentKind,                 // SelfEnrollment | ThirdPartyAttestation
    pub consent_text_version: u32,
    pub granted_at_ms: u64,
    pub expire_after_unused_days: u32,     // default 365; bounded to <= 1095
}
```

- **Encryption.** The file is an envelope `{ format: "agvp", version: 1, nonce, ciphertext }`
  sealed with XChaCha20-Poly1305. The 256-bit key is random and kept in the OS keychain through
  the existing `keyring` backend (`credentials/mod.rs:849-853`), account
  `identity:voiceprint-key`. Plaintext is held in `Zeroizing` buffers (`zeroize` is already a
  dependency). This adds one dependency, `chacha20poly1305` (RustCrypto, MIT/Apache-2.0).
- **No plaintext fallback.** If the keychain is unavailable (for example, Linux without Secret
  Service), voiceprints are *unavailable*, with a readiness reason. This deliberately differs from
  the legacy `credentials.yaml` fallback.
- **No audio is retained.** Enrollment audio is embedded and then dropped.
- **Model pinning.** A voiceprint whose `model_pin` differs from the active embedding model
  (today `nemo_en_titanet_small.onnx`, `models/mod.rs:269-283`) is `Stale`. It is never compared,
  and the UI asks for re-enrollment, following the Speechmatics "identifiers are tied to the
  model" caveat.
- **Deletion and purge:**
  - `delete_known_person` removes the record, rewrites the file, and calls
    `SpeakerEmbeddingManager::remove`.
  - `purge_voiceprints` deletes the file **and** the keychain key (crypto-shred) and clears the
    in-memory managers.
  - A retention sweeper runs at startup and before matching, and schedules deletion at the
    earliest expiry while the app remains open. For each voiceprint, delete at the earlier of
    `last_matched_at_ms.unwrap_or(created_at_ms) + expire_after_unused_days * DAY_MS` and
    `created_at_ms + 1095 * DAY_MS` (using checked time arithmetic). The second deadline is an
    **absolute 1,095-day cap from enrollment/creation**, not an inactivity limit. Matching may
    advance that voiceprint's `last_matched_at_ms`, never its creation time or absolute deadline;
    the person-level last-match summary does not extend any voiceprint's lifetime. Fresh explicit
    enrollment creates a new voiceprint with new consent; automatic updates cannot reset the cap.
    Delete earlier when consent is revoked or the purpose is satisfied. This absolute cap is a
    product retention rule, not a claim that BIPA's statutory clock starts at enrollment.
  - Deleting a person **does not** rewrite past sessions by default: names in past labels fall
    under session retention (`c282`, `be7c`). Phase 5 adds "also anonymize this person in past
    sessions", which emits per-session `Unbind` batches.

### 3.5 What is never persisted

- Per-session cluster centroids (`SpeakerRegistry.speakers[*].centroid`)
- Embeddings of non-enrolled people
- Raw enrollment audio
- Suggestions
- Evidence accumulators
- Accessibility roster snapshots: only the names a user accepts become labels

## 4. Evidence sources and fusion

### 4.1 Evidence kinds

| `IdentityEvidenceKind` | Producer | Signal | Initial weight (log-odds) | Can bind alone? |
|---|---|---|---|---|
| `UserManual` | `commands::relabel_speaker_cluster` | Assign a name, "this is me", or "not X" (veto) | Absolute | **Yes** → `UserConfirmed` |
| `EnrolledVoiceprint` | `identity::voiceprint` | Cosine of the cluster centroid against a `KnownPerson` with the same model pin; needs ≥ 10 s of overlap-free speech | +6 at ≥ τ_bind; +3 at ≥ τ_suggest | Yes, if the person is self or in the candidate set |
| `SourceChannel` | `identity::channel` | The cluster carries ≥ 80% of the speech on a `Physical` mic source (`audio/capture.rs:384-395`), and self has a display name | +5 toward self; −2 toward self for `AppProcessDerived`/`Mixed` clusters | Yes, **self only** |
| `PlatformActiveSpeaker` | `identity::roster` (Phase 5) | The roster's active-speaker intervals co-occur with cluster activity | +6 when overlap ≥ 0.8 over ≥ 20 s | Yes |
| `RosterStructure` | `identity::roster` | Exactly one non-self remote cluster and exactly one non-self attendee | +4 | Yes, two-party case only |
| `SelfIntroduction` | `identity::cues` + S1 `speaker_self_identification` | "I'm Priya" in a turn of cluster k | +4 decided / +2 proposed / +3 heuristic | Yes if the name is in the candidates and seen twice; otherwise Suggest |
| `AddressedName` | cues + S1 `speaker_addressee_replies` | "Thanks, Priya" followed by a reply from a different cluster | +2 | No |
| `ThirdPartyIntroduction` | cues + S1 `speaker_introduces_present_participant` | "Joining us is Ryan", then the first new cluster | +2 | No |
| `CalendarPresence` | roster | The candidate exists | Prior only | No |
| `CoActivityVeto` | binder | Two clusters overlapping ≥ 1 s on the same source cannot be one person | −∞ for the pair | — |

The weights are **uncalibrated design values**. Phase 2 fixtures tune them, and the
long-term plan is a fitted logistic combiner on labeled data (the `f284` approach).

### 4.2 Channel and source routing

- **Requirement R-src:** local diarization must emit **source-local timelines**, with
  `timeline_id = source_id` and `source_id: Some(..)`. Today it uses `"session"`
  (`speech/mod.rs:2043-2044`). This belongs to the `5011` and `eebf` scope, and is proposed as a
  Seed.
- Mapping from source type:
  - `Device{Input}` (`Physical`) gives the self prior.
  - `Application{..}` / `ProcessTree` (`AppProcessDerived`) are remote participants.
  - `SystemDefault` (`Mixed`) is remote, with lower confidence.
  - An unknown device kind (`Mixed`) gives no prior.
- Failure modes the binder must guard against:
  - **In-room meeting:** the mic carries more than one cluster. The self prior then applies only
    to the dominant cluster, and only when its share is ≥ 80%.
  - **Speaker bleed:** remote audio leaks into the mic. A mic cluster whose activity correlates
    with system-source activity (≥ 60% co-activity) gets no self prior. That is an echo or AEC
    failure (see `spike-0bdc`).
  - **Shared device:** several people on one remote stream. This is the Zoom "Unknown Speaker"
    case: roster evidence cannot bind an individual on a stream that has more than one cluster.

### 4.3 Roster: the closed candidate set

```rust
pub struct ParticipantRoster {
    pub entries: Vec<RosterEntry>,
    pub sources: BTreeSet<RosterSource>,
}
pub struct RosterEntry {
    pub person_ref: PersonRef,
    pub display_name: String,
    pub source: RosterSource,
    pub active_intervals: Vec<(f64, f64)>,
}
pub enum RosterSource {
    SelfProfile,
    Manual,
    Calendar,
    AccessibilityPlatformUi,
}
```

- `SelfProfile` comes from the new non-secret setting `IdentitySettings.self_display_name`.
- `Manual`: the user types attendee names before or during the session.
- `Calendar`: attendees from the `53cf` connector, when present.
- `AccessibilityPlatformUi` (Phase 5): read-only participant names and active-speaker intervals
  from the meeting app's UI, the Granola pattern. It is a per-OS capability probe (macOS AX,
  Windows UI Automation, Linux AT-SPI). It is off by default and never the default on any single
  OS.
- **Closed-set rule (Zoom and Teams):** automatic `Bound` requires `person ∈ roster ∪ {self} ∪
  KnownPerson-with-consent`. A name heard only in the transcript can reach `Suggested` at most,
  until the user accepts it, which adds it to `Manual`.
- **Size:** at most 18 candidates go to S1, the 20-option Laya limit minus `none_of_these` and
  `unsure` (`decision/route.rs:73-78`). Candidates are ranked by source and recency.

### 4.4 Transcript cues and System-1 questions (decision-contract shapes)

`identity::cues::NameCueExtractor` is pure and deterministic. It runs over final `TurnUnit`s, the
`1f56` seam. Until that lands, it runs over final `TranscriptEvent`s grouped by resolved cluster.
It emits:

```rust
pub enum NameCueKind {
    SelfIntro,            // "my name is X", "I'm X", "this is X", "X here", "X speaking"
    Addressed,            // "thanks, X", "X, can you…", "over to you X", "what do you think, X"
    ThirdPartyIntro,      // "joining us is X", "welcome X", "with X, who is …"
}
pub struct NameCue {
    pub kind: NameCueKind,
    pub name: String,
    pub turn_span_ids: Vec<String>,
    pub speaker: ClusterRef,
    pub next_speaker: Option<ClusterRef>,
}
```

Names are normalized and fuzzy-matched to roster candidates with the same Jaro-Winkler measure
`resolve_entity` uses (`graph/temporal.rs:342-362`), threshold 0.92. Reserved anonymous labels are
dropped.

Each cue becomes **one S1 request** through `decision::route::dispatch` (`route.rs:652`), in the
`projection` family. Following ADR-0049 §3, **option keys are fixed schema identifiers. Names and
transcript text appear only in `state`.** Speakers are anonymized as `S` (the target) and `O1`,
`O2`, … Cluster labels never enter the state.

Schema constants live in `src-tauri/src/identity/decisions.rs`:
`SPEAKER_IDENTITY_SCHEMA_VERSION = 1`, plus the question ids
`speaker_self_identification`, `speaker_addressee_replies`,
`speaker_introduces_present_participant`, and `speaker_identity_choice`.

**Q1, `speaker_self_identification` (noul).**

```json
{
  "state": {"task": "speaker_self_identification",
            "turn": {"speaker": "S", "text": "Hi all, Priya here from platform."},
            "name": "Priya Raman"},
  "questions": {"speaker_self_identification": {"type": "noul",
    "instructions": "Does speaker S say in this turn that they themselves are the named person?",
    "criteria": {"true": "S states their own name or identity as this person",
                 "false": "S names someone else, negates it, or quotes it"}}}
}
```

**Q2, `speaker_addressee_replies` (noul).**

```json
{
  "state": {"task": "speaker_addressee_replies",
            "turns": [{"speaker": "O1", "text": "Thanks Priya, can you cover the timeline?"},
                      {"speaker": "S",  "text": "Sure, we are on track for March."}],
            "name": "Priya Raman"},
  "questions": {"speaker_addressee_replies": {"type": "noul",
    "instructions": "Is the named person the one who speaks the reply labeled S?"}}
}
```

**Q3, `speaker_introduces_present_participant` (noul).** The state holds one turn and a name. The
question asks: "Does this turn introduce the named person as someone taking part in this
conversation, not merely mention them?" This handles the `9509` cue "With Ryan Greenblatt, who is
the chief scientist at Redwood Research".

**Q4, `speaker_identity_choice` (choice).** This is the cluster-level arbiter. It runs when two or
more candidates conflict, or when the evidence for a cluster crosses the Suggest threshold.

```json
{
  "state": {"task": "speaker_identity_choice", "target": "S",
            "turns": [{"speaker": "S", "text": "…"}, {"speaker": "O1", "text": "…"}],
            "candidates": [{"key": "c0", "name": "Priya Raman", "sources": ["calendar", "self_intro"]},
                           {"key": "c1", "name": "Marcus Lee",  "sources": ["calendar"]}]},
  "questions": {"speaker_identity_choice": {"type": "choice",
    "instructions": "Using only these turns, which candidate is speaker S? Pick none_of_these if S is clearly someone else, unsure if the turns do not say.",
    "criteria": {"c0": null, "c1": null, "none_of_these": null, "unsure": null}}}
}
```

Rules shared by all four questions:

- **State size.** At most 6 turns per state, so it fits Laya's `max_state_tokens: 1024`.
  Oversized states fail `EngineLimits::validate` (`decision-contract/src/lib.rs:143-170`) or the
  engine's own tokenizer check, become `unavailable`, and fall back to the heuristic. The builder
  bounds states by turn count only. Per the contract, token limits are checked with the engine's
  tokenizer, never estimated from bytes.
- **Heuristic floor.** ADR-0049 requires one, with mirrored fixtures:
  - Q1 uses the anchored regex set above, with fuzzy candidate match and negation guards ("I'm
    not Priya", "is this Priya?").
  - Q2 requires a vocative at the start or end of a turn and a reply from a different cluster
    within 3 s.
  - Q3 uses an introduction pattern and requires a new cluster within 60 s.
  - Q4 takes the argmax of the heuristic-fused score (§4.5).
- **Authority.** Per ADR-0049 §3, every route stays in **shadow** (the heuristic answers and a
  `DecisionRecord` is stamped) until a `CalibrationRecord` exists: at least 200 labeled items per
  question and at least 20 per option.
- **Outcome mapping:**
  - `decided`: full weight.
  - `proposed`: half weight, and can only produce a Suggest.
  - `abstained` or `unavailable`: the heuristic's weight.
- **Egress.** Every native route (Laya, D3) is `Local`. A hosted route would need the
  Content-Egress Checklist for `TranscriptText` and `SpeakerLabels`. That is not proposed here.
- **Provenance.** Each `DecisionRecord` id is referenced from `IdentityEvidenceRef`. The records
  themselves ride on the relabel batch's first revision (`decision_provenance`), following
  "decisions ride the artifact they decide about" (ADR-0049 §3).

### 4.5 Fusion, assignment, and thresholds

```text
for each canonical cluster k with >= 4 s of speech:
  for each candidate p in roster ∪ {self} ∪ consented KnownPersons ∪ {⊥ unknown}:
    s[k][p] = log_prior(p) + Σ_evidence w(kind, outcome) · strength
  P[k][·] = softmax(s[k][·])
assignment = stabilize::greedy_assign(P, suggest_floor)    // reuse, one-to-one
  with cannot-link: CoActivityVeto pairs; user "not X" vetoes
  with merge allowance: one person may own several clusters only if they never co-occur
Bind     if P >= 0.90 and P - P_second >= 0.30
            and (one bind-alone kind OR >= 2 independent kinds)
Suggest  if P >= 0.60 (UI chip "Priya?"; no relabel)
AutoUnbind only for Bound (never UserConfirmed) when P < 0.50 on 2 consecutive evaluations
Rate limit: <= 1 automatic relabel per cluster per 60 s
```

`identity::binder` is a dedicated `std::thread`, like the diarization worker. It consumes a
crossbeam channel of `IdentityInput` values:

- accepted diarization revisions, for activity
- final turns
- roster updates
- centroid snapshots from the worker (`SpeakerRegistry::snapshot`)
- S1 results, which arrive asynchronously through tokio
- user commands

The binder never holds its own lock while it acquires the ledger, timeline, or graph locks
(§5.2).

## 5. Revision semantics

### 5.1 Relabel batches

```rust
// src-tauri/src/identity/relabel.rs
pub enum RelabelKind { Bind, Rename, Unbind, Merge, Split }
pub enum RelabelScope { Entity, Segments }          // graph semantics, see 5.4
pub struct SpeakerRelabelBatch {
    pub batch_id: String,
    pub kind: RelabelKind,
    pub scope: RelabelScope,
    pub revisions: Vec<DiarizationSpanRevision>,     // one per affected span, revision n+1
}
pub fn build_relabel_batch(timeline: &SpeakerTimeline, target: &RelabelTarget, label: &str,
                           stamp: SpeakerIdentityStamp) -> SpeakerRelabelBatch;
```

For every span `s` in `timeline.latest_spans` that belongs to the target, the builder emits one
revision. A span belongs to the target when its `identity.cluster_ref` matches, or when it has no
stamp and its `provider/timeline_id/speaker_id` match the cluster. For a split, only the selected
subset is included. Each emitted revision is `s.clone()` with these fields changed:

- `revision_number = s.revision_number + 1`
- `supersedes = Some(revision_ref(span_id, n))`
- `speaker_label = Some(label)`
- `identity = Some(stamp)`
- `received_at_ms = now`
- `raw_event_ref = Some("identity_batch:{batch_id}")`

`span_id`, `speaker_id`, `stability`, timing, and basis ids are **unchanged**.

| Kind | Label written | Stamp `cluster_ref` | Scope | Trigger |
|---|---|---|---|---|
| `Bind` | Person display name | Unchanged | `Entity` | Fusion crosses the threshold, or the user names a speaker |
| `Rename` | New display name for the same person | Unchanged | `Entity` | The user edits a person's name |
| `Merge` | Current label of the surviving cluster k1 | k1 | `Entity` | The user says "same speaker", identity collision, or post-session re-clustering |
| `Split` | A new anonymous label or a person | New cluster k' | `Segments` | User "split from here" or post-session re-clustering (Phase 4) |
| `Unbind` | Anonymous diarizer label | Unchanged | `Segments` | The user says "not X", or AutoUnbind |

### 5.2 Dispatch

Add `speech/mod.rs::emit_and_dispatch_speaker_relabel_batch(ctx, batch)` and the pure
`dispatch_speaker_relabel_batch(timeline, graph, batch, ts) -> RelabelOutcome`:

1. **Lock order stays as today.** Lock the `transcript_ledger` alone for the session-relative
   timestamp and drop it. Then take `speaker_timeline`, then `knowledge_graph` (`:199-230`). The
   identity ledger lock is a leaf: it is never held across these locks.
2. **Pre-validate the whole batch** under the timeline lock. Each revision must be exactly
   `current + 1` for its span. If any span has moved, reject the batch and rebuild it. This
   prevents partial application and `ConflictingDiarizationRevision`.
3. **Persist one atomic replay unit before publishing.** While still holding the timeline lock,
   append the entire batch as one versioned `SpeakerRelabelBatch` JSONL record to the existing
   speaker log, through its serialized writer, and flush/sync it before applying in memory.
   This is a new record envelope, not 1,200 independent revision records; legacy single-revision
   rows remain readable. If append/flush fails, report failure and do not apply or emit the batch.
   A crash after durable append but before in-memory apply is recovered by replay.
4. Apply with the new `SpeakerTimeline::apply_batch`. It is the same as `apply_event` per revision
   but sorts once (`sort_latest_spans`), not once per revision. It returns the remaps,
   **deduplicated by `(superseded_label, canonical_label)`**.
5. Call `graph.reattribute_speaker(old, new, scope, ts)` **once per distinct pair** (§5.4).
6. Release the locks. Then:
   - emit `DIARIZATION_SPAN_REVISION` per revision, so the frontend `speakerTimeline.ts`
     materializes latest-wins
   - emit one `GRAPH_DELTA` and one `GRAPH_UPDATE`
   - enqueue the notes and materialized-graph consumers (§5.4)
   - emit `SPEAKER_DETECTED` with the resolved label

**Replay behavior.** Decode and validate the complete envelope, revision count, batch indices,
and all span revisions before applying any member. Apply a valid batch all-or-none; a batch
already applied in full is an idempotent no-op. Reject a conflicting or malformed batch as a
whole, with a recovery diagnostic, never apply its valid prefix. An unterminated/truncated final
JSONL record is ignored as an uncommitted tail; the writer removes that tail before another
append. A durable complete record replays even if the process died before live publication.
Compaction must not expose a partial batch. These persistence/replay changes are prerequisites
to `apply_batch`, not guarantees supplied by today's best-effort per-revision writer.

**Cost.** A one-hour local-clustering session has about 1,200 spans (one per 3 s hop). A bind
appends one record containing about 1,200 revisions to `speaker.jsonl`. This pays synchronous
I/O under the timeline lock to preserve validation/commit ordering. Do not split a durable
batch into independently replayable chunks, even above 2,000 spans; serialization/transport
may stream bytes, but publication and replay still require the entire record.

### 5.3 Forward labeling

After a bind, new spans of the cluster must be **born** with the bound label. Otherwise every
window would produce "Speaker 4" followed by a relabel. These call sites take an
`IdentityResolver` handle; any producer-side read is only provisional:

- `run_clustering_emit_loop` (`speech/mod.rs:2011-2023`)
- `ClusteringSpeakerStats::record` (`diarization/mod.rs:916-930`)
- the inline `TranscriptSegment` labeling via `overlap_speaker_for_segment`
  (`speech/mod.rs:1842`)
- `diarization_span_revision_for_transcript` (`:393-430`)

The dispatcher performs the authoritative binding read **under the `speaker_timeline` lock**,
immediately before applying the new span, and replaces its label and stamp with that binding.
Binding changes and resolver updates commit under the same timeline lock (§5.2), taking the
leaf identity lock only inside it; no producer holds the identity lock while entering dispatch.
Emit the finalized payload only after this read/apply, not the producer's stale payload. Thus a
producer that read Anonymous before a bind but dispatches after it gets the bound label and
stamp. A span dispatched first is included in the subsequent bind's batch.
Past `TranscriptEvent`s keep their ingestion-time inline label. **Readers must resolve through
the timeline**, which the `1586` fix ensures.

### 5.4 Fan-out

**Live graph:** new method `TemporalKnowledgeGraph::reattribute_speaker(from, to, scope, ts) -> usize`.

- `Entity` scope:
  - If `to` resolves, delegate to `supersede_entity(from, to, ts, 1.0)` (`temporal.rs:413`).
  - If `to` is absent, **rename in place**:
    - set `node.name = to` and push `from` onto `aliases`
    - point both lowercased keys at the node in `name_index`
    - keep the node id, so the frontend keeps the node
    - mark the node updated in the delta

    Today `supersede_entity` silently does nothing in this case (`:424-427`). This branch fixes
    it.
  - In both branches, update `from` to `to` in every entity's speaker-mention provenance
    (defined below), then derive its `GraphEntity.speakers` list from that provenance. Keep the
    provenance synchronized on Entity-scope binds too, so a later segment correction cannot
    resurrect stale anonymous labels.
- `Segments(ids)` scope:
  - Retain **all contributing segment provenance on live edges**, including folds. Today the
    fold branch (`graph/temporal.rs:512-528`) adds the weight but keeps only the survivor's
    `source_segment_id`, dropping the folded segment. Add persisted contribution records keyed
    by `(segment_id, cluster_ref)` with endpoint attribution, weight and timing; expose their
    deduplicated segment ids as `source_segment_ids`. Initialize them at extraction and union
    them on every fold, clone and replay. The single seek id may remain a display choice, never
    the correction lookup key.
  - Find every live edge with a contribution from `ids` and the affected cluster, even if its
    displayed `source_segment_id` belongs to another cluster. Invalidate the old aggregate for
    audit, partition its contributions, and re-create aggregates: only selected contributions
    get the corrected speaker endpoint; unrelated contributions keep their endpoint and weight.
    Fold again only while retaining the full contribution records. A list of ids alone cannot
    recover the weights needed for this split.
  - Re-point `name_index[to]` at the **retired node of that exact name** if one exists, otherwise
    call `add_entity(to)`. `supersede_entity` re-pointed the anonymous key at the person, and an
    unbind must undo that.
  - Update **every affected entity's `GraphEntity.speakers`**, not only speaker nodes or edges.
    Persist speaker-mention provenance per entity keyed by `(segment_id, cluster_ref)`, with its
    resolved label/binding revision. Extraction records it; merges union it without losing the
    cluster key. Change only the selected provenance entries, then derive the deduplicated
    `speakers` list from all entries: keep `from` if any unaffected cluster still supports it,
    and add `to` for the corrected mentions. Mark changed entities in the graph delta.
  - Legacy aggregates/lists lacking complete provenance must first be rebuilt from their source
    extraction records (or re-extracted from the affected transcript windows). Do not infer all
    contributions from the surviving seek id or rewrite a whole label list as a fallback.
  - Do not move other edges or alter other clusters' mention provenance.
- **Behavior change for existing single-revision remaps.** Per-span provider or diarizer remaps
  (the AssemblyAI case in §1) switch from whole-entity supersede to `Segments` scope. A one-turn
  correction must not merge two people. This needs its own bug Seed and a regression test before
  this design lands.

**Notes** (`262a`; depends on `4796`): **re-project affected notes for every relabel scope**.

- Do not add a name-token replacement operation. A literal mention of Priya in a quote, title,
  task description or user text is not necessarily speaker attribution, even if it matches a
  whole, case-sensitive token.
- Select affected generated notes/blocks by their cited diarization span ids or source transcript
  segment/window basis, intersected with the batch's affected spans/segments. Legacy notes
  without speaker-span citations use their source window, not a search for the old name.
- Mark those generated projections stale through trusted invalidation and enqueue re-projection
  with the timeline-resolved labels. Preserve human-authored text/edits under the existing notes
  merge contract; no global search/replace in titles, bodies or `NoteBlock` text is permitted.
- The replacement patch's basis is the current transcript basis **plus the diarization span
  revisions actually used**, via `ProjectionBasis::from_transcript_events_and_speaker_spans`.
  It goes through the same apply gate, JSONL patch log and historical replay as every other patch.
  Until that patch is accepted, show the generated note as stale rather than claim it is corrected.

**Materialized projection graph:** a trusted Graph patch. If a node named `superseded_label`
exists, emit `UpsertGraphNode { id, name: canonical }` (a rename), or `MergeGraphNodes` when the
canonical node exists. Both operations already exist (`projections.rs:1909-1952`).

**Timeline** (`dcd3`): re-folding through `speaker_attribution_index` (`timeline.rs:216-240`)
picks up the latest revision automatically, given `ca06` anchors. Add
`TimelineEntry.speaker_provenance: Option<SpeakerProvenanceView { label_source, state,
previous_label }>` so superseded attribution is "visibly marked, not silently rewritten"
(ADR-0026 §3).

**Frontend:** display only. The store already materializes `diarization-span-revision`
latest-wins (`src/hooks/useTauriEvents.ts:145`, `src/utils/speakerTimeline.ts`). The new
`SpeakerIdentityPanel` shows suggestions, "Name this speaker", "This is me", "Not this person",
"Same as…", and voiceprint consent. Every action calls a backend command. React never writes a
label.

### 5.5 Projection-basis invariants (G7)

1. **I1. No transcript rewrite.** Identity never emits `TranscriptEvent`s, so v1 and v2
   transcript hashes (ADR-0042) and `covered_prefix` digests are untouched.
2. **I2. Monotonic revisions only.** Relabels are new revision numbers on existing `span_id`s.
   Same-number edits are impossible because of §5.2 step 2.
3. **I3. History stays valid.** An accepted patch is replayed against the speaker history
   bounded by its `created_at_ms` (`replay_accepted_patches_with_history`, `LedgerHistory`).
   Relabels appended later cannot invalidate it.
4. **I4. Cited spans get a staleness signal.** Requirement **R-basis**: once `1586` makes
   speaker-bearing prompts resolve labels from the timeline, the prompt builder must cite the
   diarization spans it used, through `from_transcript_events_and_speaker_spans`. Then:
   - **Every relabel kind:** invalidate affected generated notes and schedule re-projection of
     their source windows (§5.4); no deterministic name-token rewrite is safe. Any in-flight LLM
     job whose basis cites the old revisions is classified `Revised`
     (`classify_basis_currency`, `projections.rs:1150-1166`) and rescheduled with the new labels
     (ADR-0031, ADR-0045 coalescing slots).
5. **I5. Stamps are not hashed.** The diarization basis is `(span_id, revision_number)` only, so
   identity stamps and `DecisionRecord`s never enter a hash.
6. **I6. Compaction keeps stamps.** Compaction (`e2d5`) must keep each span's latest revision,
   identity stamp included.

### 5.6 Reload and post-session

On load: `replay_speaker_timeline`, then `SessionIdentityLedger::from_timeline`, then a primed
resolver. The UI shows names straight from the labels.

Evidence accumulators start empty, and voice evidence is gone because centroids are discarded
(§3.5). Existing bindings are durable facts. After the session, only text evidence, the user, or
an explicit Phase 4 refinement can change them.

## 6. Live graph before and after binding (worked example)

| Step | Speaker revisions | Live graph | Notes |
|---|---|---|---|
| t=0–90 s | `speaker-c-3` spans labeled "Speaker 4" (rev 1) | Node "Speaker 4" (LLM-minted per `1586`) with edges from its segments. `speakers` lists contain "Speaker 4" | "Speaker 4 proposed moving the launch" |
| t=95 s: "Hi, Priya here", with Priya on the calendar | Binder: SelfIntroduction (heuristic) + RosterStructure gives P = 0.93. `Bind` batch: every c-3 span gets rev 2 with label "Priya Raman" and stamp `Bound` | No "Priya Raman" node exists, so rename in place. The node id is kept, alias "Speaker 4" added, `speakers` lists rewritten. One `GRAPH_DELTA` | Affected generated note is marked stale, then re-projected as "Priya Raman proposed moving the launch"; unrelated name mentions and human edits are preserved |
| t=96 s onward | New c-3 spans are born "Priya Raman" (§5.3) | New mentions resolve directly | — |
| t=20 min: user marks "Not Priya — this is Marcus" | `UserConfirmed` `Bind` to Marcus Lee: rev 3, label "Marcus Lee" | Entity scope: "Priya Raman" is now the node, so `supersede_entity("Priya Raman", "Marcus Lee")`, or rename if Marcus is absent | Affected generated notes are re-projected with Marcus Lee's attribution, not token-replaced |
| If Priya also spoke from another cluster c-7 | c-7 stays "Priya Raman" | A whole-entity supersede would wrongly move c-7's edges. So when another cluster is bound to the same person, the binder chooses **`Segments` scope over c-3's segment ids** | Notes go through the re-projection path |

Rule: `Entity` scope is allowed only when the person being relabeled **away from** is bound to no
other cluster in the session. Otherwise use `Segments`.

## 7. Separation: when to run it and when to skip it

| Situation | Decision | Why |
|---|---|---|
| Live ASR and live diarization | **Skip** | The repo decision stands (`source-separation-bakeoff-2026-06-26.md`, `speaker-channel-routing-2026-06-26.md`). No candidate is proven on meeting speech. AssemblyAI and Deepgram do not separate overlap either |
| Identity embeddings (centroids, voiceprint match, enrollment) | **No separation. Use overlap exclusion** | A new `worker::exclusive_cluster_sample_ranges` drops sample ranges where the segmenter shows more than one local speaker, before embedding for identity. Separation is not needed, because overlap detection already exists in pyannote segmentation. This follows the pyannote "exclusive" idea |
| Enrollment recording | **Reject and re-record** if more than 0.5 s of a second speaker or overlap is detected | Speechmatics and pyannoteAI both require single-speaker enrollment audio |
| Post-session refinement (Phase 4) with `OriginalSessionAudio` retained, overlapped speech ≥ 15%, and identity coverage below target | **Optional offline run** of a 16 kHz separator (MossFormer2_SS_16K or TIGER, Apache-2.0), only to compute per-speaker embeddings for overlapped regions | Separation influences identity evidence and merge or split proposals only, never ASR text or provider routing. It is gated on `c237` fixtures and `dd19` metrics |
| Separated lanes as provider channels | **Never** (in this design) | They are generated audio, not source-native (`AudioChannelProvenanceKind::GeneratedSourceSeparation`) |

## 8. Phasing and acceptance

**P0. Prerequisites (existing Seeds; this design depends on them):**

- `8848`: ship a neural diarizer in release builds
- `ca06`: clustering basis anchors, which the timeline join needs
- `1586`: prompts read timeline-resolved labels, plus R-basis
- **source-local timelines** (new Seed, R-src)
- `4796` before `262a`, for the notes fan-out

**P1. Minimal shippable slice: manual and channel binding on the retcon spine**

Scope:

- the identity stamp
- `SpeakerTimeline::apply_batch`
- the relabel builder and dispatcher
- `reattribute_speaker` (Entity and Segments)
- the commands `relabel_speaker_cluster`, `merge_speaker_clusters`, `mark_cluster_is_self`
- the mic-source self **suggestion**
- `IdentitySettings.self_display_name`
- forward labeling
- timeline `speaker_provenance`
- `SpeakerIdentityPanel`
- the AssemblyAI per-span remap scope fix

There are no S1 calls and no voiceprints. Speaker renames via `merge_graph_entities` are routed to
the new command.

Acceptance (Rust unit and integration tests, pure std, every build):

- `relabel_batch_bind_revises_every_cluster_span_without_new_span_ids`
- `relabel_batch_is_atomic_when_any_span_revision_moved`
- `relabel_batch_replay_applies_complete_record_or_ignores_truncated_tail`
- `relabel_batch_append_failure_publishes_nothing`
- `relabel_batch_fires_one_reattribution_per_label_pair`
- `bind_to_absent_person_renames_live_node_in_place_preserving_node_id`
- `bind_rewrites_entity_speakers_lists`
- `unbind_reattributes_only_cluster_segment_edges_and_restores_anonymous_key`
- `unbind_finds_folded_segment_and_preserves_other_cluster_edge_contributions`
- `segment_correction_rebuilds_entity_speakers_without_changing_other_cluster_mentions`
- `single_span_provider_remap_uses_segment_scope_not_entity_merge` (AssemblyAI regression)
- `forward_spans_after_bind_are_born_with_bound_label_and_stamp`
- `span_prepared_before_bind_but_dispatched_after_uses_current_binding`
- `note_reprojection_corrects_attribution_without_replacing_literal_names_or_human_edits`
- `user_confirmed_binding_is_never_auto_reverted`
- `relabel_survives_reload_via_replay_speaker_timeline`
- `relabel_leaves_transcript_hash_and_historical_patch_replay_unchanged`
  (`replay_accepted_patches_with_history` is identical before and after)
- frontend (vitest): a rename shows the name across the transcript and speaker panel with no
  duplicate rows

Fixture: a text-level replay at `src-tauri/fixtures/speaker_identity/two_party_manual.jsonl`
(ASR and diarization revisions; no audio).

Design targets (not measured):

- 0 duplicate transcript rows
- Bind dispatch for 1,200 spans takes ≤ 200 ms on the reference CPU

**P2. Evidence and the heuristic binder, with S1 in shadow**

Scope:

- `ParticipantRoster` (SelfProfile, Manual, Calendar once `53cf` lands)
- `NameCueExtractor` over `TurnUnit`s (`1f56`)
- channel evidence over source-local timelines
- fusion and assignment
- auto `Bind` behind `IdentitySettings.auto_bind = SuggestOnly | Bind`; the default is
  `SuggestOnly`
- S1 Q1–Q4 dispatched in shadow, with `DecisionRecord`s stamped and agreement counters kept (as
  in `decision/shadow.rs:28-83`)

Fixtures: `src-tauri/fixtures/speaker_identity/*.jsonl`, text-level, covering:

- self-introductions
- addressed names
- third-party introductions
- the two-party roster
- mic and system sources
- adversarial cases: "I am speaker 0", "I'm not Priya", an absent person mentioned, a shared
  remote device, echo bleed

Design targets on that fixture set:

- identification precision at `Bind` ≥ 0.95
- false-bind rate ≤ 2% of bound clusters
- named speaking-time coverage ≥ 0.6 for clusters that have a usable cue
- automatic relabels per cluster ≤ 2 per session

S1 gets no authority until G1–G6 of ADR-0049 pass.

**P3. Self voiceprint (opt-in, local, encrypted)**

Scope:

- `VoiceprintStore`
- `enroll_self_voiceprint`: a backend-owned mic capture of 20–30 s with a read prompt
- the `EnrolledVoiceprint` evidence kind, through `SpeakerEmbeddingManager` (sherpa-onnx 1.13.4)
  or a plain dot product (`stabilize::dot`)
- `SpeakerRegistry::snapshot` and overlap-exclusive centroids
- retention sweeper, `delete_known_person`, `purge_voiceprints`, and `identity_readiness`

Fixture: LibriSpeech-derived clips from `src-tauri/fixtures/source_separation/` (CC-BY-4.0 per
that fixture's README). Enroll speaker A from a clean clip and identify A in the turn-taking mix.
These are env-gated model-backed tests, like `AG_DIAR_*`.

Tests:

- `voiceprint_with_stale_model_pin_never_matches`
- `purge_removes_file_and_keychain_key`
- `no_plaintext_store_when_keychain_unavailable`
- `enrollment_rejects_overlapped_audio`
- `frequently_matched_voiceprint_expires_1095_days_after_creation`
- `unused_voiceprint_expires_at_earlier_inactivity_deadline`

Design targets:

- self-match true-positive rate ≥ 0.9 at false-accept rate ≤ 0.01 on clusters with ≥ 10 s of
  clean speech
- τ calibrated per embedding model pin

**P4. Post-session refinement (requires retained `OriginalSessionAudio`)**

Scope: offline full-session re-diarization, either sherpa offline over the whole session or the
Nemotron-3-Diarization offline spike. The new clustering is matched to the live clusters by
time-overlap matching, which produces `Merge` and `Split` batches as *proposals*. A proposal is
auto-applied only when the result has no `UserConfirmed` conflict. Optional separation follows §7.

Gates:

- DER (collar 0, overlap scored, the pyannote-card protocol) must improve by ≥ 2 pp absolute
  over the live baseline on the multi-speaker fixture, or the phase does not ship.
- Identity-only phases (P1–P3) must not regress DER by more than 0.5 pp against the `c237`
  baseline.

**P5. Extended identity**

- Third-party voiceprints, behind a per-person attestation, default off
- `AccessibilityPlatformUi` roster with active-speaker intervals (per-OS capability)
- "Anonymize this person in past sessions"

## 9. Cross-platform notes

- **Inference.** sherpa-onnx prebuilt libraries already pass the three-OS smoke matrix (`b05b`,
  closed). The parakeet-rs and sherpa ORT link conflict (`compile_error!` in `lib.rs`) is
  unchanged by this design.
- **Keychain.** `keyring` maps to macOS Keychain, Windows Credential Manager, and the Linux Secret
  Service. Where none is available, voiceprints are disabled with a readiness reason, never stored
  in plaintext.
- **Source kinds.** `AudioDeviceKind` can be `None` on some backends, which maps to `Mixed`
  (`audio/capture.rs:384-395`). In that case the self prior is off, and the UI offers
  "This is me" instead.
- **Accessibility roster** (P5): macOS AX needs the Accessibility permission; Windows uses UI
  Automation; Linux uses AT-SPI, which is limited under Wayland. Each is probed by the backend,
  off by default, and documented per OS. None is a default on a single OS (AGENTS.md).
- **Tests.** Identity, cue, fusion, and relabel tests are pure std and run in every CI build.
  Model-backed tests are env-gated.

## 10. Privacy, consent, and purge

- **Classification.** Add a "Speaker identity" row to the data classification matrix
  (`docs/research/local-first-trust-model-2026-06-26.md`):
  - display names in labels are personal data under session retention
  - voiceprints are biometric: local-only, never exported, never synced, never sent to any
    provider

  An optional `DataClass::VoiceprintTemplate` would let the data-movement ledger record local
  enrollment, match, and delete rows, giving ADR-0034-style evidence that no voiceprint ever
  leaves the device.
- **Opt-in.**
  - Voiceprints are off by default. Enabling them shows a consent screen: purpose, local storage,
    retention period, and how to delete. The consent is recorded (`ConsentRecord`).
  - The default allows self-enrollment only. Third-party enrollment (P5) needs a per-person
    attestation ("this person agreed") and a separate setting.
  - BIPA §15 requires notice, a written release, a retention schedule, and destruction. GDPR
    Art. 9 treats biometric identification as special-category data. The research doc has the
    citations.
- **Transient matching.** Session centroids, including those of non-enrolled participants, live
  only in memory and are discarded at session end. This follows Zoom's on-device-then-delete
  pattern.
- **Purge** is crypto-shred: delete the file and the keychain key. Each voiceprint expires at
  the earlier inactivity deadline or absolute 1,095-day deadline from its creation (§3.4);
  regular matching cannot extend the absolute cap.
- **Logging.** Names and `person_ref` never appear in logs: every new type gets a redacted
  `Debug`, following `REDACTED_DEBUG_VALUE`. `DecisionRecord` stores only state digests.

## 11. Exact module and type changes

New modules, under `src-tauri/src/identity/`:

| Module | Contents |
|---|---|
| `mod.rs` | `ClusterRef`, `PersonRef`, `SpeakerBindingState`, `SessionIdentityLedger`, `IdentityResolver` |
| `evidence.rs` | `IdentityEvidenceKind`, `IdentityEvidence`, `EvidenceAccumulator`, fusion and assignment |
| `relabel.rs` | `SpeakerRelabelBatch`, `RelabelKind`, `RelabelScope`, `RelabelBatchRef`, `build_relabel_batch` |
| `binder.rs` | Binder thread and the `IdentityInput` channel |
| `cues.rs` | `NameCueKind`, `NameCue`, `NameCueExtractor` (pure) |
| `roster.rs` | `ParticipantRoster`, `RosterEntry`, `RosterSource` |
| `channel.rs` | Source and channel prior |
| `decisions.rs` | Q1–Q4 builders, schema constants, heuristic floor |
| `voiceprint_store.rs` | `VoiceprintStoreV1`, `KnownPerson`, `Voiceprint`, `EmbeddingModelPin`, `ConsentRecord` (P3) |

Changed files:

- `projections.rs`:
  - `DiarizationSpanRevision.identity`, `SpeakerIdentityStamp`, `SpeakerLabelSource`
  - `SpeakerTimeline::apply_batch`
  - `SpeakerLabelRemap { scope, segment_ids }` (in-memory only, so the change is free)
  - basis-linked notes invalidation and re-projection consumer (`262a`), no token replacement
- `events.rs`: `DiarizationSpanRevisionPayload.identity`, `SPEAKER_IDENTITY_SUGGESTION`
- `speech/mod.rs`:
  - `emit_and_dispatch_speaker_relabel_batch`, `dispatch_speaker_relabel_batch`
  - resolver hooks in `run_clustering_emit_loop` and `diarization_span_revision_for_transcript`
  - remap scope in `dispatch_diarization_span_revision`
- `graph/temporal.rs`: `reattribute_speaker`, rename in place, contribution-preserving edge folds
  and segment-scoped `speakers` derivation
- `graph/entities.rs`: `GraphEntity.person_ref: Option<String>` with `#[serde(default)]`, plus
  persisted per-segment/per-cluster speaker-mention provenance
- `persistence/mod.rs`: versioned single-record relabel batches, atomic replay and tail recovery
- `diarization/stabilize.rs`: `SpeakerRegistry::snapshot()` and clean-speech seconds
- `diarization/worker.rs`: `exclusive_cluster_sample_ranges` and the centroid snapshot channel
- `diarization/mod.rs`: `ClusteringSpeakerStats::record` takes the resolved label
- `timeline.rs`: `TimelineEntry.speaker_provenance`
- `projection_llm.rs`: `pinned_typed_facts` resolves labels through the timeline and the basis
  cites diarization spans (`1586`, R-basis)
- `commands.rs`:
  - `relabel_speaker_cluster`, `mark_cluster_is_self`, `merge_speaker_clusters`
  - `split_speaker_cluster` (P4)
  - `confirm_speaker_suggestion`, `reject_speaker_suggestion`
  - `enroll_self_voiceprint`, `list_known_people`, `delete_known_person`, `purge_voiceprints`
  - `identity_readiness`
  - `merge_graph_entities` keeps working for non-speaker entities
- `settings/mod.rs`: `IdentitySettings { self_display_name, auto_bind, voiceprints_enabled,
  third_party_voiceprints_enabled, voiceprint_retention_days }` (non-secret, `config.yaml`)
- `decision/`: route the identity schemas through `route::dispatch` in the `projection` family,
  with an identity shadow evaluator next to `shadow.rs`
- `crates/ipc-contract`: optional `DataClass::VoiceprintTemplate`
- Frontend:
  - `src/types/index.ts` (payload `identity`)
  - `src/utils/speakerTimeline.ts` (provenance)
  - new `SpeakerIdentityPanel`
  - i18n strings
- `Cargo.toml`: `chacha20poly1305` (P3)

## 12. Open questions and decisions needed

1. **ADR (proposed Seed).** "Bind speaker identity lazily through speaker-revision relabels."
   It must decide:
   - whether an additive `identity` member on `speaker_revisions` v1 is allowed without bumping
     `SPEAKER_REVISIONS_SCHEMA_VERSION` (ADR-0043)
   - where `DecisionRecord`s are placed (ADR-0049 leaves "where a decision lives when it has no
     host artifact" open; here the host is the relabel batch)
   - whether identity questions become an ADR-0049 migration (M7) in the `projection` family
2. **AssemblyAI per-span remap** whole-entity supersede: confirm with a regression test, then fix
   (P1).
3. **Cloud-diarized sessions.** Provider speaker ids churn across reconnects, so identity there
   relies on text, roster, and channel evidence. An optional local embedding sidecar on the
   processed bus is the `5011` "hybrid" mode. Decide whether P3 voiceprints apply to cloud-ASR
   sessions.
4. **Echo-bleed detection** for the mic prior depends on AEC quality (`spike-0bdc`). Thresholds
   are placeholders until there are fixtures.
5. **Nemotron-3-Diarization.** Its arrival-order slots could replace the parakeet-rs Sortformer
   path with 8 speakers under a permissive license. Desktop CPU latency and Rust FFI to
   NeMo-Speech.cpp are unverified, so this needs a spike.
6. **Graph name keying.** Two people with the same display name require the label-suffix rule
   (§3.1). A person-ref-keyed graph index would remove the rule, but it is out of scope here.
