# Source-Separation Bakeoff For Experimental Speaker PCM Lanes

Date: 2026-06-26
Seed: `audio-graph-dd19`
Status: research decision, implementation gated by `audio-graph-c237`

## Decision

Keep source-separated speaker PCM lanes **experimental and non-selectable** for
the cross-platform alpha.

The default route stays:

1. capture or import audio as source-local PCM,
2. downmix/resample into the 16 kHz mono processed-audio bus,
3. run ASR on mono audio,
4. attach provider or local speaker attribution as revisioned timeline metadata.

Separated speaker PCM lanes may be evaluated offline, but they must not become a
provider-routing input until a ground-truth fixture set proves that they improve
transcription without creating artifact-driven speaker or text errors.

## Why

Speech separation can improve overlap-heavy meeting transcription, but the
usable systems are model- and pipeline-heavy. The local Rust-ready options do not
yet prove meeting-grade speaker-lane quality inside AudioGraph's desktop latency
and packaging constraints.

Current AudioGraph code also does not preserve source-native multichannel audio
through the processed-audio bus. The safe design is to treat separated lanes as
derived artifacts with lower authority than the original mono stream.

## Candidate Paths

| Candidate | What It Offers | Alpha Readiness | Decision |
|---|---|---:|---|
| `sherpa-onnx` source separation | Cross-platform ONNX ecosystem that already has Rust/C/Python bindings and source-separation model pages. | Medium for offline experiments, low for meeting speaker lanes. Existing listed models are music/source-separation oriented, not proven speaker-stream separation for meetings. | Evaluate only after fixture harness exists. Do not route live ASR through it yet. |
| `charon-audio` | Rust source-separation crate with ONNX Runtime/Candle backends, CPAL realtime support, Symphonia/Rubato/Hound I/O. | Medium for Rust integration, low for meeting speech quality. It presents itself as music source separation. | Useful prototype shell for model plumbing, not evidence that speaker lanes are safe. |
| `pyannote/speech-separation-ami-1.0` | Real meeting-speech pipeline: 16 kHz mono input, diarization output, separated speaker streams, trained on AMI SDM. | High research relevance, low product integration readiness. Python/HF access-token/user-condition path, GPU recommended for speed, not a Rust desktop runtime. | Use as offline oracle/baseline, not embedded alpha runtime. |
| Custom ONNX/Candle model path | Could eventually give a controlled Rust runtime. | Unknown. Requires model choice, licensing, runtime packaging, benchmark harness, and artifact rollback. | Defer until fixtures show a specific model is worth porting. |

## External Evidence

- Charon documents a Rust source-separation library with ONNX Runtime and Candle
  backends, audio I/O, CPAL realtime support, and hardware acceleration hooks,
  but describes the domain as music source separation.
  https://docs.rs/charon-audio/latest/charon_audio/
- The pyannote AMI separation pipeline ingests 16 kHz mono audio and returns both
  diarization and separated sources; it requires `pyannote.audio[separation]`,
  accepted Hugging Face model conditions, and a token.
  https://huggingface.co/pyannote/speech-separation-ami-1.0
- Sherpa-ONNX documents local, self-contained ONNX runtime support across Linux,
  macOS, Windows, mobile, WebAssembly, and includes sections for speaker
  diarization, speech enhancement, and source separation.
  https://k2-fsa.github.io/sherpa/onnx/index.html
- PixIT targets the exact problem we care about: joint diarization and speech
  separation from real-world multi-speaker recordings. Its abstract says it
  improves ASR on meeting corpora, but that is still a research pipeline, not
  proof that local desktop routing should trust separated lanes by default.
  https://arxiv.org/abs/2403.02288
- Continuous speech separation literature shows why this is hard: long-form
  meeting separation needs windowing/stitching, diarization, speaker consistency,
  and ASR interaction. It is not just "split mono into N channels."
  https://arxiv.org/html/2309.16482v2

## Bakeoff Gates

Before AudioGraph can expose separated speaker PCM lanes, all of these must pass:

1. Fixture set: `audio-graph-c237` must provide at least one overlap clip and one
   turn-taking clip with ground-truth speakers, timings, transcripts, and license
   provenance.
2. Baseline: record mono ASR plus local/provider diarization results first.
3. Candidate output: for each separation candidate, persist derived lane files,
   lane timing, model/runtime version, CPU/memory, wall-clock latency, and
   platform.
4. Quality: separated-lane ASR must improve overlap WER or speaker-attributed WER
   without degrading non-overlap regions.
5. Artifact guard: if a lane contains musical/noise artifacts, dropped words, or
   duplicated speech, the original mono ASR remains the source of truth.
6. Routing guard: provider routing stays disabled unless source provenance,
   provider attribution metadata, and adapter fixtures all agree.
7. Replay: session artifacts must record that separated lanes were experimental
   derived audio, not source-native channels.

## Measurement Plan

Minimum metrics:

- overlap-region WER against ground-truth transcript,
- full-session WER,
- diarization error rate or speaker-attributed WER where labels exist,
- duplicated-token count across lanes,
- missing-token count compared with mono baseline,
- real-time factor,
- peak RSS and average CPU on macOS, Windows, and Linux,
- end-to-end latency added before ASR can emit first final span.

Recommended fixture file shape:

```json
{
  "id": "two-speaker-overlap-001",
  "audio_path": "fixtures/source_separation/two-speaker-overlap-001.wav",
  "sample_rate": 16000,
  "speakers": [
    { "id": "speaker_a", "label": "Speaker A" },
    { "id": "speaker_b", "label": "Speaker B" }
  ],
  "segments": [
    {
      "speaker_id": "speaker_a",
      "start_ms": 1200,
      "end_ms": 4200,
      "text": "..."
    }
  ],
  "license": "fixture-specific",
  "source": "fixture-specific"
}
```

## Architecture Boundary

Separated lanes are not source-native channels. They are derived artifacts.

That means:

- `AudioSourceInfo.channel_provenance.source_native` must remain `false` for
  generated lanes.
- provider `requires_source_native_channels` must not be satisfied by separated
  lanes.
- the mono processed-audio bus remains the default ASR input.
- notes and temporal graph projections consume revisioned transcript and
  diarization joins, not direct separated PCM trust.

## Recommendation

Do not add a source-separation crate to the live desktop runtime yet.

The next concrete step is `audio-graph-c237`: build the fixture harness. After
that, run an offline bakeoff in this order:

1. pyannote AMI pipeline as an offline quality oracle,
2. sherpa-onnx source-separation path if a speech-suitable model is identified,
3. `charon-audio` only as a Rust integration/runtime experiment with the same
   fixtures,
4. custom ONNX/Candle path only if one of the above proves worth productizing.

Until those results exist, source-separated speaker PCM lanes should stay out of
Settings and provider routing.
