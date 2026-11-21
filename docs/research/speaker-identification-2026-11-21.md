# Research: speaker identification, late name binding, and 2025–2026 diarization for a local-first Rust app

Date: 2026-11-21
Context Seeds: `audio-graph-3588` (speaker timeline epic), `audio-graph-5011` (local worker),
`audio-graph-9509` (speaker identity ladder), `audio-graph-9c45` (speaker ID field ask),
`audio-graph-1fbd`, `audio-graph-eebf`, `audio-graph-262a`, `audio-graph-dcd3`, `audio-graph-b153`, `audio-graph-8181`
Companion design: [`docs/designs/lazy-speaker-identification.md`](../designs/lazy-speaker-identification.md)
Status: research record. It changes no code.

## Question

1. Which Zoom features match what the user calls "lazy speaker identification"? How does each one
   bind anonymous voices to names, including bindings made late or after the meeting?
2. As of 2025–2026, which streaming diarization, speaker separation, and speaker identification
   options can a local-first Rust desktop app use? The survey covers pyannote 3.1, community-1, and
   precision-2; NVIDIA Sortformer and Streaming Sortformer; NeMo TitaNet; WeSpeaker, 3D-Speaker, and
   ECAPA embeddings; the sherpa-onnx diarization APIs; diart-style online clustering; overlap
   separation (SepFormer, TIGER, MossFormer2); and cloud diarization from Deepgram, AssemblyAI,
   Speechmatics, Soniox, and AWS. It records the license and model size of every local model.
3. How do competitors put names on speakers: calendar attendees, platform participant and
   active-speaker metadata, voice enrollment, or name-mention inference?

## Direct answer

**1. Zoom has no feature called "lazy speaker identification".** A search for the exact phrase
returned only unrelated academic papers, so the phrase is the user's own. The closest Zoom
features, all verified on Zoom's support site on 2026-11-21, are listed below. Together they make
up the behavior the user describes:

| Zoom feature | Anonymous state | How a name gets bound | When the binding can happen |
|---|---|---|---|
| **Smart name tags for voice, manual** (Zoom Rooms) | Generic tags such as "Speaker 1" or "Participant 1" for people in the room | Someone uses the Rooms controller to assign a user to a generic tag, and can link the voice tag to a video tag | During the meeting. Manual tags are **not saved** when the meeting ends |
| **Smart name tags for voice, automatic** (Zoom Rooms) | Generic tags until a match | The room device compares room audio, **locally on the device**, against reference recordings of enrolled users. Only users **on the meeting invite or paired to the room** are compared | Live, against a **closed candidate set** |
| **Post-meeting attribution** (cloud recording transcript) | Generic tag or "Unknown Speaker" | The host edits the smart name tag, or the "Unknown Speaker" name, in the cloud recording transcript. The attributed user can opt in to an email notice | **After the meeting** (deferred binding) |
| **Host or co-host editing of smart name tags** (Workplace desktop, release of 2025-10-20) | Room-generated tags | Edits "sync across meeting features including closed captions, live transcripts, AI Companion, and cloud recording transcripts"; "the most recent change is applied" | Live, applied retroactively across artifacts, latest edit wins |
| **Workplace app automatic smart name tags** (Voice Recorder / My Notes, release of 2025-09-15) | "Generic speaker tags in the live transcript" | Manual tagging during the recording. Enrolled voice prints are matched automatically **for people listed as participants or hosts** | Live, then editable afterwards ("Change speaker names", "Revert") |
| **Per-participant attribution in ordinary Zoom meetings** | None: each participant has their own client stream | The platform's identity for each stream (`user_id`, `user_name`) | Immediately. This is platform metadata, not voice recognition |

What all of these share: anonymous tags appear first. A name is bound when the evidence is strong
enough: an enrolled voice matched against a **closed set of invitees**, or a human edit. Bindings
can be changed later, and an edit propagates to every derived artifact. The latest edit wins. The
reference voice data is consented, user-deletable, and kept in the account's cloud region. The
room's own audio is processed on the device and deleted when the meeting ends.

**2. State of the art, briefly:**

- **Streaming diarization.** End-to-end models with arrival-order speaker slots now lead.
  NVIDIA **Nemotron 3 Diarization** (released 2026-09-23, 100 M parameters, OpenMDW-1.1, up to
  8 speakers, 0.32–1.04 s streaming latency) cuts DER against Streaming Sortformer v2.1 at a 1.04 s
  buffer:
  - DIHARD III: 13.18 vs 19.60
  - AMI SDM: 12.80 vs 21.73
  - NOTSOFAR-1 SC: 12.77 vs 31.81

  It ships a native C++/ggml runtime, NeMo-Speech.cpp (Apache-2.0). These models keep speaker
  slots stable within a session, but they cap the speaker count and give no identity across
  sessions. Embeddings are still needed for identity.
- **Offline / clustering diarization.** pyannote **community-1** (CC-BY-4.0, gated) beats 3.1 on
  almost every benchmark in its card. It adds an "exclusive" one-speaker-at-a-time mode for
  aligning with STT output. **DiariZen** has the best open DER in its own table, but its weights
  are **CC BY-NC 4.0** and cannot ship commercially. pyannoteAI **precision-2/3** are proprietary
  and are the only pyannote models with voiceprint identification.
- **Embeddings** for verification and identification:
  - 3D-Speaker CAM++: 7.2 M parameters, 0.65% EER on VoxCeleb1-O
  - ERes2NetV2: 17.8 M, 0.61% EER
  - TitaNet-Large: 23 M, 0.66% EER, CC-BY-4.0
  - ECAPA-TDNN: 0.80% EER (SpeechBrain, Apache-2.0)

  Embeddings are **model-specific**: a voiceprint must be pinned to the exact model that made it.
- **sherpa-onnx 1.13.4**, the Rust crate already in this repo, includes
  `SpeakerEmbeddingManager`, which can add, search, verify, and return best matches. That covers
  enrollment-based identification with no new native dependency. It has no streaming diarizer, so
  AudioGraph's rolling-window clustering (`stabilize.rs`, which is diart's incremental scheme)
  stays the unbounded live path.
- **Separation.** SepFormer (8 kHz, Apache-2.0), TIGER (<1 M parameters, Apache-2.0), and
  MossFormer2_SS_16K (Apache-2.0) are all two-speaker, synthetic-mixture separators. None of them
  is evidence that separated lanes help live meeting ASR. The existing repo decision stands: keep
  separation offline and fixture-gated.
- **Cloud.**
  - Deepgram: streaming diarizer v1 only; word `speaker` only, no confidence when streaming.
  - AssemblyAI: streaming `speaker_labels` with a hard `max_speakers` cap of 1–10. Turns shorter
    than about 1 s are labeled `PENDING`, and overlapped speech is not split.
  - Speechmatics: real-time speaker identification from enrolled identifiers. Clips are 5–30 s,
    at most 50 identifiers per session, and identifiers are model-specific and encrypted per
    customer.
  - Soniox: token speakers, at most 15.
  - AWS: at most 30 speakers, `spk_0`–`spk_29`.

  AssemblyAI **Speaker Identification** maps labels to names or roles **from conversation content,
  without enrollment**. It is the closest cloud analogue to inferring names from transcript
  mentions.

**3. How competitors name speakers** (four mechanisms, often combined):

- **Platform participant and active-speaker metadata:**
  - Zoom per-participant streams.
  - Otter Notetaker, which uses "participant names of Zoom meetings".
  - Granola speaker tags, which read the meeting app's participant names and active-speaker
    indicator through the OS **Accessibility** API.
- **Voice enrollment against invitee-restricted candidates:**
  - Zoom smart name tags.
  - Microsoft Teams Intelligent Speakers: users must be enrolled **and invited**, at most 50
    invitees with voice profiles. Voice signatures are stored in the tenant's Microsoft Cloud.
- **Tags learned from user corrections.** Otter learns a speaker profile from manual tags and
  reuses it across a workspace. Tagging is only possible after a recording stops.
- **Name inference from content:**
  - AssemblyAI Speaker Identification.
  - Granola: "our AI usually does a good job of inferring speakers from contextual clues". Its
    fallback labels are **Me / Them**, from microphone versus system audio.

**Recommendation for AudioGraph** (detailed in the companion design): treat identity as a *late,
revisable relabel* on top of anonymous clusters, and route it through the existing
`SpeakerTimeline` → `SpeakerLabelRemap` → `supersede_entity` retcon spine. Fuse evidence in this
order:

1. User relabel
2. Source/channel prior (mic = me)
3. Platform or calendar roster as the **closed candidate set**
4. Self-introductions and addressed names in the transcript, judged by System-1 `choice`/`noul`
   questions in shadow mode until calibrated
5. Opt-in local voiceprints, self only at first

Keep voiceprints on the device, encrypted, model-pinned, and purgeable. Never send them to a cloud
identification API.

---

## Evidence by claim

### A. Zoom speaker identification features (primary: Zoom support, read 2026-11-21)

1. **Smart name tags for voice in Zoom Rooms**
   ([KB0077409](https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0077409), updated 2026-10-14).
   - Purpose: name tags "provide attribution in meeting-related features like captions,
     transcripts, and meeting summaries. ... Both automatic and manual smart name tags can be edited
     during a meeting via the Zoom Rooms controller."
   - Manual mode: "By default, manual smart name tags will display a generic name tag (e.g.,
     Speaker 1) ... tap Edit smart name tags, then tap a user to assign them to a name tag. If smart
     name tags for video are enabled, tap Next, then tap a user to link the voice tag with the video
     tag." Also: "**Manual smart name tags are not saved when the meeting ends.**"
   - Automatic mode compares only against a closed candidate set: "the Zoom Room will compare the
     audio of in-meeting participants in the Zoom Room against the reference audio data of enrolled
     users who are on the meeting invitation or paired to the Zoom Room." Limits: "enrolled users
     must be included on the meeting invite before the meeting begins"; "perform best when applied
     to 1-16 in-room participants"; "not supported in meetings with more than 100 invited
     participants".
   - Data handling: "The audio data from the Zoom Room is only processed locally on the equipment
     in the Zoom Room. All audio data from the participants ... and from the reference audio data
     ... is deleted from the equipment in the Zoom Room when the meeting ends. Enrolled users'
     reference audio data is stored in the Zoom cloud." Users "can disable this feature or delete
     their reference audio data from their profile at any time."
   - Deferred binding: the user option "Notify me when a smart name tag for voice is attributed to
     my account" sends "an email when a meeting host attributes a smart name tag to you after a
     meeting. This attribution occurs when a host edits a smart name tag from the cloud recording
     transcript."
   - Enrollment nudge: "When an unenrolled participant associates a manual smart name tag with
     their name during a meeting, Zoom will send them an email encouraging them to enroll."
   - Voice and video together: "the Zoom Room can also provide active speaker indication for
     in-room participants by highlighting the smart name tag of the speaker".
2. **Enrollment** ([KB0082578](https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0082578);
   [KB0083296](https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0083296), updated 2026-03-25).
   The user reads provided text aloud. "Using a single workflow, users can upload a single voice
   recording or image ... to activate multiple features." Delete is explicit: "Your reference voice
   recording cannot be used once deleted and the smart voice recognition feature will be disabled."
   Admins gate enrollment per account or group
   ([KB0082593](https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0082593)).
3. **Workplace app automatic smart name tags**
   ([KB0082489](https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0082489), updated 2026-09-17).
   These identify speakers "during mic-enabled audio interactions" and require "Voice recorder
   with AI or My Notes".
4. **Release notes** ([KB0080363](https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0080363);
   dates are the page's section headings).
   - 2025-09-15: "Voice Recorder with AI Companion: Support smart name tags for speaker
     identification ... the system automatically tags speakers in transcripts and summaries based
     on their voice profiles instead of using generic speaker labels".
   - 2025-09-15: "The system displays generic speaker tags in the live transcript that you can
     update with actual participant names. For meetings with enrolled voice prints, the system
     automatically identifies and labels speakers who are listed as participants or hosts."
   - 2025-10-20: "Meeting hosts will have the ability to create and edit Smart name tags generated
     by Zoom Rooms ... All Smart name tag changes sync across meeting features including closed
     captions, live transcripts, AI Companion, and cloud recording transcripts. If both Zoom Room
     users and meeting hosts make edits simultaneously, the most recent change is applied."
5. **Voice Recorder transcripts**
   ([KB0080794](https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0080794)).
   "Change speaker names": "In the full transcript, tap a speaker. Enter the name to search your
   contact list ... Entered names are added to your contact list. ... Tap Revert to return to the
   default name."
6. **Cloud recording transcripts**
   ([KB0064927](https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0064927), updated 2026-11-16).
   The transcript is generated after the meeting as a VTT file. "If a phrase belongs to an unknown
   speaker, you can also hover over Unknown Speaker and click the pencil icon to edit their name."
7. **Per-participant platform attribution** (Zoom RTMS developer docs,
   [media](https://developers.zoom.us/docs/rtms/meetings/media)).
   - "Audio data is available per participant and as a merged packet of all participants." Each
     packet "contains the user's participant identifier (`user_id`), username (`user_name`), and
     the timestamp".
   - "Transcript data is available per participant with attribution, also called diarization."
   - "Video data is sent as a single video stream of the active speaker."

   [INFERENCE] Ordinary Zoom meeting transcripts attribute speech by participant stream, not by
   voice recognition. That is why "Unknown Speaker" appears mainly for shared rooms and dial-ins,
   the case smart name tags exist to solve.
8. **My Notes** ([KB0083849](https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0083849), updated 2026-11-06).
   For third-party apps, My Notes "relies on your device's system and microphone audio", the same
   capture model as AudioGraph and Granola. The article does not describe how speakers are labeled
   (checked 2026-11-21). Speaker naming for My Notes is inferred only from KB0082489 (item 3).

### B. Platform metadata without a bot

- **Granola speaker tags**
  ([docs](https://docs.granola.ai/help-center/taking-notes/speaker-attribution)).
  - "Speaker tags use the participant names and active-speaker information shown by your meeting
    app. Accessibility lets Granola read this information from the app's interface; the Meet
    extension reads it from Google Meet." Available on macOS and Windows for Meet, Zoom, and Teams.
  - Limits: "If several people speak through the same shared device, Granola cannot distinguish
    the individuals"; speaker tags "do not add names to older transcripts".
  - Fallback: "When speaker tags aren't available or enabled, transcripts show **Me** and **Them**,
    corresponding to your microphone input and system audio. ... our AI usually does a good job of
    inferring speakers from contextual clues in the transcript, and you can correct a name directly
    in your notes".
- **Otter**
  ([Speaker Identification Overview](https://help.otter.ai/hc/en-us/articles/21665587209367-Speaker-Identification-Overview),
  updated 2026-04-01).
  - Otter Notetaker "will automatically tag speakers in real-time based on the participant names of
    Zoom meetings".
  - For audio Otter only hears, "By tagging a speaker, you will teach Otter to recognize that
    speaker ... for your future conversations". Workspace members' speaker profiles are shared.
  - Unknown speakers show as "Speaker #"; tagging one "will automatically tag all other associated
    Speaker #s".
  - "Speaker identification processing will begin after the recording has stopped"; tagging during
    a live recording is not supported; speakers cannot be deleted ("You cannot delete a speaker at
    this time").

### C. Enrollment-based identification in comparable products

- **Microsoft Teams Rooms Intelligent Speakers**
  ([Learn](https://learn.microsoft.com/en-us/microsoftteams/rooms/voice-recognition), updated 2026-09-16).
  - "People to be identified in the room, need to be enrolled with their voice profile and be
    invited to the scheduled meeting. The limitation for people invited with a voice profile is 50
    invitees."
  - "Each person gets a unique voice signature, stored securely in your organization's tenant in
    the Microsoft Cloud."
  - The `roomAttributeUserOverride` policy has three modes: Off, Attribute (enrolled users get
    their name, others show as "Speaker"), and Distinguish ("speaker 1, speaker 2 ... their name
    won't show").
  - "Speaker attribution using the meeting invite list begins when someone other than a Teams Room
    device joins the meeting."
- **Speechmatics**
  ([feature](https://docs.speechmatics.com/speech-to-text/features/speaker-identification);
  [realtime](https://docs.speechmatics.com/speech-to-text/realtime/speaker-identification)).
  - Enrollment uses "short audio clips (5–30s) where they ideally speak alone".
  - "A maximum of 50 speaker identifiers across all speakers can be configured per session."
  - "Identifiers are tied to the model used to generate them ... Whenever a model is updated,
    existing identifiers must always be regenerated."
  - Identifiers are "encrypted and scoped" per customer and per project.
  - In real time, identifiers are retrieved with `GetSpeakers`, then supplied through
    `speaker_diarization_config.speakers`. `speaker_sensitivity` "adjusts how strongly the system
    prefers enrolled speakers over detecting new generic ones"; unknown speakers keep `S1`, `S2`.
- **pyannoteAI**
  ([voiceprints tutorial](https://docs.pyannote.ai/tutorials/identification-with-voiceprints);
  [models](https://docs.pyannote.ai/models)).
  - Voiceprints come from at most 30 s of single-speaker audio. pyannoteAI "does not maintain a
    reusable voiceprint database"; outputs are deleted after 24 h.
  - Matching takes a `threshold` and an `exclusive` option. The docs warn: "A submitted voiceprint
    may match a speaker even when that person is not in the recording."
  - Identification is exclusive to Precision-2/3; "Speaker identification and voiceprint features
    are not available for Community-1 models."
- **AssemblyAI Speaker Identification, content-based**
  ([docs](https://www.assemblyai.com/docs/speech-understanding/speaker-identification)).
  "Replace generic 'Speaker A' ... labels with real names or roles, no voice enrollment needed.
  Speaker Identification uses conversation content to infer who's speaking and applies the
  identifiers you provide. ... can associate names when the names are present within the
  transcript." It requires `speaker_labels: true`.
- **Industry signal.** Microsoft retired Azure AI Speaker Recognition: "Beginning on September 30,
  2025, Azure AI Speaker Recognition will be retired"
  ([Azure update](https://www.microsoft.com/releasecommunications/api/v2/azure/rss/azure-ai-speaker-recognition-retirement)).
  [INFERENCE] General-purpose cloud voice biometrics APIs are commercially fragile. A local
  embedding path avoids depending on one.

### D. Local diarization models (license and size recorded)

| Model | Kind | Streaming | Max speakers | Size | License | Accuracy evidence | Rust/desktop path |
|---|---|---|---|---|---|---|---|
| pyannote `segmentation-3.0` | Powerset segmentation (VAD, overlap, local speakers) | Windowed | 3 local per window (powerset) | ~5.7 MB ONNX, 1.5 MB int8 (sherpa release, per [internal note](sherpa-diarization-live-2026-05.md)) | MIT ([HF API](https://huggingface.co/api/models/pyannote/segmentation-3.0), gated "auto") | Component of the 3.1 and community-1 pipelines | **In repo** via sherpa-onnx (`models/mod.rs:259-266`) |
| pyannote `speaker-diarization-3.1` | Pipeline: segmentation + embedding + clustering | Offline | Unbounded | Pipeline of the above | MIT ([HF](https://huggingface.co/pyannote/speaker-diarization-3.1), gated) | Baseline in the community-1 table | Python only. AudioGraph's sherpa pipeline is the same shape |
| pyannote `speaker-diarization-community-1` | segmentation-3.0-family + WeSpeaker ResNet34 embedding + **VBx** clustering; "exclusive" mode | Offline | Unbounded; `min/max/num_speakers` | Not stated on card | **CC-BY-4.0**, gated ([HF](https://huggingface.co/pyannote/speaker-diarization-community-1)) | DER (no collar, overlap scored), 3.1 → community-1: AMI-SDM 22.7 → 19.9, DIHARD3 21.4 → 20.2, CALLHOME 28.5 → 26.7, VoxConverse 11.2 → 11.2 | Python (pyannote.audio 4.0). Embedding is ONNX-exportable [INFERENCE] |
| pyannoteAI precision-2 / precision-3 / Live-1 | Proprietary API | Live-1: yes, 100 ms chunks, "sub-300ms", up to 8 speakers | precision: any | n/a | Commercial API. Enterprise self-host available ([models](https://docs.pyannote.ai/models)) | precision-2: AMI-SDM 15.6, DIHARD3 14.7. Precision-3 "34.2% more accurate, on average, than Community-1" (vendor claim) | Cloud content egress. Needs the Content-Egress Checklist |
| NVIDIA `diar_sortformer_4spk-v1` | End-to-end Sortformer | Offline | 4 | 123.5 M params (HF safetensors total) | **CC-BY-NC-4.0** ([HF API](https://huggingface.co/api/models/nvidia/diar_sortformer_4spk-v1)), so **not shippable** | — | — |
| NVIDIA `diar_streaming_sortformer_4spk-v2` | Streaming Sortformer with AOSC | Yes | 4 | ONNX ~492 MB fp32 (per Seed `8848`) | **CC-BY-4.0** ([HF API](https://huggingface.co/api/models/nvidia/diar_streaming_sortformer_4spk-v2)) | — | **In repo** via `parakeet-rs` (`models/mod.rs:90-92`). Not compiled into release builds (`8848`) |
| NVIDIA `diar_streaming_sortformer_4spk-v2.1` | Streaming Sortformer | Yes; 0.32–30.4 s buffer | 4 | **117 M** params | NVIDIA Open Model License ([card](https://huggingface.co/nvidia/diar_streaming_sortformer_4spk-v2.1)) | DIHARD3 full 20.21; AMI SDM 20.57; NOTSOFAR1 SC 28.75 (1.04 s) | NeMo or NeMo-Speech.cpp |
| NVIDIA **Nemotron-3-Diarization** | Sortformer + AOSC (paper [2611.12246](https://arxiv.org/abs/2611.12246)) | Yes; 0.32 / 0.64 / 1.04 s; offline 30.4 s | **8** | **100 M** params | **OpenMDW-1.1** ([card](https://huggingface.co/nvidia/Nemotron-3-Diarization); [license](https://openmdw.ai/license/1-1/): "permission ... to deal in the Model Materials without restriction") | At 1.04 s vs v2.1: DIHARD3 13.18 vs 19.60; CALLHOME 10.29 vs 11.31; AMI SDM 12.80 vs 21.73; NOTSOFAR1 SC 12.77 vs 31.81 | GGUF via [NeMo-Speech.cpp](https://github.com/NVIDIA/NeMo-Speech.cpp) (Apache-2.0, ggml, C SDK; CPU/Metal/Vulkan/CUDA build docs). Needs Rust FFI [INFERENCE] |
| BUT `DiariZen` (WavLM + VBx) | Pipeline | Offline | Unbounded | Not stated | Code MIT; **weights CC BY-NC 4.0** ([repo](https://github.com/BUTSpeechFIT/DiariZen)), so **not shippable** | Large-s80-v2: AMI-SDM 13.9, DIHARD3 14.5, NOTSOFAR-1 16.7 | Offline oracle only |
| 3D-Speaker diarization recipe | Pipeline (VAD, CAM++ embedding, clustering, optional overlap) | Offline | Unbounded | Per component | Apache-2.0 toolkit ([README](https://github.com/modelscope/3D-Speaker/blob/main/README.md)) | AMI_SDM 21.76, VoxConverse 11.75 (their table) | Embeddings already used through sherpa |

Streaming Sortformer's key property is "arrival-time ordering of output speakers" through an
"Arrival-Order Speaker Cache"
([arXiv 2507.18446](https://arxiv.org/abs/2507.18446), 2025-07-24). Speaker slots are stable
within one session, so every arrival-order model still needs an identity layer to name speakers
and to carry them across sessions.

### E. Speaker embedding models (identification and verification)

| Model | Params | VoxCeleb1-O EER | License | Notes |
|---|---|---|---|---|
| NeMo TitaNet-Large | **23 M** | 0.66% | CC-BY-4.0 ([card](https://huggingface.co/nvidia/speakerverification_en_titanet_large)) | English training mix; 16 kHz mono |
| NeMo TitaNet-small (sherpa ONNX `nemo_en_titanet_small.onnx`) | Not verified (repo notes ~38 MB ONNX, `models/mod.rs:275`) | Not verified | Not verified for the small checkpoint | **The repo's current live embedding model** (`DIAR_EMB_TITANET_FILENAME`), dim 192 per [internal note](sherpa-diarization-live-2026-05.md) |
| WeSpeaker ResNet34-LM (VoxCeleb) | Not stated on wrapper card | — | CC-BY-4.0, following the VoxCeleb data ([card](https://huggingface.co/pyannote/wespeaker-voxceleb-resnet34-LM)); WeSpeaker code Apache-2.0 | The community-1 embedding |
| 3D-Speaker CAM++ | **7.2 M** | 0.65% | Apache-2.0 toolkit ([README](https://github.com/modelscope/3D-Speaker/blob/main/README.md)); checkpoint terms on ModelScope not verified | Best accuracy per parameter in the table |
| 3D-Speaker ERes2Net-base / ERes2NetV2 / ERes2Net-large | 6.61 M / 17.8 M / 22.46 M | 0.84% / 0.61% / 0.52% | Same as CAM++ | sherpa ships a 3D-Speaker ERes2Net ONNX (~37 MB, multilingual, per the internal note) |
| ECAPA-TDNN (SpeechBrain `spkrec-ecapa-voxceleb`) | 20.8 M (the 3D-Speaker table's ECAPA; not stated on the SpeechBrain card) | 0.80% (SpeechBrain card) | **Apache-2.0** ([card](https://huggingface.co/speechbrain/spkrec-ecapa-voxceleb)) | Python. ONNX export not verified |

Embeddings from different models are not comparable. Speechmatics says so for its own identifiers
("Identifiers are tied to the model used to generate them"), and the same reasoning applies to any
voiceprint store [INFERENCE]. A voiceprint must record its embedding model's pin and digest and be
invalidated when the model changes.

### F. sherpa-onnx APIs available to this repo

Observed directly in the `sherpa-onnx-1.13.4` crate source in the local cargo registry, the crate
version `src-tauri/Cargo.toml:276` resolves to:

- `OfflineSpeakerDiarization::{create, sample_rate, set_config, process}` returns a result with
  `sort_by_start_time`. Config: `FastClusteringConfig { num_clusters, threshold }`. Already wrapped
  by `src-tauri/src/diarization/clustering.rs`.
- `SpeakerEmbeddingExtractor::{create, dim, create_stream, is_ready, compute}`. Already used by
  `src-tauri/src/diarization/worker.rs:530-541`.
- **`SpeakerEmbeddingManager::{create(dim), add, add_list, add_list_flattened, remove, search(embedding, threshold), get_best_matches, verify(name, embedding, threshold), contains, num_speakers, get_all_speakers}`.**
  This is an in-memory, name-keyed enrollment and matching API. AudioGraph does not use it yet.
- Module list: `offline_speaker_diarization`, `speaker_embedding`, `offline/online_speech_denoiser`,
  `vad`, `kws`, `offline/online_asr`, `tts`, and others. There is **no** online diarization module
  and **no** source-separation module in the 1.13.4 Rust crate, although the sherpa project lists
  source separation as a feature ([repo](https://github.com/k2-fsa/sherpa-onnx)).

### G. diart-style online clustering

[diart](https://github.com/juanmc2005/diart) (MIT, Python) "combines a speaker segmentation and a
speaker embedding model to power an incremental clustering algorithm that gets more accurate as the
conversation progresses". It ships uint8 ONNX segmentation and embedding assets. AudioGraph's
`src-tauri/src/diarization/stabilize.rs` already implements this scheme in Rust (Coria et al.
2021): cosine matching to running centroids, greedy cannot-link assignment, and duration-gated
centroid updates, with `DEFAULT_SIM_THRESHOLD = 0.6` and `DEFAULT_MIN_UPDATE_SECS = 1.5`. diart
adds nothing a Rust app needs beyond tuning evidence.

### H. Overlap-aware separation

| Model | License | Size | Input | Evidence |
|---|---|---|---|---|
| SpeechBrain SepFormer (`sepformer-wsj02mix`) | Apache-2.0 ([card](https://huggingface.co/speechbrain/sepformer-wsj02mix)) | Not stated on card | **8 kHz** mono | 22.4 dB SI-SNRi on WSJ0-2Mix (synthetic two-speaker mixtures) |
| TIGER (`JusperLee/TIGER-speech`, ICLR 2025) | Card: Apache-2.0 ([HF](https://huggingface.co/JusperLee/TIGER-speech)); repo: MIT | "first speech separation model with **fewer than 1 million parameters** that achieves performance close to the SOTA" | Two-speaker | Beats TF-GridNet on the EchoSet noisy and reverberant set with 94.3% fewer parameters |
| MossFormer2_SS_16K (ClearerVoice-Studio) | Apache-2.0 ([HF](https://huggingface.co/alibabasglab/MossFormer2_SS_16K)) | Not stated | **16 kHz** | "separates mixed-speaker speeches into individual speaker's speech"; trained partly on private data |
| pyannote `speech-separation-ami-1.0` | Gated (see [prior decision](source-separation-bakeoff-2026-06-26.md)) | — | 16 kHz | Joint diarization and separation on AMI. Python only |

Cloud streaming diarizers do not separate overlap either. AssemblyAI: "When two speakers talk
simultaneously, the model cannot split the audio and will assign the turn to a single speaker"
([docs](https://www.assemblyai.com/docs/streaming/diarization-and-multichannel)). pyannote's
"exclusive" mode (community-1 card) instead *picks the most likely transcribed speaker* during
overlap, and that is what transcript attribution needs. The repo's decision
([`source-separation-bakeoff-2026-06-26.md`](source-separation-bakeoff-2026-06-26.md), Seeds
`dd19` and `c237`) to keep separation experimental and offline still holds.

### I. Cloud provider diarization (as of 2026-11-21)

| Provider | Streaming diarization | Speaker count | Confidence | Identification | Source |
|---|---|---|---|---|---|
| Deepgram | `diarize_model=latest` resolves to streaming v1; v2 is batch only ("not supported" streaming) | Not stated | Streaming returns only `speaker`; batch adds `speaker_confidence`; `metadata.diarize_info` shows the diarizer that actually ran | None | [docs](https://developers.deepgram.com/docs/diarization) |
| AssemblyAI | `speaker_labels: true` on `assemblyai-stt-3-7-realtime`; turn `speaker_label` plus per-final-word `speaker` | `max_speakers` 1–10 is a **hard cap**: extra speakers are merged into the nearest label | `speaker_confidence` is "not calibrated ... Do not apply absolute thresholds"; turns under ~1 s are `PENDING` | Speaker Identification (names or roles from content) in Speech Understanding | [streaming](https://www.assemblyai.com/docs/streaming/diarization-and-multichannel), [SI](https://www.assemblyai.com/docs/speech-understanding/speaker-identification) |
| Speechmatics | `diarization: "speaker"` | `max_speakers` applies to generic speakers only | — | Enrolled identifiers: 5–30 s clips, at most 50 per session, model-specific, encrypted per customer | [feature](https://docs.speechmatics.com/speech-to-text/features/speaker-identification) |
| Soniox | `enable_speaker_diarization`; per-token `speaker` | "Up to **15** different speakers" | — ; "Temporary speaker switches that stabilize"; endpointing "reduce[s] diarization accuracy" | None | [docs](https://soniox.com/docs/stt/concepts/speaker-diarization) |
| AWS Transcribe | `show-speaker-label=true` (HTTP/2 or WebSocket) | At most **30** (`spk_0`–`spk_29`); `MaxSpeakerLabels` in batch | — | None; channel identification is a separate feature | [docs](https://docs.aws.amazon.com/transcribe/latest/dg/diarization.html) |

Every provider returns **session-scoped anonymous labels** that can churn. In AudioGraph they are
correctly kept as `provider_speaker_id` provenance and never treated as identity
(`src-tauri/src/projections.rs:608-637`).

### J. Privacy and legal framing of voiceprints

- **Illinois BIPA** defines "Biometric identifier" to include "voiceprint"
  ([740 ILCS 14/10](https://www.ilga.gov/legislation/ilcs/documents/074000140K10.htm)). Section 15
  ([740 ILCS 14/15](https://www.ilga.gov/legislation/ilcs/documents/074000140K15.htm)) requires:
  - a public written retention policy, with destruction "when the initial purpose ... has been
    satisfied or within 3 years of the individual's last interaction, whichever occurs first";
  - written notice of purpose and term, and "a written release" before collection;
  - no sale, and no disclosure without consent;
  - protection "the same as or more protective than" other confidential data.
- **GDPR** Art. 9 makes "biometric data for the purpose of uniquely identifying a natural person" a
  special category ([EUR-Lex 2016/679](https://eur-lex.europa.eu/eli/reg/2016/679/oj/eng)).
  [INFERENCE] Matching an *enrolled* voiceprint is identification under this definition. Transient
  diarization embeddings that are never matched to a named person sit closer to the line, but the
  design should treat them as biometric as well.
- Product pattern: Zoom processes room audio on the device and deletes it, and lets users delete
  their reference voice. Teams and Zoom both restrict matching to invitees. pyannoteAI refuses to
  keep a voiceprint database. Otter cannot delete a speaker profile (as of 2026-04-01). That is the
  failure mode to avoid.

---

## Uncertainties

- **"Lazy speaker identification" is not a verifiable Zoom product name.** The mapping in the
  direct answer is the closest set of real features. If the user meant a specific Zoom Rooms or
  Revenue Accelerator screen, it was not found.
- Zoom support pages are rendered client-side. Content was read through a headless browser on
  2026-11-21, and release-note dates are the page's section headings.
- Not verified: TitaNet-small license and parameter count, the per-checkpoint ModelScope terms for
  3D-Speaker models, and the sizes of SepFormer, MossFormer2, and DiariZen.
- Nemotron-3-Diarization numbers are NVIDIA's own (forced-alignment references, BF16, Blackwell
  GPU). No CPU latency was published for the diarizer, and no ONNX export is listed. Desktop CPU
  viability needs a spike.
- The pyannote community-1 blog shows a CMS "published" date of 2026-11-20 that disagrees with its
  body text, which mentions a webinar on October 23. The card's benchmark table is "last updated in
  2025-09".
- No source was found that quantifies identification accuracy for content-based name inference
  (AssemblyAI) or for Granola's Accessibility speaker tags.
- Legal points are summaries of statutory text, not legal advice. Other US state laws (Texas,
  Washington) were not reviewed.

## Implications for AudioGraph (tied to code)

1. **Build identity on the existing retcon spine, not a new pipeline.**
   `SpeakerTimeline::apply_event` already emits a `SpeakerLabelRemap` whenever a span's label
   changes (`src-tauri/src/projections.rs:792-850`).
   `speech/mod.rs::dispatch_diarization_span_revision` (`:459-496`) feeds it to
   `TemporalKnowledgeGraph::supersede_entity` (`graph/temporal.rs:413-594`). Accepted revisions are
   persisted (`speech/mod.rs:570-580`, Seed `719d` closed). A late name binding should therefore be
   a **new revision of each anonymous cluster's spans** with the same `speaker_id` and a new
   `speaker_label`. That is the Zoom "edit the smart name tag, and it syncs everywhere,
   latest-wins" behavior, rebuilt on event-sourced revisions.
2. **Keep `speaker_id` anonymous and stable. Change only `speaker_label` and an additive identity
   stamp.** Local clustering span ids embed the speaker id
   (`speech/mod.rs::diarization_span_revision_id`, `:368-383`). Changing the id would create new
   spans instead of revising existing ones.
3. **Candidate sets must be closed and small.** Zoom and Teams match only invitees (at most 100 and
   50). For AudioGraph, candidates are: calendar attendees (Seed `53cf`), an optional
   Accessibility-read roster (Granola pattern, per-OS capability), names cued in the transcript,
   and the local user. That also fits the System-1 contract: `choice` over at most 18 named
   candidates plus `none_of_these` and `unsure` fits Laya's `max_choice_options: 20`
   (`src-tauri/src/decision/route.rs:73-78`).
4. **Mic means "Me" is the cheapest strong prior.** Granola and Zoom My Notes both rely on mic
   versus system audio. It needs source-local diarization timelines. Today the clustering worker
   emits `timeline_id: "session"` and `source_id: None` (`speech/mod.rs:2034-2047`). Source types
   are already classified in `audio/capture.rs` (`Device{Input}` is `Physical`; `Application` is
   `AppProcessDerived`; `SystemDefault` is `Mixed`).
5. **Voiceprints are optional, local, and model-pinned.** `SpeakerEmbeddingManager` exists in the
   current sherpa crate. Cluster centroids already exist in `SpeakerRegistry`
   (`diarization/stabilize.rs:37-58`). Voiceprints must record the embedding model's pin and
   digest (Speechmatics caveat), be encrypted at rest, sit behind explicit opt-in (BIPA, GDPR
   Art. 9), and support crypto-shred purge. Cloud voiceprint APIs (Speechmatics identifiers,
   pyannoteAI voiceprints) are biometric egress and should stay out of scope.
6. **Streaming engine choice is orthogonal to identity.** Keep sherpa clustering as the unbounded
   default. Evaluate Nemotron-3-Diarization through NeMo-Speech.cpp as an 8-speaker,
   permissive-license successor to the parakeet-rs Sortformer v2 path (Seed `8848`). Never ship
   Sortformer v1 or DiariZen weights (NC licenses).
7. **Separation stays off the live path.** It is useful only for excluding overlapped frames from
   identity embeddings and, later, for offline refinement experiments gated by `c237` and `dd19`.
8. **Prerequisite gaps** found while grounding this research:
   - Clustering revisions carry no basis anchors (Seed `ca06`), so `timeline.rs`'s join cannot map
     them to transcript spans.
   - The projection prompt reads the inline label instead of the corrected timeline label (Seed
     `1586`), so relabels never reach the LLM.
   - Release builds compile no neural diarizer (Seed `8848`).
   - Notes have no `ReattributeNoteSpeaker` consumer yet (Seed `262a`, which depends on `4796`).
