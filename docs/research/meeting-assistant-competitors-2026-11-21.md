# Meeting assistants: what AudioGraph should adopt or beat

Research date: 2026-11-21. Scope: public primary-source research and selected source inspection, not a hands-on product benchmark.

## Question and direct answer

How do Meetily, Granola, Cluely, Plaud, “yogurt,” and the principal independent and platform-native meeting assistants work, and what should AudioGraph adopt or beat?

**AudioGraph should compete on trustworthy, correctable meeting memory, not just transcription or a claim to be bot-free.** Bot-free capture is now offered by cloud products including Fireflies, Fathom, Read, tl;dv and Zoom My Notes; local inference is already offered by Meetily, Yogurt and Anarlog. Adopt platform-derived speaker names, a low-friction human-notes-to-summary workflow, cited cross-meeting answers, calendar pre-briefs, useful templates, and explicit capture/retention/sharing controls. Beat them through revision-aware speaker attribution that repairs downstream notes and graph, temporal evidence rather than undifferentiated chat history, and measured low-cost decision routing. These are product recommendations, not measured superiority claims. [M1][][G1][][F1][][FA1][][R1][][T1][][Z1][][Y1][][H1][]

**“Yogurt” is real, but ambiguous.** The strongest match is **FrigadeHQ/Yogurt**, at **yogurt.so**, a free MIT desktop meeting-notes app for macOS, Windows and Linux. A second real product, **jarvisrchen/Yogurt**, at **yogurt-ai.dev**, is a macOS meeting copilot with a free MIT source edition and separately distributed Pro features. Do not merge their architectures, pricing or privacy claims. The user’s wording alone cannot establish which one they meant; both are covered below. [Y1][][Y2][][J1][][J2][]

Three important corrections to older competitive comparisons:

- **Meetily Community is open source; Meetily Pro is commercial.** The inspected Community source does not establish the implementation of Pro diarization. Detailed support documentation says final speaker identification for a live recording happens **after stop**, despite a Pro marketing page saying “live as you record.” [M1][][M2][][M3][]
- **Hyprnote → Char → Anarlog is no longer one current product identity.** The open-source meeting app is now Anarlog. Current Char is a separate private-beta agentic notepad, not the current name of that source tree. [H1][][H2][][H3][]
- **Limitless is a continuity/portability warning, not a normal purchasable competitor.** Meta acquired it; new Pendant sales ended December 5, 2025, existing Pendant service is promised through 2026, subscriptions became free for continuing customers, desktop/web recording stopped, and Rewind capture was sunset. Do not compare against obsolete hardware/subscription offers. [L1][]

## Evidence standard and reading guide

- Links below are first-party documentation, policies, repositories or first-party product pages. A policy or certification statement is **the vendor’s assertion**, not an independent security assessment performed here.
- **Unknown** means not established by the cited public evidence. It does not mean the feature is absent. Closed-product implementation languages, exact current model pins, diarization networks, embeddings, confidence thresholds and internal classifier routing are generally unpublished.
- **Local capture ≠ local transcription ≠ local generation ≠ local storage.** Offline recording is also not proof of offline inference. “No model training,” “zero retention at an AI subprocessor,” and “no stored meeting in the product” are different claims.
- Pricing is the **tier shape**, not a procurement quote. Quotas, annual billing, rollout eligibility, mobile availability, regional restrictions and enterprise contracts can change independently.
- Source snapshots inspected: Meetily `2166bc6fe3ba585580f3f94b79cda7e7b91deb76`; Frigade Yogurt `e1398289ab2bad7aba75ad5501ae16bbce9ac81a`. Links to those implementations are pinned below. Anarlog’s README is a moving source and is identified as such. No competitor was installed, recorded, purchased, or sent meeting content.

## 1. Capture, inference, source availability and speaker identity

“Hosted” below means a service-side workflow; it does not identify the undisclosed model vendor or promise an on-device mode.

| Product | Capture and platform coverage | STT / LLM; source and stack | Diarization versus real-name attachment |
|---|---|---|---|
| **Meetily** | Bot-free microphone + system audio; Community macOS/Windows/Linux; Pro installers macOS Apple Silicon/Windows, Linux planned. | Community MIT; Tauri 2/Rust + Next.js/React, SQLite; Whisper through whisper-rs, Parakeet through ONNX Runtime, Silero VAD. Local summaries or Ollama/BYOK. Product docs also offer OpenAI-compatible/Groq/OpenAI **audio egress**. | Community has no Pro speaker identification. Pro locally groups voices after recording/import/retranscription; user listens to samples, names, merges, or corrects lines. Host/Guest fallback where available. Names are meeting-local, not automatic recognition across meetings. [M1][][M2][][M3][][M4][][M5][][M6][][M7][] |
| **Granola** | Bot-free mic/system capture on macOS/Windows; iOS/Android for in-person capture. | Cloud STT (providers include Deepgram/AssemblyAI), cloud LLMs (OpenAI/Anthropic cited); local cache, cloud notes. No public application source verified. | Default Me/Them is input-source attribution, not N-person diarization. Desktop speaker tags read display names and active-speaker state using Accessibility or a Meet extension. Shared-room users remain one platform participant; contextual name inference is explicitly distinguished from those tags. [G1][][G2][][G3][] |
| **Cluely** | Bot-free microphone/system capture, with optional screen context; desktop Mac/Windows documentation, iOS download offered. | Hosted transcription/AI service; no offline inference or public production app source verified. Exact STT/LLM stack unknown. | Docs claim speaker-identified transcripts, but do not establish how real names are attached, voice enrollment, or cross-session voice recognition. Calendar/person briefs are not proof of acoustic identification. [C1][][C2][][C3][][C4][] |
| **Plaud** | Hardware recorders/wearables for in-person and phone audio; App/Web/Desktop workspace. Hardware records before upload; not a meeting bot. | Cloud transcription and AI processing; no on-device STT/LLM established for this workflow. Closed device/service implementation; exact models change and are not pinned by the membership docs. | Separate speakers → Speaker 1/2. Optional **self** voice profile: at least 30 seconds and consent. Does not automatically recognize everyone else or automatically match calendar attendees; users assign other names per recording, optionally from calendar suggestions. [P1][][P2][][P3][][P4][] |
| **Yogurt / Frigade** | Bot-free mic + call/system audio; Mac/Windows/Linux. | MIT; Electron/React/TypeScript with native helpers. whisper.cpp, Apple SpeechAnalyzer on supported Macs; local open-weight LLM through llama.cpp/node-llama-cpp. | Uses supported Zoom/Teams/Meet UI participant/active-speaker information. Inspected attribution is a deterministic time-overlap join, not a voiceprint recognizer. Falls back to Them under ambiguity. No general acoustic multi-person room identity proven. [Y1][][Y2][][Y3][][Y4][][Y5][] |
| **Yogurt / jarvisrchen** | Bot-free macOS mic + system capture; Rust service with browser UI at localhost. | Free source MIT; Pro code excluded. README says **Deepgram is default**, with selectable local Parakeet/Whisper-family models. LLM via BYOK/OpenAI-compatible service or local model. | Public examples use Me/Them; no verified N-person naming, voice enrollment or cross-session speaker identification. Website’s “audio never leaves” wording conflicts with README’s default cloud-STT path. [J1][][J2][] |
| **Otter** | Calendar-connected meeting Notetaker for Zoom/Teams/Meet; direct recording/import; web and iOS/Android, desktop downloads offered. | Hosted STT/AI; no offline/local inference established; application source/model architecture not verified. | Zoom bot can use participant names live. Otherwise post-processing recognizes previously tagged voices; manual tagging teaches future recognition. Account, Workspace and Channel sharing scope affects which voice profiles can be reused. [O1][][O2][] |
| **Fireflies** | Bot **or** bot-free desktop system capture; Mac/Windows desktop, web/mobile workflows. | Hosted transcript/notes/AskFred service; no local STT/LLM established. Source/model stack not public in reviewed docs. | Bot capture has named speaker labels. Desktop no-bot capture explicitly lacks those labels (generic Speaker 1/2); do not promise parity between modes. Calendar event linking supplies title/attendees, not proven voice matching. [F1][][F2][] |
| **Fathom** | Bot plus 3.0 bot-free video/audio/transcript modes. Mac rollout; Windows bot-free limited Teams alpha as of Nov 10; iOS in-person capture. No Linux/Chromebook capture. | Upload/service-side processing, cloud account history; no local inference established; app/model implementation not public in reviewed docs. | Participant/alias matching identifies the recording user in Zoom; unresolved match invokes a Name Picker. iOS offers opt-in voiceprinting **only of the user**, not every attendee. In-room/hybrid naming remains limited. [FA1][][FA2][][FA3][][FA4][] |
| **Krisp** | Bot-free device/virtual microphone/speaker path; optional bot recording; Mac/Windows and mobile offering. | Noise cancellation is local; meeting STT is cloud. Recording storage/meeting intelligence are cloud-service features. No production app source or exact meeting-model stack verified. | The retrieved public material does not establish the current diarization model, voice enrollment, or complete real-name mapping algorithm. Do not confuse primary-voice isolation with identifying every remote speaker. [K1][][K2][][K3][] |
| **Anarlog (Hyprnote / former Char)** | Bot-free system/mic capture. macOS mature; Windows/Linux beta; mobile beta source/client noted. | Community MIT; enterprise components commercial/source-visible. Tauri 2/Rust + React/TypeScript; local SQLite/files. Local STT where supported or selected provider; local/BYOK/cloud intelligence. Managed cloud currently prefers Soniox 5; Auto intelligence uses Claude Sonnet via OpenRouter. | Transcript labels can be assigned to participants, applied globally, or changed for selected words/segments. Provider-specific diarization and manual identity must not be conflated; cross-session voice enrollment not established here. [H1][][H2][][H4][] |
| **Char (current separate product)** | Private-beta notepad aggregates meetings, messages, calendar/files/tools; capture details not established. | Separate current codebase, not proven open source by Anarlog’s license. Current STT/LLM deployment and production platform matrix unknown. | No verified diarization/naming mechanism; treat as an adjacent memory/action product rather than silently inheriting Anarlog capabilities. [H1][][H3][] |
| **Limitless** | Continuing Pendant hardware/mobile service; historical desktop/web capture no longer offered. | Cloud/service-backed transcription, summaries and memory; no verified local inference or public production source. | Saved Voices: manually label clean audio; ≥20 seconds cumulative enrollment, most recent 90 seconds used; automatic labels apply going forward, not retroactively. Different recording conditions may need separate voice entries. [L1][][L2][][L3][] |
| **tl;dv** | Calendar bot, bot-free desktop, mobile in-person, extension and upload. Website now advertises all three capture families. | Hosted STT/LLM; policy names AssemblyAI/ElevenLabs and Anthropic/Google Vertex. No local inference established; no public app source verified. | Markets named speakers across capture modes. The precise non-bot/room identity mechanism, enrollment and confidence policy are not established; a named transcript example is not proof of biometric recognition. [T1][][T2][][T3][] |
| **Read.ai** | Web meeting agent/platform icon plus desktop/mobile local capture; Mac/Windows/iOS/Android. | Hosted processing/account reports; no local STT/LLM established; exact models/app source unknown. | Web agent knows platform speakers. Hybrid room transcript can use Conference Room–Speaker N; user can assign a name to one turn or all. Equivalent desktop real-name inference is not proven by local-capture support. [R1][][R2][][R3][][R4][] |
| **Zoom AI Companion / My Notes** | Native Zoom meeting audio; My Notes also mic/system capture for third-party/in-person meetings, phone and mobile mic modes. | Zoom cloud ASR; cloud AI with deployment/model options and regional constraints. No offline model execution established; closed service. | Native participant identity is available within Zoom. Room smart **video** name tags use enrolled faces or manually entered names; do not equate face name tags with voiceprint-based speaker recognition. Third-party/room naming parity not established. [Z1][][Z2][][Z3][][Z4][][Z5][] |
| **Teams Copilot / Intelligent Recap** | Platform-native Teams meetings/calls/events; Teams Rooms/BYOD room recognition subject to licensing and settings. Desktop/mobile recap. | Cloud transcription; recap notes/tasks use Azure OpenAI plus internal models. Other timeline features do not all use GPT. No on-device meeting inference established. | Individual platform participants provide identity; room speaker recognition adds opt-in active or express voice enrollment with tenant/admin controls. Voice/face profiles are deletable and purpose-bound. [MS1][][MS2][][MS3][][MS4][] |
| **Google Meet Gemini notes** | Platform-native Meet; “take notes” also offers in-person mobile capture. | Google cloud/Gemini workflow, Docs/Drive/Calendar outputs; no on-device STT/LLM or public application source established. | Platform context supports person-referenced notes, but reviewed notes docs do not promise voice enrollment or reliable individual names for everyone sharing one mic. Do not infer attendee-to-voice mapping merely from the calendar invitation. [GO1][][GO2][][GO3][] |

## 2. Live help, durable memory, packaging and governance

Each row combines feature and policy evidence; missing details remain explicit rather than inferred from industry norms.

| Product | Live versus after; memory/search; templates and sharing | Privacy/retention and pricing shape |
|---|---|---|
| **Meetily** | Live transcript; post-meeting summary. Import/retranscribe; meeting workspace/search. Pro templates and PDF/DOCX/Markdown; opt-in MCP/CLI/signed webhooks. External assistant access is not proof of built-in cited all-meeting Q&A. | Local path retains data on device under user control; BYOK summary sends text; selected cloud STT sends audio. Community free; Pro per-seat annual subscription; Enterprise custom. Full local artifact expiry defaults not established. [M1][][M2][][M3][][M7][] |
| **Granola** | Human notes guide enhanced summary; templates, recipes, pre-briefs; chat across note/folder/library, citations; integrations and sharing controls/MCP. Live transcript does not make it equivalent to Cluely’s unsolicited coaching cards. | No retained meeting audio; encrypted US AWS text storage, indefinite by default unless deleted/custom retention. Own model-improvement use opt-out; Enterprise off by default; third-party training disallowed. Basic free, Business, Enterprise. [G1][][G2][][G4][] |
| **Cluely** | Live dynamic question/keyword cards, “what should I say,” follow-up, fact-check and chat; custom modes/knowledge base. Postcall notes, next steps, resume, manual edits/public summary links; pricing includes questions across past meetings. | Policy says no training/sale; retains data while account exists/as needed with archival/legal exceptions. Claimed E2EE is available by contacting support: architecture/coverage not independently established. Starter free, Pro subscription, additional undetectability tier. [C1][][C2][][C3][][C4][] |
| **Plaud** | Primarily record → transcribe → notes. Professional templates, multiple summary formats, Ask Plaud across recordings, AutoFlow follow-ups/exports, team workspace. No Cluely-style live coaching verified. | With Private Cloud Sync off, server audio/transcripts/summaries deleted after processing; on, encrypted storage until request. Hardware purchase plus Starter minute allowance, Pro larger allowance, Unlimited daily cap, Team seats; minute top-ups. [P1][][P2][][P3][][P4][] |
| **Yogurt / Frigade** | Live transcript, notes-enhanced summary, sourced Ask across meeting/folder/library, folders/search, vocabulary, Markdown export and opt-in local read-only MCP. Custom template marketplace or multi-user hosted sharing not established. | No account; local processing/storage after model download, update/calendar-link network use. Audio saved by default, switch off for future recordings. MCP off by default but enabled same-user apps can read; consuming assistants may upload data. Free MIT, no subscription required. [Y1][][Y2][] |
| **Yogurt / jarvisrchen** | Live transcript; human notes enhanced with timestamp links. Last-30-days Ask free; Pro all-time Ask, live recap, format picker, comparison, export/import and self-hosted sync. | Local files; cloud-STT and chosen LLM boundaries must be disclosed. Local-STT website describes audio deletion; blanket application of this to every build/provider is unverified. Free source vs paid licensed Pro; cloud sync uses user’s Vercel/Supabase. [J1][][J2][] |
| **Otter** | Live transcript, summaries/actions, AI Chat within/across meetings, templates/workflows, team channels, exports/CRM, MCP. | Hosted audio/text; Enterprise custom retention and speaker-learning disable controls documented. Exact general default purge interval/training treatment not established by the inspected pages. Basic free; Pro; Business; Enterprise, with minutes/history/chat/template/import limits and enterprise controls. [O1][][O2][][O3][][O4][] |
| **Fireflies** | Live floating pane: AI Notes, transcript, AskFred, Skills/manual notes. Postcall search, tasks, daily/meeting briefs, CRM and collaboration. | No training on meeting content; AI vendors do not retain after processing. Product storage remains separate; Enterprise private storage/custom retention/admin rules. Free → Pro → Business → Enterprise; extra AI usage can be separately metered. No-bot mode does not save audio/video, unlike bot mode. [F1][][F2][][F3][] |
| **Fathom** | 3.0 live summary and scratchpad; postcall templates/actions; account-wide Ask; team sharing/clips/integrations. Mobile currently lacks Ask and meeting search. | Unlimited retained recordings advertised; Enterprise retention is support-configured, one policy per team. Notice must include email and/or chat during 3.0 setup. Free and paid individual/team/enterprise packaging; consult current plan page rather than older Premium tables. [FA1][][FA2][][FA4][][FA5][][FA6][] |
| **Krisp** | Transcript/meeting notes/actions, AI Chat, organized history/sharing, integrations/MCP; local noise/accent tools improve live communication. Detailed template and cross-meeting-Q&A scope not established by retrieved help. | Cloud STT processing audio deleted after processing; explicitly retained recordings stay cloud encrypted. Those are separate modes. Seven-day trial → Core → Advanced/Enterprise controls; storage and audio/AI features bundled. Exact default note/recording expiry unknown. [K1][][K2][][K3][] |
| **Anarlog** | Memos + generated summaries + transcripts, chat, editable templates, correction at selection/global scope; share generated summary separately from private memo; exports, local CLI/MCP and optional sync. | Local SQLite/files; cloud AI and encrypted sync are opt-in. Free unlimited local/BYOK path; optional managed cloud/commercial enterprise, exact current paid tariff not verified. Deleting audio preserves text. [H1][][H2][][H4][] |
| **Char (current)** | Context across tools, persistent wiki/memory, drafts/action delegation with a user approval step described; not a verified shipped meeting benchmark. | Private beta; future paid SaaS stated in split announcement. Exact retention, platform, pricing and security controls unknown. [H2][][H3][] |
| **Limitless** | Wearable conversation memory, transcript/summaries/search and dictionary; Saved Voices. Not a current live-coaching alternative. | Updated privacy terms needed to continue; export/deletion documented; several regions discontinued with deletion deadlines. Continuing customers free Unlimited; no new Pendant sales. Long-term service beyond 2026 not guaranteed by current notice. [L1][][L2][][L3][] |
| **tl;dv** | Postcall timestamped notes, templates, multi-meeting Ask/reports, coaching/playbooks, clips/sharing and CRM automation/MCP. Live latency/coaching parity with Cluely not independently established. | Policy: free audio/video/transcripts three months; paid until account deletion. Primarily EEA hosting; selected AI processing location can be EU/US; no general-purpose model training. Free → Pro → Business per-seat tiers verified on pricing page; feature/AI/storage/automation limits vary. Unlimited recordings does not mean unlimited retained storage/AI. [T1][][T2][][T3][] |
| **Read.ai** | Live notes/transcription/metrics, summaries/actions, topic readouts, enterprise search across meetings/email/messages, coaching, integrations/MCP; general custom template depth unknown. | Training only with explicit opt-in. Data-policy default AWS US; pricing also offers EU/CA hosting on Enterprise+—contract/tenant-specific, not a universal US-only assertion. Free text ≤2 years, recordings 180 days; paid until deleted/custom retention; transcript storage can be disabled. Free five transcripts/month → Pro → Enterprise → Enterprise+. [R1][][R2][][R3][][R4][][R5][] |
| **Zoom** | In-meeting Q&A; summary/next steps and smart recordings. My Notes combines personal notes with transcript, Ask AI, durable account context. Custom templates/connectors and cross-content chat depend on deployment/entitlement. | My Notes retains no processing audio after transcript completion, but notes/transcripts persist under **separate** retention settings; Meeting Summary ZDR does not apply to My Notes. Core AI in eligible paid Zoom plans; advanced offerings/credits/add-ons vary. Native recording retention is another feature, not equivalent to My Notes. [Z1][][Z2][][Z3][][Z4][][Z6][] |
| **Teams** | Copilot during/after Q&A; recap notes/tasks, speaker timeline, mentions, join/leave/screenshare markers, topics/chapters, customizable recap summaries; Microsoft 365 provides broader authorized work context. | Inherits tenant compliance/retention; recap artifacts span Exchange/OneDrive/SharePoint; no training/testing on customer recap content. Teams base license + Premium or Copilot; Copilot needed for some experiences, including recap without saving transcripts. Not identical entitlements. [MS1][][MS2][][MS4][][MS5][] |
| **Google Meet** | Live “Summary so far,” postcall Quick/Full notes and next steps; configurable length/sections (some beta), screenshots, automatic Docs/Calendar delivery. Cross-meeting retrieval belongs to broader Drive/Gemini tooling, not proven by the notes feature alone. | Workspace/eligible AI plan and host/admin controls; notes follow configured Meet/Vault retention. Workspace content is not used for generative-model training without customer permission/instruction; existing access controls apply. Recipient set can include invited non-attendees; visibility of a Calendar attachment is not document access. Workspace privacy differs from consumer Gemini terms. [GO1][][GO2][][GO3][][GO4][] |

## 3. Product notes: mechanisms worth copying, limits worth avoiding

### Meetily: inspect the shipped source, not just the “private” headline

The inspected application is Tauri/Rust with a Next.js/React frontend and SQLx/SQLite. Audio code includes mic/system capture, mixing/resampling, optional high-pass/noise suppression/normalization, Silero segmentation, and asynchronous STT dispatch. The engine validates/reuses local Whisper or Parakeet models; Cargo shows `whisper-rs`, `ort`, `silero_rs`, CPAL, Rubato and RNNoise-related `nnnoiseless`. This is not evidence that the historical Python backend directory remains the required production path. [M4][][M5][][M6][]

Silero operates at 16 kHz. The live pipeline uses a 500 ms silence redemption versus 2,000 ms for batch paths; uninterrupted speech still needs special treatment, so “live transcript” is not a guaranteed bounded-latency streaming decoder. These are concrete cheap gates before expensive ASR, not semantic LLM classifiers. Summarizer code separates providers including Ollama, built-in local sidecar and hosted/BYOK routes. [M5][][M6][]

**Diarization qualification:** Community lacks Pro speaker identification; Pro docs describe bundled local models but do not identify their architecture. The observed open source cannot justify claims of pyannote/Sortformer/sherpa usage inside Pro. Review UX is strong: listen to a voice sample, rename, merge duplicates, adjust expected count (Auto or 1–10), rerun labels without changing transcript words, correct a single line. However, saved merges have no unmerge action, retranscription resets line corrections/merges, and existing summaries require regeneration after names change. AudioGraph should adopt the correction affordances and beat the destructive/incomplete propagation behavior through its revision/projection machinery. [M2][][M3][]

There are two public inconsistencies to preserve in evaluation: marketing says live speaker labels but help says after stop; older README/Pro copy says audio stays local while current provider docs explicitly offer audio-upload STT. Record edition, build and selected route in any comparison. [M1][][M2][][M3][][M7][]

### Granola: names can come from interface metadata, not expensive inference

The important technique is active-speaker/participant metadata joined to transcript time. Accessibility reads names/state from the conferencing UI; the Meet extension provides an alternative. Calendar identity adds useful meeting/person context but is not itself acoustic attribution. Without tags, source lanes are Me/Them, and contextual AI naming can be wrong. Room users sharing one device remain unresolved. Adopt both the metadata lane and honest fallback labels, rather than asserting “speaker identification” for every route. [G3][]

Granola’s other advantage is document ergonomics: short private notes guide enhancement, alongside templates, recipes, folder-scoped questions and pre-briefs. AudioGraph should preserve human text and visibly distinguish generated additions. Granola deletes audio, trading reduced storage risk for inability to audit by playback; AudioGraph should offer explicit retention choices rather than claim one universally correct policy. [G1][][G2][][G4][]

### Cluely: separate detecting an opportunity from generating an answer

Dynamic questions/keywords/suggestions are selectable cards; default actions include recap, follow-up and fact-check, with role/mode prompts and knowledge-base context. This supports a product split between a cheap event detector, a useful card and an explicitly requested expensive answer. **It does not disclose what model or classifier performs detection.** Copy the interaction and evaluate routing costs; do not assert Cluely uses a particular small classifier. Its public-summary link is also not equivalent to an ACL-controlled share. “Undetectability” is a screen-sharing feature, not privacy, consent or security. [C1][][C2][][C3][][C4][]

### Plaud: identity enrollment should be narrow and explicit

Plaud separates diarization, self-recognition and naming other people. It explicitly says calendar attendees are a manual naming aid, not automatic matches; other people’s names apply only to the recording. Its self-profile consent, deletion, ≥30-second sample and clear failure cases (overlap, similar voices, short turns, distance/noise) are a better reference than assuming everyone has a voiceprint. Rename propagation to summary is a useful product expectation. Cloud processing remains cloud even when sync is off and server results are deleted. [P2][][P3][]

### Both Yogurts: strongest new local competitors, with different trust boundaries

Frigade Yogurt provides a particularly useful inspectable baseline. `nameFor` time-weights active-speaker observations over a transcript span; samples expire after **2 seconds**; a name must cover **more than half** the line. Exactly two roster entries can resolve the remote name only when one is known as self and there is no conflicting outside-roster speaker. Multiple active speakers, missing permissions/state, or insufficient coverage yield unknown/Them. Readings are bound to call processes and helpers are lower priority than recording/transcription. These are implementation thresholds, **not calibrated accuracy claims or defaults AudioGraph should copy without tests**. [Y3][][Y4][]

Its call detector combines known calling applications using the mic, browser call evidence and nearby calendar events. Stop requires app departure and quiet audio, rather than treating calendar end as proof the meeting ended. This is another example of deterministic state routing that needs no LLM. The source’s explicit stale-evidence and two-person checks are more important than the headline claim of automatic names. [Y5][]

The second Yogurt is not the same app: Rust/localhost UI, partial open-source edition, provider choice, Pro all-time retrieval and self-hosted sync. Its README says default Deepgram sends audio off-device while the website says audio never leaves. Until a selected build/provider is checked, cite the conflict instead of giving it an unconditional all-local rating. [J1][][J2][]

### Otter: voice memory has a scope and can spread through collaboration

Manual voice labels improve future recognition. Workspace member profiles and Channel-shared speakers can be reused within those scopes; unrelated users do not automatically inherit them. Zoom bot participant names give a distinct live pathway; ordinary voice recognition starts after recording/import processing. Enterprise can disable speaker enrollment. AudioGraph should not turn a name correction into silent cross-meeting biometric enrollment or team-wide identity sharing. Record enrollment consent, scope, removal and provenance separately from a session-local label. [O1][][O3][]

### Fireflies: capture-mode capability matrices are essential

The same app offers bot and no-bot capture, but speaker labels, Sales Assist and saved media differ. Both can expose live assist. Calendar-linked recordings may trigger recap distribution according to settings; linking is not merely cosmetic metadata. Copy the visible capability matrix and the contextual preparation/task surfaces, but show recipient/egress consequences before enabling automation. [F1][]

### Fathom: user identity disambiguation and capture consent are product features

Name Picker resolves a recording-user/participant mismatch using known aliases and a person’s selection. The documented host shortcut is a heuristic, not verified acoustic truth. iOS’s optional self-only voiceprint is separately consented from recording, and in-person capture asks for participant count (calendar attendees can prepopulate it). Desktop and mobile do not have feature parity: iOS lacks Ask/search and offline recording; Windows bot-free rollout is limited. The 3.0 setup requires at least an email or chat notice. AudioGraph should benchmark capability by route/platform, not award every Fathom user all advertised features. [FA1][][FA2][][FA3][][FA4][]

### Krisp: enhance audio carefully; do not erase the people to be transcribed

Its local noise cancellation plus cloud transcription contradicts a simplistic local/cloud product label. Voice isolation is useful for a personal microphone but is not meeting-source separation or speaker naming. **Inference:** aggressively applying primary-speaker isolation to a room/mixed-remote stream can suppress legitimate speakers; evaluate separately on self-mic versus mixed-conversation paths. The existing AudioGraph source-separation recommendation already requires proof before derived lanes become ASR input. [K1][][K2][]; [AudioGraph separation research](source-separation-bakeoff-2026-06-26.md).

### Anarlog and current Char: preserve the product distinction

Anarlog’s local SQLite, independently selectable transcription/intelligence, selection-level speaker corrections, audio-only deletion, separate private memo/shared summary, and proposal-diff approval are directly relevant to AudioGraph. The active provider/model is visible. Current Char’s action-delegation thesis is adjacent, not evidence of shipped offline meeting behavior or open-source ownership. Do not apply old Hyprnote platform limits or licensing to the renamed current community app without checking the new source. [H1][][H2][][H3][][H4][]

### Limitless: good enrollment UX, bad assumption of permanent service

The Saved Voices guide exposes quality gates: minimum labeled duration, recent bounded examples, clean speech, distance/no-overlap guidance, reset/delete a contaminated profile, and channel-condition differences. Do not automatically create separate durable people because their phone and in-person embeddings differ; AudioGraph can model multiple acoustic exemplars for one confirmed person. That last sentence is a recommendation, not a Limitless feature. Exportable local artifacts matter because the official service transition demonstrates that hardware ownership is not service continuity. [L1][][L2][]

### tl;dv and Read: memory and workflow competitors, not just recorders

Compare tl;dv on cross-meeting reports, template/playbook extraction and timestamped evidence, not unverified “99%” accuracy marketing. Its current homepage advertises bot-free and mobile capture, while its privacy policy identifies cloud STT/LLM processors and explicit retention. Read adds cross-tool enterprise search and live metrics; hybrid room speakers can be renamed per turn or globally. Read’s local capture cannot receive the agent’s stop/opt-out chat commands. Those controls must not be presented as universal across modes. [T1][][T2][][T3][][R1][][R2][][R3][][R4][][R5][]

### Platform-native assistants: metadata and governance are structural advantages

Teams already offers a speaker seek timeline, mention markers and join/leave/screenshare context, so a timeline alone is not novel. Its recap documentation explicitly says only notes/tasks in that recap implementation use GPT; marker construction uses transcript/display-name/attendance/recording data. The privacy article is older than current transcript-free recap support; prefer the current overview for eligibility. [MS1][][MS2][]

Zoom My Notes now crosses platform boundaries: native Zoom uses routed meeting audio, third-party/in-person capture uses device audio, and cloud ASR handles both. Muting Teams does not necessarily stop My Notes’ direct mic capture. My Notes has its own retention controls and is not governed by Meeting Summary ZDR. Smart video name tags require careful distinction from acoustic voice identity. [Z1][][Z4][][Z5][]

Google’s ready-to-use Docs/Calendar artifact and “Summary so far” reduce workflow overhead. Its sharing set can include everyone invited, not just attendees. One-language-at-a-time support and eight documented note languages are concrete constraints to test; broad platform availability does not establish code-switching support. The in-person feature does not establish reliable N-person room identity. [GO1][][GO2][]

## 4. Speaker-identification techniques for the sibling design

This is the complete technique inventory established by the inspected evidence; unpublished vendor internals remain unknown.

| Technique and observed products | What it proves / does not prove | AudioGraph design input |
|---|---|---|
| **Source-role separation**: Granola Me/Them; Yogurt remote versus self; Meetily fallback | Mic/system direction, not one identity per remote person. Echo/room pickup can violate self assumptions. [G3][][Y3][][M2][] | Preserve source id/role separately from speaker/person. Never synthesize N clean audio channels from diarization. |
| **Platform participant + active-speaker time join**: Granola, Frigade Yogurt; Otter Zoom bot; native platforms | Display name and observed activity, not legal identity, persistent unique person or overlap separation. Stale UI, duplicate names and room devices remain risks. [G3][][Y3][][O1][] | Add timestamped evidence with acquisition method, freshness/latency, platform session/participant id and unknown outcome. Keep metadata collection and timing in Rust, not React. |
| **Roster + self identity, aliases and explicit picker**: Yogurt two-person rule; Fathom Name Picker | Resolves a constrained candidate set; host/name equality is still a heuristic. Calendar invitees may not attend. [Y3][][FA3][] | Deterministic alias/participant matching first; expose unresolved self selection. Do not merge graph people solely by similar display names. |
| **Calendar candidates/context**: Plaud manual assignment; Fathom participant count; Granola people/context | Useful candidates and priors, not proof of which voice said a line. [P2][][FA4][][G3][][G4][] | Maintain attended versus invited versus inferred; attach candidate sets to existing identity-ladder work rather than auto-name every cluster. |
| **Within-recording acoustic clustering and review**: Meetily Pro/Plaud, Read room labels | Distinguishes voices without knowing names. Count hints do not guarantee detected count. [M2][][P2][][R4][] | Existing `diarization/{worker,clustering,stabilize}` and `audio-graph-3588`/`5011` already own this; add identity on top, not replacement clustering. |
| **Manual listen/rename/merge/per-turn correction**: Meetily; selection/global assignment Anarlog; Read; Otter tagging | Human confirmation; must declare whether correction is session-local or teaches future voice recognition. [M2][][H4][][R4][][O1][] | Persist evidence-backed remaps; replay through `speech_span_revision.rs`, `projections.rs`, `graph/temporal.rs`; close notes propagation via `audio-graph-262a`. Keep undo/basis linkage. |
| **Opt-in self voice enrollment**: Plaud ≥30 seconds; Fathom iOS self only | Explicitly limited recognition scope; recording consent does not automatically consent to biometric enrollment. [P2][][FA4][] | Start with self-only optional local exemplars if approved; profile deletion, retention, quality, device/channel variability and explicit enrollment scope required. |
| **Reusable labeled voices**: Otter account/Workspace/Channel; Limitless Saved Voices | Cross-session recognition can improve convenience; labeling contamination and sharing expand harm scope. [O1][][O3][][L2][] | Separate stable person id from voice profile/version and session cluster; retain multiple confirmed exemplars, conservative unknown/abstain and revocation. Never silently train from a guessed name. |
| **Tenant-controlled room voice recognition**: Teams | Opt-in active/express enrollment, per-tenant retrieval, deletion, inactivity policy, administrator enablement; not a universal no-enrollment room solution. [MS3][] | Useful governance checklist; do not import a cloud biometric system merely to beat room naming. |
| **Face enrollment/manual room name tags**: Zoom Rooms | Visual identity/context, not proof the same person produced a particular audio span. [Z5][] | Optional future evidence type, not default camera capture or substitute for speaker provenance. |
| **Contextual name inference**: Granola fallback | Can use introductions and conversation context; does not make a generated guess observed participant metadata. [G3][] | Fits ratified `audio-graph-9509` explicit → implicit → post-session ladder; tag inferred/proposed versus confirmed, correctable retroactively. |

Recommended evidence precedence is **an AudioGraph design inference**, not an industry standard: user-confirmed binding and well-scoped platform observations; then consented voice-profile matches with confidence/quality; then explicit conversational naming evidence; then context-generated proposals. Conflicts must remain visible, not be resolved by an undocumented global confidence score. Self-introduction can name the introducer; third-party introductions do not necessarily name the person currently speaking. Existing `audio-graph-9509` requires design review before implementation tickets, so this report does not reopen its ratified ladder or claim design approval.

Mandatory evaluation cases: same display name twice; one person renamed mid-call; shared room participant; overlapping speech; one-word speaker; late join/leave; stale UI; permissions revoked; screen/audio lag; self-mic echo; phone versus room voice; bad profile enrollment; absent calendar invitee; transcript revision crossing a turn; a correction that changes task owner and graph relations. Score **false real-name attribution and abstention coverage**, not only DER or generic speaker-count accuracy.

## 5. Replacing LLM calls: observed techniques versus proposed classifiers

**No inspected competitor source establishes use of Laya, D3, Jev or Clef for meeting semantic routing.** Closed-product live cards, topic detection and scorecards reveal task boundaries, not their internal model or cost. Do not report those as proven classifier optimizations.

### Proven cheaper mechanisms to adopt before semantic models

1. **Metadata/time arithmetic instead of generated names.** Granola UI speaker tags and Yogurt’s expiring majority-overlap join provide names without asking a generative model to infer every turn. Use normalized ids, freshness and ambiguity checks; leave uncertain spans unnamed. [G3][][Y3][]
2. **Process/calendar/audio-state rules instead of “is this a meeting?” prompts.** Yogurt uses mic ownership, calling-app/browser evidence, event matching and quiet/departure state. Zoom My Notes also offers a mic-activity prompt. Calendar alone is not proof of capture start/end. [Y5][][Z1][]
3. **VAD before STT; signal quality before semantics.** Meetily’s Silero path prevents silence going to Whisper. Krisp local noise handling is a separate stage; evaluate its effect on all-speaker recall rather than treating it as language intelligence. [M5][][K2][]
4. **Alias/picker/count hints rather than unconstrained identity generation.** Fathom’s alias lookup and explicit Name Picker, Plaud’s attendee picker, and Meetily’s expected speaker count are bounded operations. They do not need a prose LLM response. [FA3][][P2][][M2][]
5. **Compute timeline markers from evidence.** Teams constructs mention/attendance/speaker/screenshare markers from existing records; its recap privacy doc limits GPT use to generated notes/tasks. AudioGraph should not spend a generator call to discover a join/leave event it already recorded. [MS2][]
6. **Choose scope before answering.** Yogurt/Granola support meeting/folder/library scopes; tl;dv supports cross-meeting reports. Scope filters and source retrieval can reduce generation context, although their internal ranking/model costs are not public. “Use a cheap retriever and classifier first” is our implementation inference, not a disclosed vendor pipeline. [Y1][][G1][][T1][]
7. **Use explicit templates and user focus to bound output.** Granola, Fathom and Meetily expose user notes/templates; Plaud exposes structured formats. A user-selected template is a deterministic decision; auto-template choice is a possible classifier task. Actual summary composition still needs generation. [G1][][FA1][][M3][][P4][]

### Candidate System-1 questions, in priority order

These are **proposals for existing AudioGraph work**, not shipped competitor mechanisms or a claim that classifiers can replace free-form synthesis.

| Decision | Cheap input/output | Integration and safety boundary |
|---|---|---|
| Opportunity/card kind and whether a question is answerable | Recent canonical span + bounded context → existing `choice` kind and `noul` answerability | Existing `src-tauri/src/decision/`, `speech/mod.rs`, `commands.rs`; `audio-graph-0ac0`, `6dfb`, `f284`. Shadow already exists; do not add a second detector or prematurely change spend authority. |
| Whether retrieval is needed; scope/template selection | User request + explicit active scope → bounded route/template option, including unknown | Use deterministic explicit selections first. Calibrate new schemas separately; never turn a classification into permission to search private folders or upload context. |
| Duplicate/superseded live opportunity | Existing card ids + canonical revision/basis → keep/update/dismiss candidate | Identity/revision checks first; classifier only for semantic ambiguity. Measure missed unique questions and stale-card false positives. |
| Speaker-name evidence type and candidate ranking | Pre-extracted introduction/attendee/platform evidence → self-ID / introduction / mention / none; known candidate / unknown | `audio-graph-9509` and speaker revisions. Candidate ranking is not voice recognition or authority to create a person; generator may extract a proposed name, deterministic validation/user confirmation governs binding. |
| Claim support / entity candidate / relation category | Bounded source excerpt + existing candidates → score/choice | `claim_evidence`/graph and ADR-0049 projection family. Validators remain the only admission authority. Do not replace quote/basis validation with “high confidence.” |
| Whether a recurring thread deserves a pre-brief | Existing unresolved, dated graph facts → include/exclude/rank | `audio-graph-53cf`; novelty/relevance can be classified, briefing prose remains generated and source-cited. |

The existing contract already supports `choice`, `noul` and `score`; native Laya/D3/Clef and HTTP routes must not be reintroduced as new architecture. Native parity and latency do **not** establish task accuracy. For example, the published local d3-edge short-request CUDA p50 is about 6.3 ms on the reference RTX 5090, but CPU p95 is about 400 ms and longer CPU contexts are slower. Existing Jev replay is twelve synthetic spans and heuristic agreement, not an independent semantic benchmark. Clef’s design records a post-fix parity rerun as outstanding. Keep per-route/model/schema calibration, abstention, model pins, egress checks and manual Ask behavior. [AudioGraph contract](../../src-tauri/crates/decision-contract/src/lib.rs), [route ADR](../adr/0049-decision-route-layer.md), [evaluation](system-one-eval-2026-10-18.md), [D3 latency](d3-native-latency-2026-10-18.md), [native designs](../designs/d3-native-runtime-2026-10-18.md), [Clef status](../designs/clef-flash-native-2026-10-18.md).

Measure total work per meeting: decision calls, generation calls avoided, context/token savings, retrieval cost, CPU/GPU/RAM/energy, cold/warm p50/p95, false automatic answers, missed useful cards, calibration/coverage and recovery after transcript retcons. Zero API fee is not zero compute cost. Existing `audio-graph-f284` owns held-out labeled decisions; extend it rather than create a competing dataset.

## 6. Ranked AudioGraph capability gaps and owners

These are **prioritization judgments**, not hands-on competitive scores. Existing open Seeds establish unfinished scope; source inspection establishes particular seams. Do not describe the app as lacking a whole subsystem merely because its end-to-end product gate remains open.

| Rank | Capability to close or demonstrate | Competitor bar and concrete AudioGraph seam / existing Seed |
|---:|---|---|
| **1** | Accurate real names, transparent unknowns and correction propagation | Granola/Yogurt metadata names; Plaud/Fathom enrollment boundaries; Meetily correction UX. Existing `audio-graph-9509`, `9c45`, `3588`, `5011`, `1fbd`; `src-tauri/src/diarization/{worker,clustering,stabilize}.rs`, `speech_span_revision.rs`, `projections.rs`, `graph/temporal.rs`. `262a` already tracks notes remap propagation; do not duplicate it. |
| **2** | Reliable meeting lifecycle with minimal setup and honest mode capabilities | Fireflies bot/no-bot matrix; Yogurt reminders/stop rules; Fathom rollout/mode notices. `src-tauri/src/audio/`, `sessions/mod.rs`, `commands.rs`, provider registry; ready/live/review shell `audio-graph-19c7`. Compare permission denial, headset change and long-call recovery, not just first successful capture. |
| **3** | Cited cross-meeting memory and useful pre-meeting brief | Granola folder chat/briefs, Yogurt local sourced Ask, tl;dv reports and Read cross-tool search. Existing graph/session/projection stores, `graph/temporal.rs`, `sessions/mod.rs`, `persistence/`; `audio-graph-53cf` and benchmark `8181`. Demonstrate source jumps, contradictions, corrected identities and access scope. |
| **4** | Useful low-noise live assist, not a stream of expensive answers | Cluely cards + action types; Fireflies live pane; Fathom live summary. `speech/mod.rs`, `commands.rs`, `src/decision/`, `decision-contract`, `decision-engine`; `audio-graph-0ac0`/`6dfb` and `f284`. Product gap is calibrated authority/quality, not absence of the landed decision layer. |
| **5** | Human-authored note preservation, templates and repeatable outputs | Granola enhancement, Fathom scratchpad, Anarlog memo/summary separation, Meetily/Plaud templates. Existing notes/projection path `src-tauri/src/projections.rs`, `speech/mod.rs`; playbook Seed `audio-graph-9284`, shell `19c7`. Corrections and regeneration must not overwrite user edits or detach evidence. |
| **6** | Understandable retention, egress and consent by data class | Granola/Plaud/Krisp separate processing audio from retained media; Zoom My Notes has distinct policy. `persistence/`, session artifacts/data movement, provider registry and [content-egress checklist](../designs/provider-architecture.md#provider-addition-content-egress-checklist); `audio-graph-c282`, `2d22`, `be7c`, `70a3`, `51e0`. Local policy enforcement should not be a premium-only trust promise. |
| **7** | Safe sharing/export/integrations without private-memo leakage | Anarlog shares summary separately; Granola controlled folders; Cluely public links are a lower privacy bar. Existing `audio-graph-1971`, `617e`, `8d35`, artifact manifest/export path in `persistence/mod.rs` and `sessions/mod.rs`. Sharing remains later/deferred under its existing Seed; do not silently reprioritize that owner decision. |
| **8** | In-person/mobile continuity without cloud dependency or platform overclaims | Plaud hardware, Fathom iOS, Granola mobile, Limitless portability lesson. Desktop import/artifact/export foundations first; provider capability contracts distinguish OS/device paths. Hardware/mobile expansion is not a reason to postpone correct desktop capture or create a new unscoped epic. |

Repository observations supporting that mapping: the rolling diarization worker computes local clusters/embeddings and maps them to global ids before emitting trailing-hop stable spans; stabilization uses deterministic cosine/cannot-link assignment. `graph/temporal.rs::resolve_entity` is name/fuzzy matching, not verified human identity. `speech_span_revision.rs` validates semantic speaker/channel/timing combinations. These support adding evidence-backed identity joins and corrections, not replacing the whole pipeline. See [worker](../../src-tauri/src/diarization/worker.rs), [stabilizer](../../src-tauri/src/diarization/stabilize.rs), [temporal graph](../../src-tauri/src/graph/temporal.rs), [semantic revisions](../../src-tauri/src/speech_span_revision.rs), [channel routing decision](speaker-channel-routing-2026-06-26.md), and [existing local diarization research](sherpa-diarization-live-2026-05.md).

## 7. Where AudioGraph can be structurally better

1. **Local-first with evidence of the boundary.** Meetily/Yogurt/Anarlog mean locality is no longer unique. Differentiate by showing the actual route, data class, execution location and retention, with blocked-policy fixtures proving no write. No silent local-to-cloud fallback, including for classifiers or MCP consumers. This follows existing provider contracts, not a new frontend networking layer. [M7][][Y1][][H1][]; [AudioGraph checklist](../designs/provider-architecture.md#provider-addition-content-egress-checklist).
2. **Correctable temporal knowledge, not frozen summaries.** Meetily requires summary regeneration after renaming and loses some edits on retranscription; Limitless voice labels are forward-only. AudioGraph’s basis/revision/supersession machinery can propagate a correction through transcript, notes, task ownership and relations while preserving human edits and the historical explanation. That is a design opportunity until demonstrated end to end, especially `audio-graph-262a`. [M2][][L2][]; [projection contracts](../../src-tauri/src/projections.rs).
3. **Speaker timeline plus relationship timeline.** Teams already has who-spoke-when. The additional bar is “who said what, what relation/decision it supports, what was believed at time t, and why it changed.” Build on `audio-graph-0d72` rather than claiming a seekable diarization strip is unique. [MS1][]; [timeline ADR](../adr/0026-session-timeline-who-said-what-when.md).
4. **System-1 decides whether to spend; System-2 writes.** Metadata and deterministic checks should eliminate avoidable calls before calibrated classifiers. Keep generators for actual composition/extraction beyond a bounded label set. Native routes can reduce egress and generation cost, but only if measured on held-out user-relevant examples and admitted under existing rules; do not sell shadow agreement as answer quality. [MS2][][Y3][]; [ADR-0049](../adr/0049-decision-route-layer.md).
5. **Portability and scope survive vendor changes.** Local export/import with full artifact provenance beats a hardware/service that can stop taking new customers or change region coverage. Scoped assistant access should explain that a local read tool can still disclose to an external AI. [L1][][Y1][]; existing `audio-graph-be7c`, `617e`, `8d35`, `1971`.

## 8. Applied Seed refreshes and remaining proposals

This PR applied the research refreshes to `audio-graph-b153` and `audio-graph-8181` as
`refresh_2026_11_21` extensions, linking this report without closing either Seed. The b153 extension
records the competitive findings and ranked gaps; its remaining work is the route/platform
capability matrix and evidence-linked demos. The 8181 extension records added cohorts and metric
families, not an executed benchmark. The detailed acceptance/method refinements below remain
proposals where not captured by those extensions; implementation, demos and benchmark execution
are still open.

### `audio-graph-b153` — refresh the roadmap’s competitive baseline

- Keep the existing dependency-first/deferred-roadmap intent and child ownership. Link this report; update the comparison date and include Meetily Community/Pro, both Yogurts, Anarlog/current Char split, current bot-free entrants and Limitless transition.
- Replace “bot-free + local-first” as the primary differentiator with **proven capture → named/unknown speaker evidence → correctable notes/temporal memory → useful live assistance**.
- Reuse `9509`/`9c45`/`3588`/`1fbd`/`262a` for identity, `53cf` for briefs, `9284` for playbooks, `1971` for sharing, `c282`/`2d22`/`be7c` for retention/artifacts, `6dfb`/`f284` for decision quality. Do not create parallel architecture epics.
- Acceptance addition: a route/platform capability matrix and evidence-linked demonstration for the top five gaps, with unknowns, consent and corrections visible. Explicitly forbid claims of “better than X” without the benchmark evidence below.

### `audio-graph-8181` — stratify the benchmark by capability and trust boundary

Retain existing use-case fixtures and quality dimensions; add:

- **Cohorts:** open/local (Meetily Community, applicable Pro features, Frigade Yogurt, jarvisrchen Yogurt, Anarlog), hosted bot, hosted bot-free, hardware/in-person, platform-native. Archive exact edition/build/date, OS, selected providers and licensing; mark unavailable/retired/rollout-ineligible cells N/A rather than zero.
- **Speaker evaluation:** DER/speaker-attributed WER plus real-name false assignment, unknown coverage, voice-profile false accept/reject, identity time-to-stability, turn-boundary correctness, correction propagation/undo and invited-versus-attended confusion. Separate anonymous diarization from naming and cross-session re-identification.
- **Notes/memory:** preserve human text, action owner/deadline precision and recall, decision coverage, evidence citation support and jump correctness, contradictory/retconned facts, missing-answer abstention and cross-folder privacy. Distinguish stale extraction from hallucination.
- **Live assist:** question/card precision/recall, time-to-card/answer, interruption burden, duplicate cards, automatic spend mistakes, generation calls avoided versus baseline, local CPU/GPU/energy and cloud tokens/cost. Separate opportunity detection from answer quality.
- **Privacy/lifecycle:** no-content blocked-egress evidence; no automatic cloud fallback; mute/pause/stop semantics; notice/consent behavior; retained versus ephemeral audio; delete/export/profile removal propagation; MCP disclosure warning. No private fixtures uploaded without explicit authorization.
- **Method:** consented/licensed identical audio or controlled live replay; human reference labels with session/source-separated train/calibration/test partitions; blinded note assessment; public reproducible metric definitions and aggregate results. Record missing access and model identity. Vendor “accuracy” marketing and heuristic agreement are not ground truth.

Acceptance addition: produce an evidence-backed baseline report for reachable cohorts, document why each unavailable product/mode was not exercised, and deliver machine-readable aggregate metrics with pinned configs. Research alone does not close the execution acceptance of `8181`.

## 9. Uncertainties and remaining risks

- The two Yogurt products are verified; intended identity is not. Both are named explicitly rather than guessing.
- Meetily Pro’s diarization architecture is not public in the inspected Community source. Marketing/help timing and cloud-STT privacy wording conflict. The second Yogurt also has conflicting website/default-provider claims. These are **research findings**, not reproduced security defects.
- Current Char is private beta. Source-visible Anarlog enterprise components are not all MIT; its moving README and historical pages should not be treated as one immutable product release.
- Closed-product STT/LLM model pins, classifier architecture, room-speaker matching, actual retention enforcement and deletion from backups are mostly not independently testable here. No generic SOC 2/GDPR/HIPAA statement establishes every plan/workflow or region.
- Read’s hosting policy and pricing imply tenant-specific regional options. Teams privacy documentation lags newer recap modes. Zoom’s whitepaper explicitly warns that documented features depend on availability, licenses, admins and deployment.
- No latency/quality leaderboard, price-performance win, legal compliance certification, competitor installation result or adversarial privacy test is claimed. This is public evidence plus design implications. Builds/tests/linters were not run: the assigned change is research-only and integration verification belongs to the owner.

## Primary-source index

Each bracketed citation links to an individual source below; former ranges are expanded into separate source links.

### Meetily

- [M1][] [Community repository and README](https://github.com/Zackriya-Solutions/meetily/tree/2166bc6fe3ba585580f3f94b79cda7e7b91deb76).
- [M2][] [Speaker diarization help: timing, correction, naming, audio and summary limits](https://docs.meetily.ai/features/speaker-diarization).
- [M3][] [Pro edition/licensing/platform/features](https://meetily.ai/pro/).
- [M4][] [Rust dependencies](https://github.com/Zackriya-Solutions/meetily/blob/2166bc6fe3ba585580f3f94b79cda7e7b91deb76/frontend/src-tauri/Cargo.toml) and [frontend dependencies](https://github.com/Zackriya-Solutions/meetily/blob/2166bc6fe3ba585580f3f94b79cda7e7b91deb76/frontend/package.json).
- [M5][] [Audio pipeline](https://github.com/Zackriya-Solutions/meetily/blob/2166bc6fe3ba585580f3f94b79cda7e7b91deb76/frontend/src-tauri/src/audio/pipeline.rs) and [Silero VAD](https://github.com/Zackriya-Solutions/meetily/blob/2166bc6fe3ba585580f3f94b79cda7e7b91deb76/frontend/src-tauri/src/audio/vad.rs).
- [M6][] [Transcription initialization](https://github.com/Zackriya-Solutions/meetily/blob/2166bc6fe3ba585580f3f94b79cda7e7b91deb76/frontend/src-tauri/src/audio/transcription/engine.rs) and [LLM client](https://github.com/Zackriya-Solutions/meetily/blob/2166bc6fe3ba585580f3f94b79cda7e7b91deb76/frontend/src-tauri/src/summary/llm_client.rs).
- [M7][] [Provider-by-provider audio destinations](https://docs.meetily.ai/features/transcription-providers).

### Granola and Cluely

- [G1][] [Capture/cloud/storage boundaries](https://www.granola.ai/blog/local-first-ai-notetaker-vs-cloud).
- [G2][] [Security, retention and training](https://www.granola.ai/security).
- [G3][] [Speaker tags and their limits](https://docs.granola.ai/help-center/taking-notes/speaker-attribution).
- [G4][] [First-party feature documentation index](https://docs.granola.ai/llms.txt), including templates, pre-briefs, recipes, chat, sharing and people directory.
- [C1][] [Live insights/cards/actions](https://docs.cluely.com/feature/liveinsights).
- [C2][] [Postcall notes/sharing](https://docs.cluely.com/feature/postcall).
- [C3][] [Privacy/retention](https://cluely.com/privacy-policy).
- [C4][] [Current tiers and memory access](https://cluely.com/pricing) and [audio capture documentation](https://docs.cluely.com/debugging/audio).

### Plaud and both Yogurts

- [P1][] [Plaud Note hardware](https://www.plaud.ai/products/plaud-note-ai-voice-recorder).
- [P2][] [Voice enrollment and explicit naming limitations](https://support.plaud.ai/hc/en-us/articles/63429907861657-Auto-speaker-labeling-for-v4-0-x-or-later).
- [P3][] [Post-processing deletion versus cloud sync](https://support.plaud.ai/hc/en-us/articles/49922008621337-How-is-my-data-handled-after-transcription).
- [P4][] [Membership features, templates, Ask and quotas](https://support.plaud.ai/hc/en-us/articles/50742099278617-Comparison).
- [Y1][] [Frigade Yogurt README, privacy and capabilities](https://github.com/FrigadeHQ/yogurt/tree/e1398289ab2bad7aba75ad5501ae16bbce9ac81a).
- [Y2][] [Product site](https://www.yogurt.so/) and [desktop stack](https://github.com/FrigadeHQ/yogurt/blob/e1398289ab2bad7aba75ad5501ae16bbce9ac81a/app/package.json).
- [Y3][] [Exact speaker-attribution rules](https://github.com/FrigadeHQ/yogurt/blob/e1398289ab2bad7aba75ad5501ae16bbce9ac81a/app/src/main/callNames/attribute.ts).
- [Y4][] [Native call-name watcher](https://github.com/FrigadeHQ/yogurt/blob/e1398289ab2bad7aba75ad5501ae16bbce9ac81a/app/src/main/callNames/watch.ts).
- [Y5][] [Meeting detection/end rules](https://github.com/FrigadeHQ/yogurt/blob/e1398289ab2bad7aba75ad5501ae16bbce9ac81a/app/src/main/detect.ts).
- [J1][] [jarvisrchen Yogurt website](https://yogurt-ai.dev/).
- [J2][] [Separate free-source repository, Pro exclusions and default Deepgram path](https://github.com/jarvisrchen/yogurt-open).

### Otter, Fireflies, Fathom and Krisp

- [O1][] [Speaker learning, scope, live Zoom identity and limitations](https://help.otter.ai/hc/en-us/articles/21665587209367-Speaker-Identification-Overview).
- [O2][] [Pricing/capability reference](https://otter.ai/pricing.md).
- [O3][] [Disable speaker learning](https://help.otter.ai/hc/en-us/articles/40643231903639-Disable-Speaker-Learning).
- [O4][] [Custom data retention](https://help.otter.ai/hc/en-us/articles/19500988656279).
- [F1][] [Desktop modes, capability matrix, live pane and briefs](https://guide.fireflies.ai/articles/1208704416-getting-started-with-the-fireflies-desktop-app).
- [F2][] [Security/storage/retention controls](https://fireflies.ai/security).
- [F3][] [Pricing and usage packaging](https://fireflies.ai/pricing).
- [FA1][] [Fathom 3.0 rollout, capture modes, live features and mandatory notice](https://help.fathom.video/en/articles/11577345-how-does-bot-free-recording-work-in-fathom-3-0).
- [FA2][] [Google Meet mode/platform requirements](https://help.fathom.video/en/articles/449472-using-fathom-with-google-meet).
- [FA3][] [Name Picker and aliases](https://help.fathom.video/en/articles/297152).
- [FA4][] [iOS self voiceprint, participant count and limitations](https://help.fathom.video/en/articles/13926529).
- [FA5][] [Retention policies](https://help.fathom.video/en/articles/6057089) and [indefinite recording storage](https://help.fathom.video/en/articles/296448).
- [FA6][] [Current plan page](https://fathom.video/pricing).
- [K1][] [Meeting assistant overview](https://help.krisp.ai/hc/en-us/articles/8214720684956-AI-Meeting-Assistant-overview).
- [K2][] [Local noise cancellation versus cloud STT/recording](https://help.krisp.ai/hc/en-us/articles/360012035500-What-data-Krisp-sends-to-the-cloud).
- [K3][] [Current pricing and feature packaging](https://krisp.ai/pricing/).

### Anarlog/Char, Limitless, tl;dv and Read

- [H1][] [Anarlog repository: architecture, route choices and license boundary](https://github.com/fastrepl/anarlog).
- [H2][] [Name history/product split and Windows/Linux beta update](https://anarlog.so/blog/char-is-now-anarlog/).
- [H3][] [Current separate Char product](https://char.com/).
- [H4][] [Notes, correction, templates, sharing and pending edit review](https://docs.anarlog.so/notes).
- [L1][] [Acquisition/service transition and regions](https://www.limitless.ai/).
- [L2][] [Saved Voices thresholds and caveats](https://help.limitless.ai/en/articles/11501491-troubleshoot-improve-transcripts-summaries-saved-voices).
- [L3][] [Participant information/processing notice](https://help.limitless.ai/en/articles/13004190-talking-to-someone-wearing-the-pendant-what-to-expect-and-how-we-handle-your-information).
- [T1][] [tl;dv current capture/memory/workflow offering](https://tldv.io/).
- [T2][] [Pricing](https://tldv.io/app/pricing).
- [T3][] [Retention, subprocessors, regions and training policy](https://tldv.io/privacy/).
- [R1][] [Read agent versus desktop/mobile consent and attribution](https://support.read.ai/hc/en-us/articles/52118970003987-How-Consent-Works-on-Web-Mode-vs-Desktop-Mode).
- [R2][] [Data usage, training and retention](https://support.read.ai/hc/en-us/articles/52055801266579-What-Does-Read-AI-Do-With-My-Data).
- [R3][] [Current plans and features](https://www.read.ai/plans-pricing).
- [R4][] [Speaker/turn correction](https://support.read.ai/hc/en-us/articles/36169538719379-Can-I-edit-text-and-speaker-names-in-my-meeting-report).
- [R5][] [Read MCP capability](https://support.read.ai/hc/en-us/articles/49381158409491-MCP-Server).

### Platform-native assistants

- [Z1][] [Zoom My Notes capture, ASR, retention, controls and limitations](https://library.zoom.com/ai/ai-whitepaper/my-notes).
- [Z2][] [AI whitepaper and availability disclaimer](https://library.zoom.com/ai/ai-whitepaper).
- [Z3][] [Feature/deployment availability](https://library.zoom.com/ai/ai-whitepaper/feature-availability).
- [Z4][] [AI Companion/product pricing entry point](https://zoom.com/ai-companion).
- [Z5][] [Smart video name tags](https://news.zoom.com/zoom-smart-name-tags) and [room requirements](https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0075270).
- [Z6][] [AI Companion → ZoomMate product change](https://www.zoom.com/en/blog/zoom-ai-companion-zoommate).
- [MS1][] [Current Intelligent Recap features and licensing](https://learn.microsoft.com/en-us/microsoftteams/intelligent-recap-calls-meetings).
- [MS2][] [Recap data/model split, storage and retention](https://learn.microsoft.com/en-us/microsoftteams/privacy/intelligent-recap).
- [MS3][] [Voice/face enrollment, opt-in, tenant scope, deletion and retention](https://learn.microsoft.com/en-us/microsoftteams/rooms/voice-and-face-recognition).
- [MS4][] [Copilot in and after meetings](https://support.microsoft.com/en-us/teams/copilot/catch-up-on-meetings-with-microsoft-365-copilot-in-teams).
- [MS5][] [Custom recap templates](https://support.microsoft.com/en-us/teams/meetings/customize-recap-summaries-in-microsoft-teams).
- [GO1][] [Meet Gemini notes, language, sharing and retention](https://support.google.com/meet/answer/14754931).
- [GO2][] [In-person notes](https://support.google.com/meet/answer/17020724).
- [GO3][] [Workspace Gemini eligibility](https://support.google.com/meet/answer/13952129).
- [GO4][] [Workspace generative AI privacy hub](https://knowledge.workspace.google.com/admin/generative-ai/generative-ai-in-google-workspace-privacy-hub).

[M1]: https://github.com/Zackriya-Solutions/meetily/tree/2166bc6fe3ba585580f3f94b79cda7e7b91deb76
[M2]: https://docs.meetily.ai/features/speaker-diarization
[M3]: https://meetily.ai/pro/
[M4]: https://github.com/Zackriya-Solutions/meetily/blob/2166bc6fe3ba585580f3f94b79cda7e7b91deb76/frontend/src-tauri/Cargo.toml
[M5]: https://github.com/Zackriya-Solutions/meetily/blob/2166bc6fe3ba585580f3f94b79cda7e7b91deb76/frontend/src-tauri/src/audio/pipeline.rs
[M6]: https://github.com/Zackriya-Solutions/meetily/blob/2166bc6fe3ba585580f3f94b79cda7e7b91deb76/frontend/src-tauri/src/audio/transcription/engine.rs
[M7]: https://docs.meetily.ai/features/transcription-providers
[G1]: https://www.granola.ai/blog/local-first-ai-notetaker-vs-cloud
[G2]: https://www.granola.ai/security
[G3]: https://docs.granola.ai/help-center/taking-notes/speaker-attribution
[G4]: https://docs.granola.ai/llms.txt
[C1]: https://docs.cluely.com/feature/liveinsights
[C2]: https://docs.cluely.com/feature/postcall
[C3]: https://cluely.com/privacy-policy
[C4]: https://cluely.com/pricing
[P1]: https://www.plaud.ai/products/plaud-note-ai-voice-recorder
[P2]: https://support.plaud.ai/hc/en-us/articles/63429907861657-Auto-speaker-labeling-for-v4-0-x-or-later
[P3]: https://support.plaud.ai/hc/en-us/articles/49922008621337-How-is-my-data-handled-after-transcription
[P4]: https://support.plaud.ai/hc/en-us/articles/50742099278617-Comparison
[Y1]: https://github.com/FrigadeHQ/yogurt/tree/e1398289ab2bad7aba75ad5501ae16bbce9ac81a
[Y2]: https://www.yogurt.so/
[Y3]: https://github.com/FrigadeHQ/yogurt/blob/e1398289ab2bad7aba75ad5501ae16bbce9ac81a/app/src/main/callNames/attribute.ts
[Y4]: https://github.com/FrigadeHQ/yogurt/blob/e1398289ab2bad7aba75ad5501ae16bbce9ac81a/app/src/main/callNames/watch.ts
[Y5]: https://github.com/FrigadeHQ/yogurt/blob/e1398289ab2bad7aba75ad5501ae16bbce9ac81a/app/src/main/detect.ts
[J1]: https://yogurt-ai.dev/
[J2]: https://github.com/jarvisrchen/yogurt-open
[O1]: https://help.otter.ai/hc/en-us/articles/21665587209367-Speaker-Identification-Overview
[O2]: https://otter.ai/pricing.md
[O3]: https://help.otter.ai/hc/en-us/articles/40643231903639-Disable-Speaker-Learning
[O4]: https://help.otter.ai/hc/en-us/articles/19500988656279
[F1]: https://guide.fireflies.ai/articles/1208704416-getting-started-with-the-fireflies-desktop-app
[F2]: https://fireflies.ai/security
[F3]: https://fireflies.ai/pricing
[FA1]: https://help.fathom.video/en/articles/11577345-how-does-bot-free-recording-work-in-fathom-3-0
[FA2]: https://help.fathom.video/en/articles/449472-using-fathom-with-google-meet
[FA3]: https://help.fathom.video/en/articles/297152
[FA4]: https://help.fathom.video/en/articles/13926529
[FA5]: https://help.fathom.video/en/articles/6057089
[FA6]: https://fathom.video/pricing
[K1]: https://help.krisp.ai/hc/en-us/articles/8214720684956-AI-Meeting-Assistant-overview
[K2]: https://help.krisp.ai/hc/en-us/articles/360012035500-What-data-Krisp-sends-to-the-cloud
[K3]: https://krisp.ai/pricing/
[H1]: https://github.com/fastrepl/anarlog
[H2]: https://anarlog.so/blog/char-is-now-anarlog/
[H3]: https://char.com/
[H4]: https://docs.anarlog.so/notes
[L1]: https://www.limitless.ai/
[L2]: https://help.limitless.ai/en/articles/11501491-troubleshoot-improve-transcripts-summaries-saved-voices
[L3]: https://help.limitless.ai/en/articles/13004190-talking-to-someone-wearing-the-pendant-what-to-expect-and-how-we-handle-your-information
[T1]: https://tldv.io/
[T2]: https://tldv.io/app/pricing
[T3]: https://tldv.io/privacy/
[R1]: https://support.read.ai/hc/en-us/articles/52118970003987-How-Consent-Works-on-Web-Mode-vs-Desktop-Mode
[R2]: https://support.read.ai/hc/en-us/articles/52055801266579-What-Does-Read-AI-Do-With-My-Data
[R3]: https://www.read.ai/plans-pricing
[R4]: https://support.read.ai/hc/en-us/articles/36169538719379-Can-I-edit-text-and-speaker-names-in-my-meeting-report
[R5]: https://support.read.ai/hc/en-us/articles/49381158409491-MCP-Server
[Z1]: https://library.zoom.com/ai/ai-whitepaper/my-notes
[Z2]: https://library.zoom.com/ai/ai-whitepaper
[Z3]: https://library.zoom.com/ai/ai-whitepaper/feature-availability
[Z4]: https://zoom.com/ai-companion
[Z5]: https://news.zoom.com/zoom-smart-name-tags
[Z6]: https://www.zoom.com/en/blog/zoom-ai-companion-zoommate
[MS1]: https://learn.microsoft.com/en-us/microsoftteams/intelligent-recap-calls-meetings
[MS2]: https://learn.microsoft.com/en-us/microsoftteams/privacy/intelligent-recap
[MS3]: https://learn.microsoft.com/en-us/microsoftteams/rooms/voice-and-face-recognition
[MS4]: https://support.microsoft.com/en-us/teams/copilot/catch-up-on-meetings-with-microsoft-365-copilot-in-teams
[MS5]: https://support.microsoft.com/en-us/teams/meetings/customize-recap-summaries-in-microsoft-teams
[GO1]: https://support.google.com/meet/answer/14754931
[GO2]: https://support.google.com/meet/answer/17020724
[GO3]: https://support.google.com/meet/answer/13952129
[GO4]: https://knowledge.workspace.google.com/admin/generative-ai/generative-ai-in-google-workspace-privacy-hub
