import type { SpeechSpeakerValue } from "./speechSpanRevision";

function acceptsSpeechSpeakerValue(_value: SpeechSpeakerValue): void {}

acceptsSpeechSpeakerValue({ speaker_id: "speaker-id" });
acceptsSpeechSpeakerValue({ speaker_label: "Speaker Label" });
acceptsSpeechSpeakerValue({
  speaker_id: "speaker-id",
  speaker_label: "Speaker Label",
});

// @ts-expect-error SpeechSpeakerValue requires at least one non-null identifier.
acceptsSpeechSpeakerValue({});

// @ts-expect-error Two null identifiers do not satisfy the schema's anyOf constraint.
acceptsSpeechSpeakerValue({ speaker_id: null, speaker_label: null });
