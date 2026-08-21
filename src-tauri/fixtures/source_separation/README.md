# Source-Separation Fixture Set

Seed: `audio-graph-c237`

This directory contains tiny non-secret speech fixtures for offline
source-separation and speaker-attribution bakeoffs.

The checked-in WAV files are derived from LibriSpeech `clean/test` rows hosted
by OpenSLR and Hugging Face. LibriSpeech is licensed CC BY 4.0 and is derived
from public-domain LibriVox recordings.

Sources checked 2026-06-26:

- OpenSLR LibriSpeech: https://www.openslr.org/12
- OpenSLR Mini LibriSpeech: https://www.openslr.org/31/
- Hugging Face LibriSpeech dataset card: https://huggingface.co/datasets/openslr/librispeech_asr
- LibriVox public domain policy: https://librivox.org/pages/public-domain/

The fixture manifest records speaker ids, transcript text, timing annotations,
audio format, provenance, and candidate-quality thresholds. It intentionally
marks mono-ASR and diarization baselines as pending until a real local or cloud
baseline run is recorded. Generated speaker lanes remain experimental and must
not become selectable until those baselines and source-separation measurements
exist.

Each of the four checked-in WAVs (the two LibriSpeech source components and
the two derived overlap/turn-taking fixtures) also carries a `sha256` and
`byte_length` in the manifest for tamper-evidence. Because these are real
recordings, not code-generated, there is no synthesizer to regenerate them
from — `source_separation_fixtures.rs`'s
`source_separation_wavs_match_manifest_sha256` test is assert-only and fails
loudly, naming the drifted file, if the on-disk bytes ever stop matching the
recorded hash.
