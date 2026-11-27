# Conversation-aware screen snapshots

Research date: **2026-11-22**. Status: **Proposed architecture and research record**, not implementation approval or a hands-on benchmark. Satisfies the architecture questions in `audio-graph-058f`; implementation remains gated on the source-evidence/schema and privacy decisions below. Related owners: `audio-graph-918c` (read-only meeting accessibility evidence), `audio-graph-c282` (retention), and the [lazy speaker-identification design](lazy-speaker-identification.md).

## 0. Direct answer and recommended first slice

**Yes: this is doable from AudioGraph's Rust backend on macOS and Windows, and through a user-approved portal session on Linux.** Capturing a selected window is the relatively straightforward part. Meeting-share detection, keeping the crop inside the actual shared-content pane, deciding which earlier frame a sentence refers to, and obtaining meaningful consent are the difficult parts. Wayland deliberately does not offer the same arbitrary-window discovery/capture model as X11; portal/compositor support and the user's source selection constrain the result. [A1][A2][W1][W2][L1][L2]

**The result can be much more navigable than an image stack:** searchable OCR; optional local captions and image/text embeddings; frame-to-transcript and speaker links; graph-node-to-evidence links; and notes that cite an exact frame/crop. “Show me the churn chart Priya presented” is feasible when the chart is indexed and Priya's *presenter* identity is observed or confirmed. Active speaker is not necessarily presenter, and a visually similar chart is not proof of the exact metric it contains. This is a design inference, not an accuracy claim. [DESIGN] [O1][O2][O3][O4][E1][E2][E3][E4]

**Ship first:** explicitly enabled, manual snapshots plus low-rate visual-change keyframes of a user-selected meeting window's **shared-content rectangle**, on macOS 14+ and Windows 11; local OCR; persisted, reloadable timeline attachments; visible pause/stop/purge controls. Use a detached presentation window where possible; otherwise require a previewed manual crop. Do not ship desktop-wide capture, automatic share recognition, an always-on vision model, hosted vision, or semantic search in that slice. Linux remains an honestly unavailable capability until its portal phase, not a Windows-only default. [DESIGN] [A1][A2][A3][A4][W1][W2][W3][W4]

Backend module boundary: **`src-tauri/src/screen/`**, with `ScreenSession`, `CaptureGrant`, `CaptureTarget`, `SharedContentRegion`, `FrameSample`, `KeyframeRing`, `ScreenSnapshotRevision`, and `ScreenEvidenceRef`. It consumes the existing session clock and transcript/speaker revisions, calls the existing decision route for **text-only** promotion, persists through `persistence/`, and supplies evidence to `timeline.rs`, `projections.rs`, and `graph/temporal.rs`. React only selects/configures a target and displays controls, thumbnails, and citations. See §3 for exact modules and §7 for the necessary persistence/schema extension; none of these proposed types exists merely because it is named here. [DESIGN] [R1][R2][R3][R4][R5][R6][R7]

### Evidence labels

- **Verified documentation/source** means a first-party page or implementation was inspected, not that the product was installed or its accuracy/security independently tested.
- **Not verified** means the cited public evidence does not establish it; it does not prove absence.
- **[INFERENCE]** marks conclusions from those sources. **[DESIGN]** marks AudioGraph proposals, including all suggested rates, budgets, policies and gates below. Those are not measured results.
- No meeting content, credentials, model inference, recording, or authenticated provider call was used in this research. Dynamic Zoom help was also checked through its official indexed text because the static page returned “Loading…”.

## 1. Competitive research: what is actually established

### 1.1 Granola Snapshots

The current help page establishes the following behavior. [G1]

| Question | Verified behavior | Important limit |
|---|---|---|
| When does capture happen? | After the feature is enabled, Granola automatically snapshots content someone else shares and attaches it to the meeting note; the product blog describes relevant slides, charts and designs. | Neither source publishes sampling cadence, visual-difference thresholds, a stable-frame rule, a conversation classifier, or a model. **Conversation-based frame timing is not verified.** |
| How does it find the target? | The engineering account says work on speaker labels using **macOS Accessibility APIs** unlocked understanding/detection in desktop apps. Earlier internal versions used a Chrome extension for Meet. Help says capture is of the shared window/content, not the user's desktop or unrelated apps. | **[INFERENCE]** accessibility identifies meeting/share UI and target geometry, while a capture API acquires pixels. Exact AX selectors, geometry algorithm, Windows accessibility implementation, and native capture API are unpublished. Do not claim Granola uses ScreenCaptureKit or UI Automation without evidence. |
| OS support | Desktop **macOS 14+** and **Windows 11+**. | Not established for Linux or mobile Snapshots. |
| App support | Zoom desktop; Meet in Chrome, with Edge/Brave/similar Chromium browsers additionally specified on Windows; Teams desktop, plus Teams in Chrome/Edge/similar browsers on macOS. | Safari, Firefox and browser Zoom are excluded. Help suggests Gallery view if Zoom snapshots do not appear. Do not silently expand macOS Meet support to every Chromium browser. |
| Hidden window | Snapshots pause when the meeting window is not visible; the indicator says **Paused screen capture**; bringing it back continues capture. | This is a product policy, not evidence the underlying OS cannot capture an occluded window. |
| Permissions and control | On macOS, Accessibility permission is required before the switch can enable, followed by Screen Recording when prompted. Global preference, per-meeting off, re-enable from Images, capturing indicator, previews and per-image removal are documented. | Granola says a restart is unnecessary for its flow; Apple's sample describes a restart. AudioGraph must probe its own packaged build, not inherit either promise. [A3] |
| Privacy and consent | Does **not** snapshot the user's own share. It captures another participant's shared content; if that participant shares an entire desktop, everything they show can be included. Images are **uploaded to Granola** to sync with notes. Consent reminders appear when capture starts, first as a card and later as a shorter reminder. | A reminder is not proof every participant consented. Local-only storage, end-to-end encryption of snapshots, exact image retention, automatic secret redaction and remote participant opt-out enforcement are **not established by these two pages**. |
| Enterprise | Off until a workspace admin enables the workspace feature, after which members may enable it individually. | Workspace permission is not participant consent. |

The [October 6 engineering narrative][G2] explains the progression: January manual NubSnap button → March detection of screenshots the user took → July internal extension-based Meet snapshots → accessibility/speaker-label groundwork and onboarding → Mac support for Meet/Zoom/Teams by September → shipped feature. The first two were prototypes, **not evidence of the current shipped capture algorithm**. The reusable idea is the meeting-UI evidence lane and obvious controls, not an inferred secret relevance model. [G1][G2]

### 1.2 Other screen features

| Product | Verified screen-related capability | Not verified / comparison boundary |
|---|---|---|
| **Cluely** | Mac/Windows desktop; Ask AI explicitly accepts screen, audio and chat context; Get Answer can help with coding problems or spreadsheets on screen. It separately markets hiding its overlay from screen shares. [C1][C2][C3] | No verified Granola-style automatic shared-pane targeting, conversation-timed persistent keyframes, timeline frame citations, hidden-window pause, or image-retention cadence in inspected docs. Hiding an overlay is not consent or data minimization. Its invisibility docs contain conflicting default/opt-in language; do not copy that ambiguity. |
| **Microsoft Teams / Copilot** | Official Teams FAQ explicitly describes synthesis across conversation **and screen-share content**. Teams recap supplies recording/transcript navigation and screen-share context, as cataloged in the existing competitor research. [M1][R8] | The FAQ's after-meeting discussion emphasizes transcript availability; it does not establish that live screen context becomes a durable, searchable gallery of cited screenshots. Exact capture cadence, image store, OCR/vision architecture and cross-platform parity are unverified here. General **Copilot Vision** is a separate explicit screen-sharing product surface, not proof of Teams Snapshots parity. [M2] |
| **Zoom AI Companion** | Meeting-summary settings document **“Use screen share content with OCR”**, extracting readable text to improve summaries. This is a native platform context path, not external desktop scraping. [Z1] | Do not equate OCR text with chart-image understanding, saved screenshots, image embeddings or a frame browser. Exact frame cadence, graphical grounding quality and durable image evidence links were not verified. Host/admin feature controls are not blanket consent for an unrelated local recorder. |
| **Meetily** | Inspected Community README/source and current developer docs establish audio capture/transcription, summaries and imports. [MT1][MT2] | No meeting-screen capture, automatic slides, screenshot search or screenshot-grounded summary contract verified. Community source cannot establish private Pro internals; the absence of a screen feature in these sources is **not** proof no edition has one. |
| **screenpipe** | Current source README describes continuous local computer-work capture, accessibility trees, OCR fallback, source-linked recall/search, app/window filters, and optional encryption. Local history is default; cloud AI, sync and integrations are separate paths. Mac/Windows/Linux downloads are advertised. [SP1] | It is broader desktop/work history, not verified meeting-only capture. Do not transfer advertised platform coverage into guaranteed Wayland/window-filter parity or claim conversation-based shared-slide keyframing. Current repository says **source-available**; do not reuse old blanket MIT claims. |
| **Rewind → Limitless** | Historically Rewind supplied screen memory; Limitless help still describes Rewind for macOS while saying Limitless itself lacks screen recording. The authoritative acquisition notice says Rewind's latest update disables **all screen/audio capture from December 19, 2025**, and Limitless desktop/web no longer record; existing recordings remain accessible through 2026. [RW1][RW2] | The help page dated September 2026 conflicts with the explicit sunset notice. Treat it as stale/inconsistent, not proof screen capture returned. No currently offered Limitless screen-snapshot capability was verified. Historical compression/OCR implementation is not re-established by these pages. |

**Recommendation [INFERENCE]:** adopt Granola's bounded target, visible capture state, per-meeting stop and evidence-in-notes ergonomics; adopt screenpipe's source-linked retrieval concept without its whole-workday capture scope. Treat platform-native screen content as a competitive baseline, not a novel capability AudioGraph alone offers.

## 2. Rust-backend feasibility by OS

### 2.1 Capture is different from share detection

There are three distinct identities:

1. The **local meeting window** (Zoom, Teams, or the browser window).
2. The **shared-content pane** inside that window (or detached presentation window).
3. The presenter's original remote application/window, usually **not a local OS window at all**.

**[INFERENCE]** an HWND/SCWindow cannot select a remote presenter's PowerPoint directly. Capture the local conferencing renderer and crop the content pane. OS capture APIs describe what *our app* may capture; they do not supply a universal semantic event saying another participant in Zoom/Meet/Teams is sharing. Accessibility/platform observations must provide that separate evidence. [A1][W2][L1][AX1][AX2][AX3]

### 2.2 OS matrix

| OS | Acceptable acquisition path | Permission / visibility / crop behavior | Rust recommendation |
|---|---|---|---|
| **macOS** | Enumerate `SCShareableContent`; create `SCContentFilter(desktopIndependentWindow:)` for one `SCWindow`. `SCScreenshotManager.captureImage(contentFilter:configuration:)` captures one frame on **macOS 14+**; a low-rate `SCStream` supplies sampling frames. [A1][A2] | Screen Recording permission and AX trust are separate. Apple's single-window demonstration includes full window content when occluded or off-screen, pauses on minimize, and explains `contentRect`, scale factor and content scale. Use window-relative source geometry; no display fallback. Our visibility policy still pauses capture when hidden. [A3][A4] | Evaluate **`screencapturekit`**, currently source version 12.0.0, `MIT OR Apache-2.0`, with macOS feature gates and screenshot/window examples. The old `svtlabs` URL redirects to `doom-fish`; pin the exact release and audit its Swift/build integration. Framework starts at 12.3, but the first slice deliberately requires 14 for screenshot APIs. [A5] |
| **Windows** | `Windows.Graphics.Capture`; system picker or `IGraphicsCaptureItemInterop::CreateForWindow(HWND)` produces `GraphicsCaptureItem`; D3D11 frame pool supplies surfaces. HWND interop requires **Windows 10 1903+**, while base WGC started in 1803. [W1][W2][W3] | Keep the OS capture border. Borderless capture is a separate permission/capability path and is **not requested**. Copy only valid `ContentSize`, handle resize/device loss, use QPC `SystemRelativeTime`. WGC targets a window rather than a desktop-coordinate rectangle; **[INFERENCE]** it is the appropriate covered-window route, but unconditional freshness for fully occluded/minimized/browser-renderer windows is not promised by the inspected docs. Test these cases; never reuse a stale frame as current. [W1][W4] | Evaluate **`windows-capture`**, MIT, source README 2.0.1, with WGC picker, window selection, update interval and frame callbacks. Do not select its DXGI desktop-duplication path as a fallback: that broadens capture scope. Windows 11 is the proposed initial support floor, not the universal WGC API minimum. [W5] |
| **Linux / Wayland** | `org.freedesktop.portal.ScreenCast`: `CreateSession` → `SelectSources(types=WINDOW, multiple=false)` → `Start` user selection → `OpenPipeWireRemote`; consume authorized PipeWire streams. Check `AvailableSourceTypes` before offering a window. [L1] | Default each meeting to a fresh picker (`persist_mode=0`). Version 4 adds restore tokens/persistence; tokens are single-use and replaced on each successful restoration, may be ignored if invalid/revoked, and do not guarantee prompt-free restoration. Never restore/start solely because a meeting was detected. Current v6 prefers `pipewire-serial` over reusable node IDs. Crop within the selected stream, not a secretly captured monitor. [L1] | Prefer an explicit **`ashpd` + PipeWire** backend; current ashpd docs expose portal selection and serials. Probe protocol versions/backend capabilities. A compositor that lacks WINDOW selection gets a clear unavailable reason, not automatic monitor capture. [L2] |
| **Linux / X11** | XComposite redirected window pixmaps and XDamage are technically available for per-window capture/change observation; XComposite specifies off-screen window storage and re-acquiring pixmaps on map/resize. [L3] | X11's technical access is not user consent. Use the same in-app grant/preview and visibility policy. XWayland access does not give access to arbitrary native Wayland windows. Prefer the portal path when available, including on X11. [INFERENCE] [L1][L3] | A later X11 fallback must remain explicitly selected and scoped to one window, with the same fixtures. Do not base the Linux contract on `XGetImage` of the whole root window. [DESIGN] |

**Occlusion policy [DESIGN]:** capability to capture behind another window does not authorize it. Phase 1 samples only when the target is foreground, or the backend can positively establish visibility; minimized, hidden, locked, off-workspace or uncertain visibility pauses sampling and clears the unpromoted ring. This conservative first implementation can pause when the user brings notes in front. Later visibility improvements must not silently turn into background capture. Permission denial/revocation, closed target, stale frames and protected/blank content have distinct unavailable reasons. Never raise a meeting window or change workspaces to resume capture.

### 2.3 Cross-platform crate decision

| Crate | License / inspected maintenance evidence | Wayland and suitability |
|---|---|---|
| **`xcap`** | Apache-2.0; inspected source `0.9.10`; default-branch commit `b0975cbe8d0c41b67f72f445db0c216501b90b7c` on **2026-11-20** added capture-region/rate/timestamp options. This is concrete recent activity, not a support SLA. [X1][X2] | README calls Wayland screenshot/recording partially supported. Its inspected screenshot path tries GNOME Shell ScreenshotArea, then **Screenshot portal** and cropping, then wlroots-specific wayshot. That is **not** a portable single-window ScreenCast grant. Do not infer “Wayland supported” means safe, unattended, arbitrary-window snapshots. Some recorder options are OS-specific. [X1][X3] |
| **`scap`** | MIT; inspected manifest `0.1.0-beta.1`; latest default-branch commit returned was `c03f15a4631f40cd8d2e36807befb83b314cfeb7`, **2025-08-05**. Manifest still uses windows-capture 1.5.0; this does not establish it incorporates current upstream fixes. [X4][X5] | Implements a Linux portal/PipeWire path, unlike relying on X11 alone. Inspected portal code requests available source types and lacks restore-token handling; some returned metadata is unimplemented. README's `target: None` means primary display. Those defaults are unacceptable for meeting-only capture. [X4][X6] |

**[DESIGN] Choose thin per-OS adapters behind one small AudioGraph interface**, not a cross-platform crate's default settings. Reuse a crate where its scoped-target, lifecycle, frame ownership and permission behavior are proven by fixtures. This adds no competing audio capture: disable audio in all screen adapters; `rsac` remains the only audio/timing owner. Source freshness, licensing and dependency pins must be checked again when implementation lands; no dependency was added by this report.

### 2.4 Detecting someone sharing: reuse `audio-graph-918c`, do not broaden its consent

`918c` currently authorizes a proposed **read-only, off-by-default roster/active-speaker lane with no screen or audio capture**. Reuse its process binding, per-OS capability probes, permission states, bounded reads, timestamping, locale/version fixtures and adapter ownership; add a separately consented `MeetingShareObservation` consumer. Enabling roster access must never enable screenshots. [R9]

- **macOS AX:** trusted accessibility client; inspect only the selected meeting app/window's roles, labels, states and bounds; observe structural/property changes. AX permission is separate from Screen Recording. [AX1]
- **Windows UI Automation:** read exposed element properties/events/bounds in the selected app; bind the resulting pane to its owning HWND. COM/UIA availability is not a guarantee that a canvas exposes semantic content. Do not elevate or use input automation to expose it. [AX2]
- **Linux AT-SPI:** use the accessibility bus's `Accessible`, `Component`, states and events when the conferencing app/browser exposes them. AT-SPI reading does not confer PipeWire capture permission or portable Wayland global coordinates. [AX3][L1]

**Proposed adapters [DESIGN]:** Zoom desktop, Meet in explicitly supported browsers, Teams desktop/web, each keyed by OS + application build + locale. Candidate signals include a remote-presenter label, a share-content container, share-toolbar/state transitions and a detached share window. A generic “Share screen” button only proves the control exists; it does not prove someone is presenting. “Stop sharing” usually concerns self-share and must not be interpreted as remote-share permission. These signal examples are hypotheses for fixtures, **not verified selectors for today's apps**.

Emit `ShareState::{None, Remote, SelfShare, Unknown}` with observation time, expiry, app/window identity, target epoch, geometry revision and optional presenter participant reference. Require positive, fresh remote-share evidence for automatic targeting; stale/ambiguous/unsupported trees produce `Unknown`. Never infer “not sharing” simply from a missing element. Handle gallery/speaker modes, detached panes, browser tab switches, full-screen mode, localization, multiple meetings and simultaneous shares. Revalidate selection after navigation/resize; never retarget by matching a window title alone.

Manual capture works without accessibility by an explicit target/crop grant. Automatic following of share transitions remains a separate phase and cannot bypass that grant. A browser extension could later supply more precise Meet/Teams metadata, but is a separately designed permission boundary, not permission to inject code into arbitrary pages. [DESIGN]

## 3. Proposed backend module and type boundaries

All names in this section are **proposed**, not claims about landed code. Backend ownership follows [AGENTS.md](../../AGENTS.md#architecture-guardrails); the existing timeline is a pure derived fold and the decision contract is text/JSON with choice/noul/score, not media. [R1][R2][R3][R4]

| New module | Types and responsibility | Existing seam |
|---|---|---|
| `src-tauri/src/screen/mod.rs` | `ScreenSession`, `ScreenCapability`, `ScreenCaptureState`, `CaptureGrant`; owns lifecycle and cancellation epoch. | `state.rs`, `commands.rs`, session start/stop, backend app events. |
| `screen/platform/mod.rs`, `macos.rs`, `windows.rs`, `linux_portal.rs` | `ScreenCaptureBackend`, `CaptureTarget`, `WindowToken`, `FrameSample`, `FrameClockAnchor`; permission/capability probes, one selected target, bounded frame output. | Session/audio clock mapping; **no** replacement audio stream. |
| `screen/share.rs` | `MeetingShareObservation`, `ShareState`, `SharedContentRegion`, `TargetEpoch`; consumes 918c observations or manual crop, not a second accessibility crawler. | Existing/planned roster adapters from the identity design. |
| `screen/keyframes.rs` | `VisualChangeGate`, `KeyframeRing`, `KeyframeCandidate`, `CaptureReason`, `KeyframePolicy`. | Local worker scheduling; nonblocking audio path. |
| `screen/ocr.rs` | `LocalOcrEngine`, `OcrRevision`, `OcrTokenBox`, `OcrEnginePin`; platform OCR and explicit portable alternative. | Existing model-manager installation/checksum conventions, local inference workers. |
| `screen/promotion.rs` | `ScreenPromotionContext`, `SnapshotSelection`, `PromotionBasis`; text-only cue/frame choice and graph-attachment proposals. | `decision/route.rs`, canonical transcript and speaker views; observe accepted graph/notes output, not model drafts. |
| `screen/grounding.rs` | `RegionProposal`, `GroundedRegion`, `CoordinateTransform`; phrase/OCR/layout matching and optional local VLM proposals. | OCR output and bounded local model jobs. |
| `screen/evidence.rs` | `ScreenSnapshotRevision`, `ScreenEvidenceRef`, `ScreenEvidenceBasis`, `SnapshotAvailability`, `ScreenEvidenceLedger`. | `claim_evidence.rs`, projection admission/replay and timeline fold. |
| `screen/index.rs` | `ScreenSearchIndex`, `ScreenSearchHit`, `ScreenEmbeddingPin`; rebuildable OCR/caption/vector retrieval. | Session-scoped local retrieval; no new canonical graph authority. |
| `src-tauri/src/persistence/screen.rs` | `ScreenEvidenceWriter`, durable assets/revisions, recovery, manifest registration. | `canonical_log`, `canonical_durability`, `session_artifact_manifest`, `LocalMemoryRepository`. |

`screen/tests/fixtures/` holds synthetic slide sequences, timestamped transcript/speaker revisions, recorded **sanitized** accessibility observations, expected crops and privacy/cancellation traces. Native capture tests use purpose-built local fixture windows, never a developer's real desktop.

### 3.1 Lifecycle and contracts [DESIGN]

State machine: `Off → Selecting → AwaitingPermission → Armed → Capturing`; `Paused(reason)` and `Failed(reason)` are visible terminal/temporary states, not silent fallbacks. `Armed` means a target is selected but no authorized visible share is currently available. Manual mode uses the user's confirmed share-pane selection as its evidence, not a fabricated detected share. Stop returns to `Off` and revokes the grant.

`CaptureGrant` binds session id, target/window identity, target epoch, approved rectangle, mode (manual/keyframes), own-share permission, retention choice, consent acknowledgement/version and expiry. It is not a persistent OS handle or a general desktop grant. Starting audio, enabling accessibility, selecting a cloud text model or reopening a session cannot create it. The selection preview is itself screen access: show the permission/preview purpose **before** acquiring pixels, and never pre-capture every window to populate a custom picker.

`FrameSample` carries capture-native timestamp, mapped `session_media_ms`, clock-mapping uncertainty, dimensions/stride/pixel format, content rectangle, transform revision, target epoch and frame freshness. Map native monotonic timestamps into the existing capture session clock; do not use transcript arrival time as capture time. Preserve wall-clock receipt separately for revision/as-at ordering. Windows exposes QPC time; macOS supplies frame metadata; PipeWire frames need timestamp/discontinuity handling. [W1][A4][L1]

Use one bounded screen worker and latest-frame/backpressure behavior; drop expendable sampling work before blocking audio/transcription. Retain only owned bounded candidate images, not indefinitely checked-out OS surfaces; release WGC/ScreenCaptureKit pool frames promptly. Gate downsampling/hash work before OCR or encoding; do not run a full-resolution image conversion on every compositor update. [DESIGN] The buffer ownership and queue effects are documented by Apple and Microsoft. [A4][W1]

Pause/stop/revocation increments the epoch, cancels outstanding jobs and drops unpromoted pixels, OCR and proposals. Results with the old epoch are rejected **even if a provider/worker returns later**. A new window with a recycled HWND/id is not the same target. Permission recovery, target replacement, self-share transition and crop changes require revalidation/confirmation, never whole-desktop fallback.

## 4. When to capture: a trigger ladder

“Capture” must not hide two different actions: **sampling pixels into memory already constitutes capture**; **promotion** means retaining a selected frame durably. The ring and all local analysis are covered by the same visible opt-in. Retroactive promotion cannot recover anything from before the grant, after pause, or outside the approved target. [DESIGN]

### 4.1 Ladder [DESIGN]

1. **User/manual intent.** “Save current shared frame” retains the selected visible content immediately, with a timestamp. If the content is unavailable, report why; never save the previous frame as if current. Manual selection bypasses semantic relevance thresholds, not privacy/target validation.
2. **Cheap visual keyframing.** Sample the approved content at an initial **1 frame/second**, configurable only within a bounded policy. Downsample for a perceptual hash; compare against the last candidate and retained keyframe. Confirm borderline changes with tiled SSIM or local pixel/text-region differences. SSIM measures image similarity, not whether a business decision changed. [V1]
3. **Capture when stable after change.** A large change opens a candidate episode; initially require **two comparable stable samples** before keeping its best sharp/complete frame. Use hysteresis and a maximum wait so video/animation does not produce an infinite wait or a snapshot storm. These are starting fixture parameters, not measured optimal thresholds. Preserve the old stable keyframe long enough for retrospective references to the prior slide. Local tile changes catch a changed chart value that a global hash could miss; pointer blinks and participant thumbnails outside the approved pane must not trigger.
4. **Conversation-driven promotion.** Run a cheap lexical/time gate on recent stabilized turns for “as you can see,” “this slide,” “the chart on the left,” “that row,” “look at,” and numbers read aloud. Then ask the selected, locally admitted System-1 route bounded **noul/choice** questions. A number alone is weak evidence: “we need three people” during a call need not refer to a screen. Unsupported language or ambiguity abstains. Transcript corrections re-evaluate only affected candidate attachments, not every frame.
5. **Graph/notes-driven attachment.** When an **accepted** decision, action item or entity is minted, query relevant keyframes using its exact supporting spans, time and OCR. Propose a `shown_in` link or frame citation only when matching evidence supports it. A minted node is a reason to *look*, not permission to capture a new target or invent corroboration.

Phase 1 durably keeps deduplicated visual keyframes plus manual captures. Later, a user-selectable **semantic-only retention** mode may keep stable candidates only in the ring until conversational/graph promotion. It must clearly distinguish “sampling” from “saved”; do not silently discard visual-history behavior when upgrading the app.

Initial budgets: **20 seconds**, **12 candidate frames**, and **32 MiB total owned image bytes**, whichever limit is reached first; encode/resize owned candidates once, account for all pixel/OCR buffers, and reject a single over-budget frame rather than exceed the cap. These are proposed bounds for tests, not a memory benchmark. Pin the current and preceding stable candidate while eligible within those bounds. A fast slide shown between samples can be missed; continuous recording is not promised. OCR is lazy/on stable candidates, not every sampled frame. Per-meeting save quotas and minimum separation bound animated/video noise; quota exhaustion is visible, with manual capture still subject to disk/size policy.

### 4.2 Text-only System-1 question shapes

The checked-in [`DecisionRequest` and `Question`](../../src-tauri/crates/decision-contract/src/lib.rs) support text/JSON state and choice/noul/score. They have **no typed media attachments or bounding-box answers**. OCR and geometry *descriptions* can be textual inputs; putting an image/base64 into `Json` does not create an authorized media contract. Reuse the route's calibration, abstention and content-policy handling. [R3][R10]

Illustrative request, with strings only in criteria and no vendor media extensions:

```json
{
  "state": {
    "schema": "screen_promotion_v1",
    "share_state": "remote",
    "turns": [
      {"span_id": "turn-42", "revision": 3, "start_ms": 101000, "end_ms": 104000,
       "text": "As you can see, the chart on the left shows churn falling to 4.2 percent."}
    ],
    "candidates": [
      {"id": "k0", "media_ms": 99000, "stable": true,
       "ocr": "Quarterly churn | 4.2% | Q3", "regions": "left: chart; right: notes"},
      {"id": "k1", "media_ms": 103000, "stable": true,
       "ocr": "Hiring plan | Open roles", "regions": "center: table"}
    ]
  },
  "questions": {
    "visual_reference": {
      "type": "noul",
      "instructions": "Does the newest turn explicitly refer to content being shown now or just shown, rather than using a metaphor or referring to a different document?",
      "criteria": {"true": "A concrete visual reference", "false": "No concrete visual reference or insufficient evidence"}
    },
    "frame": {
      "type": "choice",
      "instructions": "Which supplied candidate best matches the visual reference in the newest turn? Select none if the evidence is ambiguous. Use only candidate descriptions, not imagined pixels.",
      "criteria": {
        "k0": "Stable earlier candidate with churn and 4.2% in OCR",
        "k1": "Stable newer hiring-plan candidate",
        "none": "No supported match, including multiple equally plausible candidates"
      }
    },
    "region": {
      "type": "choice",
      "instructions": "Which supplied textual region description is referred to? Do not invent coordinates.",
      "criteria": {
        "left_chart": "The chart region on the left of k0",
        "right_notes": "The notes region on the right of k0",
        "whole": "The entire supplied shared-content frame",
        "none": "No supported region"
      }
    }
  }
}
```

The `regions` description must come from a real local layout/OCR proposal; the example is synthetic. Questions in one batch are independent: the region question must not assume it can see the frame question's answer. In production, either supply a region choice for each candidate and validate the pair, or run a second bounded question after a frame is selected. Record all candidate ids, transcript revision basis, schema/model pin, raw answer distributions, decision id and abstention reason; diagnostics contain only digests/counts. A `choice` key must resolve to a candidate from the **same grant/epoch**. Keep `none` as an application option, not an invented `Answer::Abstain` variant.

Thresholds are per route/model/schema and language, fitted on held-out meeting labels. A selected answer's confidence is not universally P(correct); do not use a shared `confidence > 0.8` rule. Low confidence, unavailable route, expired ring, missed OCR, model error or policy block means **no automatic promotion**, not cloud fallback. Manual save remains available. [DESIGN] [R10]

### 4.3 Lazy, retroactive evidence

**[DESIGN] Sample anonymously; attach meaning only with evidence.** At 99 s the chart is merely a keyframe. At 104 s the reference resolves to that 99 s frame even though a new slide is already visible. Persist `captured_at_media_ms=99000`, `promoted_at_ms`, trigger spans/revisions and selection evidence separately. Never overwrite the capture timestamp with decision time.

This mirrors [lazy speaker identification](lazy-speaker-identification.md): immediate stable anonymous units first; later, evidence-backed identity/meaning; revisions instead of destructive rewrite; ambiguous suggestions remain nonauthoritative. The analogy stops at persistence: pixels are a genuinely new source-evidence class and cannot be hidden in the existing speaker log. Expired/nonexistent earlier pixels yield unavailable/abstain, not a reconstructed or generated “screenshot.” [DESIGN] [R4]

## 5. Where to capture: grounding inside the approved shared window

### 5.1 Two crops, not one [DESIGN]

First identify the **privacy boundary** (the approved shared-content pane). Only then locate a **semantic region** within it (for example a churn chart). Semantic grounding may only shrink that boundary; it never expands it to find a better answer.

Persist a coordinate transform chain: OS logical points → captured window content pixels → approved pane → normalized region → stored asset pixels. `GroundedRegion` records its parent frame digest, transform/layout revision, normalized rectangle, method, confidence/ambiguity and OCR token ids. Clip/validate finite bounds, reject empty/negative/out-of-parent crops, account for scaling/letterboxing/rotation, and invalidate geometry on resize, browser zoom, DPI or layout changes. Apple and Windows explicitly distinguish content/output geometry; portal position/size is not an unconditional screen-pixel mapping. [A4][W1][L1]

### 5.2 OCR first

| Candidate | Verified contract / license | Accuracy and packaging limits |
|---|---|---|
| **Apple Vision `VNRecognizeTextRequest`** | OS framework; finds text, returns `VNRecognizedTextObservation`, supports fast/accurate recognition levels, language selection/detection and language queries. Use the native framework from a Rust binding, not a hosted OCR service. [O1] | Supported languages depend on OS/request revision; record both. Normalize Vision's geometry into our image convention. No meeting-slide word-error or grounding accuracy was measured here. |
| **`Windows.Media.Ocr::OcrEngine`** | OS API returns lines/words with size/position; query `AvailableRecognizerLanguages`, `MaxImageDimension`, and language support. [O2] | **Officially supported only for desktop apps with package identity (MSIX)**. Do not assume an unpackaged/NSIS Tauri build is supported because WinRT bindings compile. Missing language packs/identity need a capability reason and an explicit local alternative. [O3] |
| **Pure-Rust `ocrs`** | Code **MIT OR Apache-2.0**; current source 0.14.0. Neural detection/recognition via RTen, with ONNX/RTen model formats and JSON word/line layout. **Pretrained weights are CC-BY-SA-4.0**, not the code's permissive license. Upstream explicitly assigns inherited HierText licensing and attribution. [O4][O5] | README calls it early preview, expects more errors than commercial engines, and currently supports the Latin alphabet only. No comparable screenshot-accuracy benchmark was found in the inspected sources; do not claim parity with Vision/Windows OCR. Pin models/checksums and complete attribution/license review before bundling. CLI first-run downloads are not acceptable implicit runtime behavior in AudioGraph. |

**[DESIGN] First-slice OCR:** Vision on macOS; on Windows, native OCR only where the shipped package passes its supported identity/language checks, otherwise a deliberately installed/pinned `ocrs` package after the model-license gate. This is an explicit local route, not a silent network/service fallback. A first-slice release must actually provide local OCR in its declared supported package; it cannot claim Windows OCR complete by testing only a development executable. Unsupported languages remain visible limitations.

Phrase matching pipeline: normalize OCR words while retaining original text and token boxes; match phrases, entities and number/unit variants from recent turns; rank by text agreement, temporal proximity, spatial cues (“left”) and token/layout confidence; merge boxes into the containing chart/table/text panel with padding. “Four point two percent” may match `4.2%`; preserve the original forms and do not normalize away units, negative signs or decimal separators. Repeated labels/numbers produce multiple candidates and abstention, not an arbitrary first match. A text label is evidence for locating a panel, not proof of every plotted value. [DESIGN]

### 5.3 Layout and optional local grounding

Start with cheap proposals: OCR line grouping, whitespace/connected components, borders, spatially grouped text blocks, and fixed pane-relative regions. Propose chart/table/image containers and score text overlap plus location. These are geometry heuristics, not reliable chart-type recognition. Keep a whole-approved-pane evidence view when a smaller crop is uncertain; do not assert a tight crop is correct. [DESIGN]

| Optional model | License and size (exact checkpoint matters) | Grounding / ONNX-Rust path and decision |
|---|---|---|
| **Florence-2 base / large** | MIT; **0.23B / 0.77B**. Microsoft card documents caption-to-phrase grounding, region proposals, dense captions and OCR-with-regions. [F1][F2] | `usls` documents Rust/ONNX Runtime Florence-2 support with FP32/FP16. That is a concrete reference implementation, **not** proof every checkpoint/task has AudioGraph parity. Best first optional local grounding experiment after OCR; validate processor, output-coordinate decoding and task token. [F5] |
| **Qwen-VL family** | Exact checked examples: **Qwen2.5-VL-3B-Instruct uses Qwen Research license**, while **7B-Instruct is Apache-2.0**. Do not call the whole family Apache or select the 3B variant as automatically commercially redistributable. [F3] | Cards document boxes/points, charts, text and layouts. These cards show Transformers, not a ready AudioGraph Rust/ONNX bundle. A separately pinned/exported ONNX vision+decoder/processor path or audited native runtime is new work; this report does **not** verify such integration. Larger-memory evaluation comparator, not baseline. |
| **Moondream 2** | Apache-2.0 checkpoint, approximately **1.9B** family; inspected card pins **2025-06-21** and marks it previous generation relative to Moondream 3. Do not transfer its license or runtime to the newer model. [F4] | Card exposes detect/point/caption/query; `usls` documents ONNX/Rust Moondream2 Q8/Q4f16 support. Revision/task compatibility, tokenizer changes, chart grounding, malformed outputs and no-object handling require parity fixtures. Vendor UI/ChartQA improvements are not meeting-grounding accuracy. [F4][F5] |
| **PaliGemma / PaliGemma 2** | Original model **3B**, designed for task fine-tuning; `paligemma2-3b-pt-224` is gated under **Gemma terms**, not Apache/MIT. [F6] | Original card documents generated box/segmentation outputs; pre-trained weights are not automatically a zero-shot conversation-grounder. An exact fine-tuned/exported checkpoint and Rust/ONNX decoder would be needed; a current turnkey path was **not verified**. Not recommended for the initial slice. |

Parameter counts are not RAM/VRAM/download promises. Quantization, vision resolution, decoder cache and execution provider change runtime cost. `usls` is a useful MIT reference, but auto-download behavior and its model zoo are not a reason to add its entire dependency/model surface. Reuse AudioGraph model management; pin each artifact/license/processor and benchmark CPU plus each supported accelerator. [DESIGN] [F5]

Evaluate grounding on synthetic and consented slide/dashboard/UI fixtures: repeated titles, two similar charts, tiny axis labels, dense tables, dark mode, mixed languages, cursor-over-label, compression, side-by-side shares, and “the one we just saw.” Score crop intersection-over-union, reference resolution precision/abstention, lost-context errors and unsupported numeric claims. Image hallucinations may propose a region; they never create graph facts without admitted source evidence. [DESIGN]

### 5.4 Image-capable decision models: evaluation only

These providers' **media interfaces exceed today's typed AudioGraph decision contract**. A dedicated typed `ScreenVisionRequest`/`GroundingResult` evaluation interface with byte/size/coordinate/egress validation is required; `WireRequest` preserving vendor JSON is not a runtime media implementation. [R3][R10]

- **Jev Box:** official TypeSafe schema includes image `assets`, PNG/base64 or multipart, normalized box output, and an explicit **`TypeSafe-Danger-Zone-Images: true`** header. Its example uses `jev-preview`; do not assume a text `jev-latest` route/model pin is a supported Box deployment. Pin the actual served vision model and schema. [D1]
- **Clef-omni:** current Cloudflare model documents image-capable choice/noul/score, **30B total / 3B active**, up to **4 embedded images**, PNG/JPEG/WebP, **4 MiB and 16 MP each / 8 MiB total decoded**, no remote URLs; each image is rescaled/tokenized and capped at **1,024 tokens**. It can compare supplied regions via choice; a typed box-output facility was **not** established. Fine-print downscaling can erase slide text. [D2]
- **Baseten image questions:** documented `state.messages` user-content image parts accept base64 PNG/JPEG/WebP or restricted S3/GCS/Azure HTTPS image URLs, **16 images maximum**, no audio/video, and **512 total options** with noul counting two. Returned choice/noul/score is not box extraction. Concentration `confidence` is not correctness probability. No private-screen evaluation is authorized by having a Baseten key. [D3]

**[DESIGN] Evaluation sequence:** public/synthetic fixtures first; local OCR/layout baseline; then separately approved hosted comparators only after the full egress checklist (§8). No automatic fallback from local failure to hosted vision, no live capture promotion driven by an unevaluated media route, no remote asset URL fetch side channel, and no private screenshot in a readiness probe.

## 6. Semantic navigation, graph links and notes

### 6.1 Snapshot index [DESIGN]

Index each retained snapshot/crop by:

- Snapshot id, asset digest, session id, capture time, stable-content interval, target epoch and availability.
- Original OCR text/token boxes plus OCR revision/engine/language pin.
- Optional **local, generated** caption with model pin and revision, visibly distinct from observed OCR.
- Image embedding and corresponding text encoder pin; optional OCR/caption text-search index.
- Supporting transcript span ids **and revisions**; diarization span references; optional observed presenter participant reference; confirmed user corrections.

Never use an arbitrary 224-pixel embedding input as the only preserved readable evidence. Keep the approved readable asset under its explicit retention policy; thumbnails and vectors are derivatives. Model-generated captions, OCR and vectors are screen-derived content, not harmless telemetry.

| Embedding candidate | License / checked size | ONNX/Rust path and limit |
|---|---|---|
| **SigLIP base patch16-224** | Apache-2.0; published full FP32 `model.safetensors` **813 MB** (both towers; not an ONNX download measurement). Card specifies 224-pixel input, English training and a 64-token text path. [E1][E2] | `usls` documents SigLIP ONNX/Rust FP32/FP16/quantized support. Pin image/text towers and preprocessing together; verify embedding parity/norm/ranking. Useful semantic matching, not OCR or exact chart arithmetic. [E4] |
| **OpenAI CLIP ViT-B/32** | Official repository MIT; published Transformers PyTorch artifact **605 MB**; config gives **512-dimensional** projection, **77-token** text limit and 224-pixel images. [E3] | `usls` documents CLIP ONNX/Rust support. Official card warns of fine-grained/counting, language and deployment limitations; its presence is not an endorsement of untested screenshot retrieval or face/person identification. A constrained held-out meeting retrieval evaluation is mandatory. [E3][E4] |

Those sizes are source checkpoint files, not peak runtime memory or the footprint of a quantized/tower-only export. No ONNX model was downloaded or executed here. Choose one model initially, install it explicitly with hashes/license metadata, and rebuild indexes on pin changes; never mix incomparable vector spaces. [DESIGN]

### 6.2 Time, speaker, presenter and graph evidence [DESIGN]

`ScreenEvidenceRef` identifies **a snapshot revision, exact asset digest and optional crop**. A separate attachment contains the supporting transcript/speaker basis and source graph/note revision. Do not encode screenshots as transcript segments or freeze a display name in a filename.

- **Timeline:** extend the backend fold with screen evidence input; add a `TimelineScreenMarker` collection and per-span evidence refs. Independent screen markers permit manual snapshots during silence; do not invent a spoken span to host them. Sort by media time. React renders thumbnails in Review/timeline and opens the exact approved frame/crop. The view remains derived, not a second canonical timeline log. [R1]
- **Active speaker:** resolve overlapping speech using `SpeakerTimeline` at capture time; store span refs and uncertainty, allowing multiple speakers/none. Re-fold on speaker revisions. “Speaker at capture” does not imply “presenter.”
- **Presenter:** use positive platform share metadata or a user's confirmation for `presented_by`; otherwise unknown. A person speaking about someone else's deck cannot silently become its presenter.
- **Graph:** an entity mentioned in speech and matched in OCR can gain `shown_in(entity, snapshot)` or a typed `evidence` attachment with both token-box and speech anchors. Co-occurrence alone proves only co-occurrence; it does not prove a relationship, decision or exact plotted numeric value. Keep observed/displayed, spoken and inferred provenance separate. Use the existing admitted-claim/retcon path; attaching a frame cannot bypass `claim_evidence.rs` validators. [R2][R7]
- **Notes:** citations such as `[Frame 04, 01:39]` resolve through typed refs, not paths or model-invented Markdown URLs. Show frame time, source status, crop and supporting speech. User removal/expiry becomes “Evidence unavailable — deleted/expired,” not a broken image or a newly generated substitute. A later OCR/caption correction does not change the original captured pixels.

### 6.3 Hybrid query example [DESIGN]

For **“show me the churn chart Priya presented”**:

1. Apply explicit authorized session/date scope locally; resolve Priya to a confirmed person/participant, retaining ambiguity between same-name people.
2. Retrieve lexical OCR matches for churn; retrieve image/text embedding matches for chart/churn and optional caption/text candidates.
3. Filter/boost **observed or confirmed presenter** links to Priya; keep speaker-coincident results separately labeled, never falsely satisfy a strict presenter filter with them.
4. Fuse bounded rankings (for example reciprocal-rank fusion), deduplicate asset/crop revisions, rerank with OCR and temporal evidence; expose missing presenter evidence and empty results.
5. Return thumbnails with citations/jump-to-timeline and graph-node-to-evidence links. An optional answer generator receives only policy-approved retrieved evidence, keeps its basis, and cites typed frame ids. Searching private local pixels must not implicitly send them to a configured cloud notes/chat provider.

Evaluate recall@k/ranking, exact-slide versus similar-slide mistakes, presenter false attribution, empty-query behavior, retcon consistency, deletion leakage and retrieval latency. Visual similarity is a ranking signal, never permission or source validation.

## 7. Projection revisions and durable session artifacts

### 7.1 Current source-grounded state

- `timeline.rs::build_session_timeline` folds `TranscriptLedger`, `SpeakerTimeline`, and live `TemporalKnowledgeGraph` edges. It carries transcript text, resolved speaker and edge ids, **not screenshots**. The module uses media time and redacted `Debug`. [R1]
- `ProjectionBasis` covers transcript span revisions, optional compacted-prefix digest, diarization revisions, transcript hash and summary boundary. Its default transcript-only constructor passes no speaker spans. `ProjectionPatch`/`ProjectionOperation` and admitted per-item evidence currently do **not** express screen-source basis or frame attachments. An additive citation is not a replacement for the whole consistency basis. [R2][R7]
- [ADR-0043][R5] freezes **four** canonical stream ids: transcript revisions, speaker revisions, projection patches, data-movement events. Screen capture does not already have a registered source stream.
- The strict typed `persistence/session_artifact_manifest.rs` kernel has content digests, privacy classes, managed identities and `Present` / `Unavailable` / `Residual` states; its declared artifact vocabulary has no screen kind. Its module calls itself dormant. The older computed `persistence::SessionArtifactKind`/`SessionArtifactDescriptor` is a separate runtime vocabulary. [R6]
- `sessions/mod.rs` still enumerates default artifact paths and removes them before retiring the control plane/index. The source explicitly describes the manifest writer as dormant. **Adding a new image directory to a design is not proof existing purge/export/recovery will include it.** [R6]

### 7.2 Proposed source and revision model [DESIGN]

**Decision recommended for a prerequisite ADR:** screen captures are genuinely new original evidence. Add one registered canonical source domain **`screen_snapshot_revisions`**, with `SCREEN_SNAPSHOT_REVISIONS_SCHEMA_VERSION`, through an explicit ADR-0043 registry extension and session-semantics compatibility-floor/migration decision. Do **not** invent an unregistered JSONL file, repurpose speaker revisions, or treat image bytes/OCR as a data-movement audit payload. This is a deliberate proposed extension of the frozen registry, **not something this research authorizes**. No new log is required for ring frames, share observations, search results or the timeline view.

`ScreenSnapshotRevision` schema specification:

- `snapshot_id` (session-local stable opaque id), monotonic `revision_number`, `schema_version`, and event identity.
- Immutable `captured_at_media_ms`, native clock anchor/uncertainty and wall-clock receipt; `promoted_at_ms` separate.
- Asset content identity `{sha256, byte_length}`, managed artifact identity, dimensions/encoding, approved parent region, transform/target epoch and capture reason.
- Source grant/policy/consent receipt **ids**, not raw app titles or copied participant rosters.
- Availability/tombstone reason, retention decision and revision timestamp. Store no OCR plaintext, caption or secret in an audit-only receipt.

Original pixels and capture time never mutate. A redacted rendition/crop has its own content digest and parent lineage. A replacement capture is a new snapshot id; availability, consent metadata or corrected attribution creates a revision. `OcrRevision`/caption revisions and evidence attachments are derived projections with separate pins/bases, not a fake change to original pixels. Latest accepted revision wins for current views; stale or conflicting revisions fail, historical views use revisions available by their as-at time.

### 7.3 Preserve existing projection hashes; version screen dependence [DESIGN]

1. Leave all **existing transcript hashes and v1 basis bytes/semantics unchanged**. Screen samples must not rewrite `TranscriptEvent` text, timestamps or speakers.
2. Introduce a versioned composite **`ScreenEvidenceBasis`** in screen-aware projection payloads: the existing complete `ProjectionBasis` plus sorted exact screen snapshot/asset refs and the OCR/caption revisions actually read. Include diarization revisions when speaker/presenter claims depend on them. An attachment citing one frame does not narrow a prompt's full transcript consistency basis.
3. Extend typed claim evidence and projection operations for frame attachments only after version-dispatch/reader compatibility is specified. New enum variants cannot be made old-reader-safe by `serde(default)`. Pin screen-aware payload domain versions and compatibility floor; old clients must refuse/offer read-only unsupported semantics rather than silently drop frame support or mutate/purge unknown artifacts.
4. Validate transcript/speaker currency **and** screen/derivative versions at admission; resolve exact cited bytes/digests, crop bounds and OCR-token ids. Model-selected ids cannot manufacture an asset or widen scope. OCR revision/retention deletion/speaker correction invalidates or re-derives affected attachments with the existing projection/graph retcon mechanism.
5. Distinguish **evidence removed** from **claim disproved**. Purging pixels makes a citation unavailable and removes searchable derivatives. It does not automatically retract a decision independently supported by speech; any claim relying solely on deleted visual evidence becomes unsupported/unavailable under its evidence policy. No manual note prose is silently overwritten.
6. Replay historical accepted patches against their exact historical transcript/speaker/screen basis, not today's OCR or presenter label. Derivative caches may be rebuilt only from still-available, correctly pinned sources. Notes/graph/timeline share one resolver, not separate best-effort joins.

This follows the existing basis-versus-citation distinction and latest-revision projection model, while explicitly acknowledging the new source/schema work. It is not a claim screen support can be added without a migration. [R1][R2][R5][R6][R7]

### 7.4 Manifest, export, crash recovery and purge [DESIGN]

Resolve all paths through backend/Tauri `user_data` APIs. Proposed layout under the managed data root: `screen/<session-id>/assets/<snapshot-id>/<digest>.png` (or an approved lossless encoding), screen revision log, and derived OCR/thumbnail/index files. Do not persist raw HWNDs as portable identity or deduplicate source assets across sessions in the first design; cross-session shared blobs complicate atomic deletion.

Extend the strict typed manifest and the actual runtime inventory/repository adapter with screen-specific kinds, for example `ScreenSnapshotRevisions`, `ScreenSnapshotImage`, `ScreenOcr`, `ScreenThumbnail`, `ScreenSearchIndex`. Images are `OriginalEvidence`; OCR/captions/indexes are `DerivedSessionMemory`; source revisions are `CanonicalSessionMemory`. Whether the manifest itself needs a schema bump is part of explicit compatibility dispatch, not an assumption its closed enums ignore unknown values. Use content identities, constrained managed paths and existing canonical coordination/durability primitives. [R6]

Commit sequence: validate grant/epoch → encode/redact approved image → write/sync managed asset → durably register asset availability and append source revision under an approved recovery protocol → only then expose a saved snapshot and schedule derivatives. The ADR must define crash cuts between blob install, manifest update and source-log append, orphan classification/recovery, and deletion tombstones; there is no claim of an already-existing multi-file transaction. Partial failure cannot advertise a persisted screenshot. The ring may remain ephemeral but never becomes an undisclosed disk spool.

Session purge quiesces capture/OCR/embedding workers, revokes grants, enumerates **every** source image, crop, thumbnail, OCR/caption record, vector/index row, temporary/quarantined file and screen revision artifact, and preserves retryable residual reports on failed unlink. Remove the session index/control-plane entries only in the existing safe order. Retention deletion of a single frame preserves a content-free unavailable citation; full-session deletion removes the complete session artifact set. Export is explicit about including screen evidence, defaults to excluding pixels unless selected, and includes truthful unavailable reasons and derivative/egress provenance. Do not let a manifest-only addition leave the current computed inventory unaware. [DESIGN] [R6]

## 8. Privacy, consent, retention and egress

### 8.1 Capture policy [DESIGN]

- Add a provider/audit data class **`ScreenContent`** with origin/representation metadata: pixels, cropped/redacted pixels, OCR text, captions and embeddings retain screen-content sensitivity. Declare `TranscriptText`, `Notes` or `GraphContext` additionally when supplied. A provider accepting strings rather than images still receives screen-derived content if OCR is included.
- **Off by default; local-only by default.** Separate global capability availability from the per-session capture grant. Persisting the user's preferred mode is not permission to start next time.
- Capture only the previewed shared-content region in the selected meeting window. Whole desktop is never the default or a fallback. If a remote presenter shares their whole desktop, that remote material can appear inside the approved pane; show that warning rather than pretending it is sanitized. Exclude participant tiles/chat/private notes through crop verification; first-slice full meeting-window recording is not silently called “shared content.”
- **Own share disabled by default**, following the useful Granola boundary. It needs a separate explicit per-session opt-in and preview of the exact local application/region; never switch from the meeting pane to the presenter's local monitor automatically. Avoid recursive previews. `SelfShare` or uncertain presenter transitions pause remote auto capture until revalidated.
- Show persistent sampling/saved/paused state and target scope even if no image has been retained. Stop/pause works without finding a hidden setting. Preserve OS capture indicators/borders. No stealth mode, startup capture, covert accessibility-triggered capture or telemetry screenshot path.
- OS permission is not participant consent. Before enabling, ask the user to attest they have the appropriate meeting consent; show a reminder at capture start, target/own-share changes and resume into a new meeting. A bot-free local app cannot claim to notify remote participants or honor their platform opt-out automatically. Respect known host/organization prohibitions; local law and participant expectations need product/legal review, not an invented universal consent rule.

### 8.2 Retention and redaction [DESIGN]

Extend **existing `audio-graph-c282`**, rather than creating a competing general retention policy. Proposed screen-specific matrix:

| Artifact | Proposed default after explicit capture enablement | Delete/export behavior |
|---|---|---|
| Sampling ring / unsaved candidates | RAM only; bounded as §4; clear on pause/stop/revocation/session end. | Never exported or recovered; no disk cache. OS paging/crash dumps are a residual exposure, so do not promise forensic erasure of RAM. |
| Saved source images/crops | Local with the session until deletion; user/global/per-session shorter expiry, and explicit “do not retain” mode. Screen retention is separately shown, not silently inherited from audio consent. | Explicit export inclusion; single-frame deletion/expiry produces unavailable refs and removes derivatives; full-session purge removes all. |
| OCR, captions, thumbnails, embeddings/index | No longer than their source; retain model/transform pins. | Delete at source purge, including search caches and OCR text. Backups and user-created exports are separately disclosed, not falsely claimed remotely erased. |
| Consent/retention/egress audit | Content-free ids, counts, policy/version, destination, timestamp and result. | Subject to c282's audit-retention rules; no reconstructable screen text or raw image in diagnostics. |

Optional redaction: local secret/PII pattern/entity detectors over OCR boxes, user masks and denied regions, applied **before durable storage and before any egress**. Redaction is fallible (missed OCR, codes in diagrams, visual faces/logos); make no “all secrets removed” claim. Prefer opaque masks for strong removal over reversible/weak blur; blur can be a clearly labeled user option. Store only the redacted rendition unless the user separately chooses to retain the original; preserve digest/lineage and re-run derived indexing from the permitted rendition. A purge removes unredacted transient buffers and all derived forms as far as the application controls them. Redaction failure under a policy requiring redaction blocks save/send. [DESIGN]

### 8.3 Hosted vision and indirect egress gates

Apply **every** item of the [Provider Addition Content-Egress Checklist](provider-architecture.md#provider-addition-content-egress-checklist), including private shadow evaluation, captions, embeddings, cloud notes/chat prompts that include OCR and exports to integrations. [R11]

1. Truthful registry descriptor/data classes, source policy, lifecycle, transport, model identity, destination/region, retention/training/subprocessors and media limits. Stay Planned/Watch until complete.
2. No-content readiness/model discovery. Never use a real screenshot or OCR excerpt as a health probe.
3. Session command policy gate before starting the media request/session; explicit selection of the **screen** data class, provider and scope. A transcript-provider choice does not authorize screen egress.
4. Low-level `ProviderContentEgressPolicy` guard **before first content write**, including multipart bodies, image/base64 parts, OCR prompts, thumbnails, embeddings and asset uploads. Enforce limits/redaction locally; never upload to an object store just to obtain a vendor-compatible image URL without the same authorization.
5. Blocked-policy transport fixtures prove **zero content bytes written**, not merely a parser error. Test cancellation/revocation races and no retry/fallback into another hosted provider.
6. Payload/secret-free logs, errors, telemetry and debug; no screenshot fixture from a private user session. Audit via the existing data-movement ledger with ids/digests/counts and admitted destinations, not pixels/text.
7. Provider-specific tests for Jev media header/Box coordinates, Cloudflare image decoding/rescaling/truncation and Baseten image-host/option/context limits. Provider “zero retention” is a claim to review against the actual account/region/contract, not proof a particular request used it. [D1][D2][D3]

Screen OCR/captions may contain prompt injection. Treat them as quoted untrusted source data, never as instructions to capture more, reveal credentials, search another session, run commands or send data. Typed candidate ids, region limits, local validators and egress policy remain the authority. A confidence score cannot grant permission. [DESIGN]

## 9. Delivery phases and acceptance fixtures

All phases require truthful capability states on **all three OS families**, not Windows-only defaults. Numeric parameters below are proposed acceptance/fixture configuration, not benchmark results. No phase includes a hidden capture path. Tests listed are **to implement/run**, not checks performed by this research.

### Phase 0 — ratify evidence and privacy contracts

Approve the registry/schema extension, compatibility floor, composite evidence basis, manifest/recovery protocol and screen additions to c282 before persistence code. Confirm packaged Windows OCR route/license and the initial supported OS matrix.

**Acceptance/fixtures:** golden old-session replay/hash fixtures remain byte-identical; new source-version dispatch and unsupported-reader behavior are specified; capture grant has no implicit start route; consent/retention/data-class matrix covers pixels and all derivatives; synthetic crash-cut inventory covers blob/manifest/log publication and purge residuals. Cross-platform path/identity/symlink/reparse-point tests are defined for Windows/macOS/Linux. This is an ADR/product/security gate, not unilateral approval from this document.

### Phase 1 — smallest shippable slice: Mac + Windows manual/keyframes, local OCR, timeline

Implement scoped picker/preview, manual crop, per-session grant, native capture adapters, visual stable-keyframe ladder, bounded ring, native/local OCR, durable evidence/artifact inventory, timeline thumbnails, pause/stop, individual removal and full purge. No auto accessibility targeting is required to save a user-selected meeting pane. No hosted model or semantic search.

**Acceptance/fixtures:**

- On macOS 14+ and Windows 11 packaged builds, a local fixture meeting window contains shared slide text plus deliberately sensitive chat/participant/desktop canaries. Saved pixels/OCR/thumbnails contain the approved slide and **none of the out-of-region canaries**. A cancelled picker/disabled grant produces no sample, OCR job or file.
- A deterministic sequence (static slide, cursor blink, animated transition, stable slide, tiny chart-value update, fast transient slide, video) tests dedupe/stability/noise behavior and explicitly records missed sub-sampling content. Bounded memory/queue policy is asserted under overload; audio work never waits for screen work.
- Hide/minimize/foreground loss/lock/target close/permission revoke pauses with fresh UI evidence; in-flight old-epoch results cannot save. Restore does not save a stale frame as current. No widening to display capture or raising another app.
- DPI/Retina, mixed-scale monitors, window resize, browser zoom, letterboxing, SDR/HDR conversion and blank/protected content fixtures validate crop geometry/freshness or a visible unsupported reason.
- OCR fixtures verify exact words/boxes on known text, report per-language errors, reject absent Windows package identity/language support, and prove the installed portable route runs offline without a download. License/attribution packaging is included.
- Capture during silence has an independent marker. Restart/replay opens the exact saved image at its original media time. Simulated disk-full/crash cuts do not advertise unsaved assets; export includes only selected screen artifacts; purge removes source and derivatives or reports retryable residuals.
- macOS and Windows native evidence is mandatory. Linux returns explicit unsupported/not-yet-available capability without opening a portal, capture stream or broad fallback. Shared pure Rust policy/replay fixtures run on Linux too.

### Phase 2 — conversation promotion, share observations and region crops

Reuse 918c's read-only adapters under separate consent; add remote/self/unknown share states and pane bounds. Add text-only decision promotion, retrospective frame selection, OCR/layout region grounding and accepted decision/action/entity attachments. Evaluate Florence-2 or Moondream locally only if the simple baseline's measured errors warrant it.

**Acceptance/fixtures:**

- Sanitized Zoom/Meet/Teams accessibility observations for every declared OS/app/locale cover hidden controls, detached share, two browser tabs, version drift, same display names, simultaneous shares and self-share. Stale/ambiguous input gives `Unknown`; disabling screen capture while roster reading remains enabled produces no pixels. No automated click is needed to reveal controls.
- A chart at 99 s, new slide at 103 s and visual reference at 104 s selects the eligible 99 s frame. Expired/pre-consent/cross-epoch frames cannot be resurrected. “Three hires” without a visual reference abstains. Each number/phrase/layout fixture has human frame/region labels.
- Transcript retcon changes the cue or entity; speaker/presenter correction changes attribution; OCR revision changes candidate support. Attachments are revised or made unavailable without mutating capture pixels/time or duplicating notes/graph entries.
- Held-out per-language promotion precision/risk-versus-coverage, false presenter attribution, crop IoU/ambiguity, cold/warm latency and resource use are reported against the lexical/OCR baseline. Threshold approval requires actual labels; no fabricated accuracy target/result.
- Mac/Windows end-to-end remain required; Linux gets replayable observation/decision/grounding fixtures without claiming live capture. Unsupported native Linux app trees stay manual in the later portal phase.

### Phase 3 — semantic search and cited navigation

Add a pinned local SigLIP/CLIP-family pair, OCR/caption lexical retrieval, hybrid ranking, timeline and graph evidence jumps, and typed frame citations in notes/answers. Keep caption generation optional and local.

**Acceptance/fixtures:** held-out queries include churn chart, duplicate chart revisions, multilingual OCR, same-name presenters, active-speaker-versus-presenter mismatch and strict empty results. Record retrieval ranking/false matches, source jumps and model-pin migration behavior. Reindexing preserves original evidence; delete/purge makes all OCR/caption/vector caches nonretrievable; generated note citations cannot point to nonexistent/foreign-session crops. Prove local-only search/answer paths make no network content writes. Run the same offline corpus on macOS/Windows/Linux CPU; accelerators are explicit optional capabilities with parity tests, not a CUDA-only default.

### Phase 4 — Linux portal support, Wayland and X11

Implement the explicit `ashpd`/PipeWire session with WINDOW-only selection, version negotiation, cancellation/revocation and timestamp/geometry handling; optionally add a separately selected X11 per-window backend only if needed. No compositor-specific privileged bypass.

**Acceptance/fixtures:** test **GNOME Wayland, KDE Wayland and an X11 session**, recording compositor/portal/PipeWire versions. A wlroots backend is declared supported only after the same tests; missing WINDOW support produces a clear unavailable result. Test per-meeting picker cancel, start error, session-closed signal, resolution/hotplug/suspend discontinuities, source close, stale node-id reuse/serial handling, and no silent display substitution. Version-4 restore-token fixtures test one-use rotation, revocation/stale-token reprompt and explicit user-initiated resume; version-3 does not pretend persistence exists. Run the common OCR/timing/keyframe/purge corpus and live chosen-window privacy canary test on each declared backend. Mac/Windows continue using their native paths; Linux rollout does not silently swap their capture library/defaults.

## 10. Findings, uncertainties and handoff

**Design recommendation:** implement screen context, but only after the Phase 0 explicit-capture/source-evidence decision; start with Phase 1, not a conversation-to-VLM loop. The local keyframe + lazy promotion design is feasible without changing the text-only System-1 contract. Semantic navigation is an incremental projection/retrieval layer, not permission to let screen-derived guesses become source facts. [INFERENCE; sources above]

Uncertainties deliberately retained:

- Granola does not publish the exact capture-selection algorithm, native capture APIs or Windows detection implementation. No claim it already uses the proposed trigger ladder.
- WGC covered/minimized-window freshness, packaged macOS permission recovery, conferencing renderer throttling and compositor-specific portal behavior require native device tests. They were not tested here.
- Accessibility selectors/pane geometry for current Zoom/Meet/Teams builds and languages are not yet verified. Source evidence of Granola's approach proves the technique is useful, not our adapters' robustness.
- No screen-grounding/OCR/retrieval quality, latency, RAM/VRAM, power or per-meeting storage figures were measured. Vendor model benchmarks do not fill those gaps.
- `ocrs` code and model licenses differ; Windows native OCR package identity is a release constraint; broad Qwen/PaliGemma/Moondream family names are not sufficient model-license pins.
- The current artifact manifest is a dormant kernel alongside a separate runtime inventory. Durable screen support requires actual integration, compatibility and crash-recovery work, not just an enum addition.
- Consent across participant jurisdictions, organization policies and self-share workflows remains a product/legal gate. No local UX can truthfully attest remote parties have consented merely from a permission dialog.

**Observed checks:** read the requested Seeds, project guardrails, related designs/research, decision contract and relevant persistence/session/timeline/projection source; inspected the primary sources linked below; searched existing Seeds for screen/snapshot/OCR/accessibility/retention overlaps. **Not run:** builds, tests, linters, formatters, native capture/OCR/model benchmarks or private/provider smoke tests; implementation/integration verification belongs to the owner. Only this destination document was written; no code, tracker state, commit or push was changed.

## Primary sources and repository evidence

All web sources were inspected on 2026-11-22. Moving branches/pages establish only the inspected state; freeze release/model/file hashes during implementation. Vendor documentation is evidence of its stated contract, not independent security or quality certification.

### Competitors

[G1]: https://docs.granola.ai/help-center/taking-notes/capture-shared-screens
[G2]: https://www.granola.ai/blog/some-ideas-take-a-few-fridays
[C1]: https://docs.cluely.com/quickstart
[C2]: https://docs.cluely.com/feature/liveinsights
[C3]: https://docs.cluely.com/feature/settings
[M1]: https://support.microsoft.com/en-us/teams/platform/frequently-asked-questions-about-copilot-in-microsoft-teams
[M2]: https://support.microsoft.com/en-us/microsoft-365-copilot/use-vision-microsoft-365-copilot
[Z1]: https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0057960
[MT1]: https://github.com/Zackriya-Solutions/meetily
[MT2]: https://docs.meetily.ai/
[SP1]: https://github.com/screenpipe/screenpipe
[RW1]: https://www.limitless.ai/
[RW2]: https://help.limitless.ai/en/articles/9135527-screen-recording

Additional Cluely invisibility evidence: <https://docs.cluely.com/feature/undetectability>. It does not establish consent or screenshot retention.

### Capture, accessibility and crate implementations

[A1]: https://developer.apple.com/documentation/screencapturekit/sccontentfilter
[A2]: https://developer.apple.com/documentation/screencapturekit/scscreenshotmanager
[A3]: https://developer.apple.com/documentation/screencapturekit/capturing-screen-content-in-macos
[A4]: https://developer.apple.com/videos/play/wwdc2022/10155/
[A5]: https://github.com/doom-fish/screencapturekit-rs
[W1]: https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture
[W2]: https://learn.microsoft.com/en-us/windows/win32/api/windows.graphics.capture.interop/nf-windows-graphics-capture-interop-igraphicscaptureiteminterop-createforwindow
[W3]: https://blogs.windows.com/windowsdeveloper/2019/09/16/new-ways-to-do-screen-capture/
[W4]: https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.isborderrequired
[W5]: https://github.com/NiiightmareXD/windows-capture
[L1]: https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html
[L2]: https://docs.rs/ashpd/latest/ashpd/desktop/screencast/index.html
[L3]: https://www.x.org/releases/current/doc/compositeproto/compositeproto.txt
[AX1]: https://developer.apple.com/documentation/applicationservices/1459186-axisprocesstrustedwithoptions
[AX2]: https://learn.microsoft.com/en-us/windows/win32/winauto/entry-uiauto-win32
[AX3]: https://gnome.pages.gitlab.gnome.org/at-spi2-core/libatspi/
[X1]: https://github.com/nashaofu/xcap
[X2]: https://github.com/nashaofu/xcap/commit/b0975cbe8d0c41b67f72f445db0c216501b90b7c
[X3]: https://github.com/nashaofu/xcap/blob/b0975cbe8d0c41b67f72f445db0c216501b90b7c/src/linux/wayland_capture.rs
[X4]: https://github.com/CapSoftware/scap
[X5]: https://github.com/CapSoftware/scap/commit/c03f15a4631f40cd8d2e36807befb83b314cfeb7
[X6]: https://github.com/CapSoftware/scap/blob/c03f15a4631f40cd8d2e36807befb83b314cfeb7/src/capturer/engine/linux/portal.rs

License/version manifests: [screencapturekit](https://github.com/doom-fish/screencapturekit-rs/blob/main/Cargo.toml), [xcap](https://github.com/nashaofu/xcap/blob/b0975cbe8d0c41b67f72f445db0c216501b90b7c/Cargo.toml), [scap](https://github.com/CapSoftware/scap/blob/c03f15a4631f40cd8d2e36807befb83b314cfeb7/Cargo.toml). Native Windows reference: [Win32CaptureSample](https://github.com/robmikh/Win32CaptureSample).

### OCR, grounding, embeddings and media decisions

[O1]: https://developer.apple.com/documentation/vision/vnrecognizetextrequest
[O2]: https://learn.microsoft.com/en-us/uwp/api/windows.media.ocr.ocrengine
[O3]: https://learn.microsoft.com/en-us/uwp/api/windows.media.ocr
[O4]: https://github.com/robertknight/ocrs
[O5]: https://github.com/robertknight/ocrs-models/blob/main/LICENSE.md
[V1]: https://www.cns.nyu.edu/~lcv/ssim/
[F1]: https://huggingface.co/microsoft/Florence-2-base
[F2]: https://arxiv.org/abs/2311.06242
[F3]: https://huggingface.co/Qwen/Qwen2.5-VL-3B-Instruct
[F4]: https://huggingface.co/vikhyatk/moondream2
[F5]: https://github.com/jamjamjon/usls/blob/main/docs/model-zoo/vlm.md
[F6]: https://ai.google.dev/gemma/docs/paligemma/model-card
[E1]: https://huggingface.co/google/siglip-base-patch16-224
[E2]: https://huggingface.co/google/siglip-base-patch16-224/tree/main
[E3]: https://github.com/openai/CLIP
[E4]: https://github.com/jamjamjon/usls/blob/main/docs/model-zoo/embedding.md
[D1]: https://docs.typesafe.ai/api/system-one.md
[D2]: https://developers.cloudflare.com/workers-ai/models/clef-omni/
[D3]: https://docs.baseten.co/reference/inference-api/system-one

Exact supplementary artifacts: [ocrs crate manifest](https://github.com/robertknight/ocrs/blob/main/ocrs/Cargo.toml), [OCR weights/license](https://huggingface.co/robertknight/ocrs), [Qwen2.5-VL-7B Apache checkpoint](https://huggingface.co/Qwen/Qwen2.5-VL-7B-Instruct), [Moondream release/1.9B family link](https://moondream.ai/blog/moondream-2025-06-21-release), [gated PaliGemma2 checkpoint](https://huggingface.co/google/paligemma2-3b-pt-224), [CLIP checkpoint sizes](https://huggingface.co/openai/clip-vit-base-patch32/tree/main), [CLIP config](https://huggingface.co/openai/clip-vit-base-patch32/blob/main/config.json), [CLIP limitations](https://huggingface.co/openai/clip-vit-base-patch32), [usls repository/license](https://github.com/jamjamjon/usls).

### AudioGraph source evidence

[R1]: ../../src-tauri/src/timeline.rs
[R2]: ../../src-tauri/src/projections.rs
[R3]: ../../src-tauri/crates/decision-contract/src/lib.rs
[R4]: lazy-speaker-identification.md
[R5]: ../adr/0043-freeze-canonical-event-stream-registry.md
[R6]: ../../src-tauri/src/persistence/session_artifact_manifest.rs
[R7]: ../../src-tauri/src/claim_evidence.rs
[R8]: ../research/meeting-assistant-competitors-2026-11-21.md
[R9]: ../../.seeds/issues.jsonl
[R10]: ../research/system-one-model-landscape-2026-11-21.md
[R11]: provider-architecture.md#provider-addition-content-egress-checklist

Persistence runtime inventory: [`persistence/mod.rs`](../../src-tauri/src/persistence/mod.rs), especially the computed `SessionArtifactKind`, `LocalMemoryRepository` and `session_artifacts` adapters. Session inventory/removal: [`sessions/mod.rs`](../../src-tauri/src/sessions/mod.rs), `default_session_artifact_paths`, `permanently_delete_session` and control-plane retirement. Governing timeline context: [ADR-0026](../adr/0026-session-timeline-who-said-what-when.md). Graph integration: [`graph/temporal.rs`](../../src-tauri/src/graph/temporal.rs). Seed reads: `audio-graph-058f`, `audio-graph-918c`, `audio-graph-c282`; read-only duplicate search found no dedicated screen implementation/OCR/search/portal Seeds beyond 058f. Extend existing 918c/c282 responsibilities rather than duplicating their general scope.

## Proposed Seeds

These are proposals for the integrating owner, not tracker mutations or approval to implement. All refer to this design; priorities reflect the existing later-capability posture. Phase 2–4 depend on Phase 1; Phase 1 depends on the contract gate. Retention implementation belongs under existing c282 and accessibility infrastructure under existing 918c.

```json
[
  {
    "title": "Ratify screen evidence source, revision basis and explicit-capture privacy contracts",
    "type": "task",
    "priority": 3,
    "parent": "audio-graph-058f",
    "description": "Approve the Phase 0 decisions in docs/designs/conversation-aware-screen-snapshots.md before capture persistence: explicit per-session target/crop grant; proposed screen_snapshot_revisions registry extension; composite screen evidence basis; compatibility-floor/version dispatch; asset/manifest/log crash protocol; ScreenContent egress class. Extend existing c282 for screen retention rather than create a second retention project. Confirm Windows packaged OCR support and ocrs model redistribution licensing.",
    "acceptance": "Reviewed ADR explicitly resolves new canonical source/schema and old-reader behavior; golden v1 hashes/replay remain unchanged; screen evidence and all derivatives appear in the privacy/retention matrix; grant cannot be created by audio/accessibility/provider enablement; Mac/Windows/Linux capability, path, crash-cut and zero-content-write fixtures are specified. No hidden capture path is approved implicitly."
  },
  {
    "title": "Ship opt-in Mac and Windows shared-pane snapshots with local OCR and timeline evidence",
    "type": "feature",
    "priority": 3,
    "parent": "audio-graph-058f",
    "description": "After the screen contract gate, implement Phase 1 in src-tauri/src/screen/ and persistence/screen.rs: explicitly previewed meeting-window crop, manual and stable visual-change keyframes, bounded RAM ring, per-OS native capture, supported packaged local OCR, typed durable assets, timeline markers, visible pause/stop/removal, export and purge. Linux reports unavailable without a display fallback. No hosted vision or automatic share detector.",
    "acceptance": "Native macOS 14+ and Windows 11 packaged fixtures prove selected-region canaries never escape, off/cancel/revoke yield no capture/save, hidden/stale targets pause, old epochs cannot save, timing/DPI/resize/HDR are correct or explicitly unsupported, OCR runs offline with licenses, silence snapshots reload as independent timeline markers, crash/disk-full never advertises unsaved assets, and manifest plus runtime inventory export/purge every derivative with retryable residual reporting. Common policy/replay fixtures also run on Linux."
  },
  {
    "title": "Extend opt-in meeting accessibility evidence with bounded share-state and pane observations",
    "type": "task",
    "priority": 4,
    "parent": "audio-graph-918c",
    "description": "Reuse 918c process binding, per-OS AX/UIA/AT-SPI probes and sanitized observations for MeetingShareObservation without broadening roster consent into capture consent. Add remote/self/unknown states, optional presenter identity, pane geometry revision and expiry for supported Zoom/Meet/Teams builds/locales. Consume only under the separate screen grant from the screen-snapshots design.",
    "acceptance": "Per-OS/app/locale fixtures cover detached and simultaneous shares, browser tab changes, localization, stale/hidden trees and self-share; a Share button alone never proves remote sharing; unsupported or stale evidence yields Unknown; disabling screenshots while roster remains enabled acquires no pixels; no input automation or whole-desktop fallback; live support is declared only for tested OS/app combinations."
  },
  {
    "title": "Add text-only conversational keyframe promotion and evidence-grounded region crops",
    "type": "feature",
    "priority": 4,
    "parent": "audio-graph-058f",
    "description": "Implement Phase 2 using the existing decision route and choice/noul text contract over bounded turns and real OCR/layout candidates. Select earlier eligible ring frames, ground phrases/numbers/spatial cues inside the approved pane, and attach evidence to accepted decision/action/entity output through versioned bases. Evaluate optional local grounding only against the OCR baseline; no hosted fallback.",
    "acceptance": "Held-out multilingual labels report promotion risk/coverage, crop IoU/ambiguity and resource use; the earlier-chart fixture selects the correct eligible frame and expired/pre-consent/cross-epoch frames abstain; metaphor/numbers-only negatives do not promote; transcript/speaker/OCR retcons repair attachments without mutating pixels/time or duplicating notes; all crops remain within the grant; Mac/Windows native and Linux offline replay fixtures pass."
  },
  {
    "title": "Index screen evidence locally for hybrid search and typed note/graph frame citations",
    "type": "feature",
    "priority": 4,
    "parent": "audio-graph-058f",
    "description": "Implement Phase 3 with one pinned licensed SigLIP/CLIP-family image/text pair, OCR/caption lexical retrieval, local hybrid ranking and typed ScreenEvidenceRef navigation. Distinguish observed presenter from contemporaneous active speaker and generated captions from OCR. Reuse projection admission/replay and the session artifact inventory.",
    "acceptance": "Offline Mac/Windows/Linux corpus reports ranking and false-presenter results for churn-chart, duplicate-slide, multilingual, same-name and empty-result queries; graph/timeline/note jumps resolve exact frame/crop digests; model-pin reindexing never mixes vector spaces; unsupported citations are rejected; source purge removes OCR/caption/thumbnail/vector caches and leaves truthful unavailable refs; local-only paths write no network content."
  },
  {
    "title": "Support explicit Linux portal window snapshots with PipeWire lifecycle and privacy parity",
    "type": "feature",
    "priority": 4,
    "parent": "audio-graph-058f",
    "description": "Implement Phase 4 through ashpd ScreenCast and PipeWire, WINDOW-only user selection, per-meeting picker by default, explicit restore-token handling when supported, media-clock/geometry mapping and the common OCR/keyframe/artifact contracts. Optional X11 per-window capture is separately selected; missing Wayland window support never silently chooses a monitor.",
    "acceptance": "GNOME Wayland, KDE Wayland and X11 native canary tests prove target isolation, cancellation/revocation/session-close, resize/hotplug/suspend and no desktop fallback; version-4 token rotation/revocation and older-portal limits are fixture-covered; v6 serial/node reuse is handled; common OCR/timing/bounds/purge tests pass; wlroots is advertised only after equivalent evidence; Mac/Windows native behavior remains unchanged."
  }
]
```
