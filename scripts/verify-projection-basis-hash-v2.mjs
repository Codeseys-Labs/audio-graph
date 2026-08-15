#!/usr/bin/env bun

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const fixtureUrl = new URL(
  "../src-tauri/fixtures/projection_basis_hash_v2/goldens.json",
  import.meta.url,
);
const fixturePath = process.argv[2] ?? fileURLToPath(fixtureUrl);
const catalog = JSON.parse(readFileSync(fixturePath, "utf8"));

function fail(kind) {
  throw new Error(`projection-basis-hash-v2 verifier: ${kind}`);
}

function requiredString(value) {
  if (typeof value !== "string" || value.trim().length === 0) {
    fail("invalid required string");
  }
  return value;
}

function optional(value) {
  return value === undefined || value === null ? null : value;
}

function finite(value, kind) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    fail(`non-finite ${kind}`);
  }
  return Object.is(value, -0) ? 0 : value;
}

function normalizeRevision(revision) {
  if (revision.contract_version === undefined) {
    const startTime = optional(revision.start_time);
    const endTime = optional(revision.end_time);
    const confidence = optional(revision.confidence);
    const turnId = optional(revision.turn_id);
    const endOfTurn = optional(revision.end_of_turn);
    const speakerId = optional(revision.speaker_id);
    const speakerLabel = optional(revision.speaker_label);
    const channel = optional(revision.channel);
    return {
      payloadKind: "legacy_v1",
      spanId: requiredString(revision.span_id),
      sourceId: requiredString(revision.source_id),
      sourceOrdinal: null,
      provider: requiredString(revision.provider),
      text: requiredString(revision.text),
      stability: revision.stability,
      isFinal: revision.is_final,
      revisionNumber: revision.revision_number,
      supersession:
        revision.supersedes === undefined || revision.supersedes === null
          ? { kind: "absent" }
          : {
              kind: "legacy",
              reference: requiredString(revision.supersedes),
            },
      timing:
        startTime === null && endTime === null
          ? { origin: "unavailable" }
          : {
              origin: "legacy_unspecified",
              startTime:
                startTime === null ? null : finite(startTime, "timing"),
              endTime: endTime === null ? null : finite(endTime, "timing"),
            },
      confidence:
        confidence === null
          ? { origin: "unavailable" }
          : {
              origin: "legacy_unspecified",
              value: finite(confidence, "confidence"),
            },
      turn:
        turnId === null && endOfTurn === null
          ? { origin: "unavailable" }
          : { origin: "legacy_unspecified", turnId, endOfTurn },
      speaker:
        speakerId === null && speakerLabel === null
          ? { origin: "unavailable" }
          : { origin: "legacy_unspecified", speakerId, speakerLabel },
      channel:
        channel === null
          ? { origin: "unavailable" }
          : { origin: "legacy_unspecified", value: channel },
    };
  }

  if (revision.contract_version !== 2) {
    fail("unsupported contract version");
  }
  const revisionNumber = revision.revision_number;
  let supersession;
  if (revisionNumber === 1 && revision.supersedes == null) {
    supersession = { kind: "absent" };
  } else if (
    Number.isSafeInteger(revisionNumber) &&
    revisionNumber > 1 &&
    revision.supersedes?.span_id === revision.span_id &&
    revision.supersedes?.revision_number === revisionNumber - 1
  ) {
    supersession = {
      kind: "v2_exact",
      spanId: revision.supersedes.span_id,
      revisionNumber: revision.supersedes.revision_number,
    };
  } else {
    fail("invalid v2 supersession");
  }

  const sourceOrder = revision.source_order;
  return {
    payloadKind: "v2",
    spanId: requiredString(revision.span_id),
    sourceId: requiredString(sourceOrder?.source_stream_id),
    sourceOrdinal: sourceOrder?.ordinal,
    provider: requiredString(revision.provider),
    text: requiredString(revision.text),
    stability: revision.stability,
    isFinal: revision.stability === "final",
    revisionNumber,
    supersession,
    timing: normalizeV2Timing(revision.timing),
    confidence: normalizeV2Value(revision.confidence, "confidence"),
    turn: normalizeV2Object(revision.turn, "turn"),
    speaker: normalizeV2Object(revision.speaker, "speaker"),
    channel: normalizeV2Value(revision.channel, "channel"),
  };
}

function normalizeV2Timing(timing) {
  if (timing?.origin === "unavailable") return { origin: "unavailable" };
  if (timing?.origin === "app_estimated") {
    return {
      origin: "app_estimated",
      startTime: finite(timing.start_time, "timing"),
      endTime: finite(timing.end_time, "timing"),
    };
  }
  if (timing?.origin === "provider" && ["coarse", "exact"].includes(timing.precision)) {
    return {
      origin: `provider_${timing.precision}`,
      startTime: finite(timing.start_time, "timing"),
      endTime: finite(timing.end_time, "timing"),
    };
  }
  fail("unsupported timing variant");
}

function normalizeV2Value(evidence, kind) {
  if (evidence?.origin === "unavailable") return { origin: "unavailable" };
  if (["app", "provider"].includes(evidence?.origin)) {
    return {
      origin: evidence.origin,
      value:
        kind === "confidence"
          ? finite(evidence.value, "confidence")
          : requiredString(evidence.value),
    };
  }
  fail(`unsupported ${kind} variant`);
}

function normalizeV2Object(evidence, kind) {
  if (evidence?.origin === "unavailable") return { origin: "unavailable" };
  if (!["app", "provider"].includes(evidence?.origin)) {
    fail(`unsupported ${kind} variant`);
  }
  if (kind === "turn") {
    return {
      origin: evidence.origin,
      turnId: requiredString(evidence.value?.turn_id),
      endOfTurn: evidence.value?.end_of_turn,
    };
  }
  const speakerId = optional(evidence.value?.speaker_id);
  const speakerLabel = optional(evidence.value?.speaker_label);
  if (speakerId === null && speakerLabel === null) fail("empty speaker evidence");
  return { origin: evidence.origin, speakerId, speakerLabel };
}

class Encoder {
  chunks = [];

  byte(value) {
    this.chunks.push(Buffer.from([value]));
  }

  u64(value) {
    if (!Number.isSafeInteger(value) || value < 0) fail("invalid unsigned integer");
    const bytes = Buffer.alloc(8);
    bytes.writeBigUInt64BE(BigInt(value));
    this.chunks.push(bytes);
  }

  string(value) {
    const bytes = Buffer.from(value, "utf8");
    this.u64(bytes.length);
    this.chunks.push(bytes);
  }

  boolean(value) {
    if (value !== true && value !== false) fail("invalid boolean");
    this.byte(value ? 1 : 0);
  }

  option(value, writeValue) {
    if (value === null) {
      this.byte(0);
    } else {
      this.byte(1);
      writeValue(value);
    }
  }

  f64(value) {
    const bytes = Buffer.alloc(8);
    bytes.writeDoubleBE(finite(value, "timing"));
    this.chunks.push(bytes);
  }

  f32(value) {
    const bytes = Buffer.alloc(4);
    bytes.writeFloatBE(finite(value, "confidence"));
    this.chunks.push(bytes);
  }

  digest() {
    return `sha256:${createHash("sha256").update(Buffer.concat(this.chunks)).digest("hex")}`;
  }
}

function encodeEvidence(encoder, revision) {
  encoder.byte(0x0a);
  if (revision.supersession.kind === "absent") encoder.byte(0);
  else if (revision.supersession.kind === "legacy") {
    encoder.byte(1);
    encoder.string(revision.supersession.reference);
  } else {
    encoder.byte(2);
    encoder.string(revision.supersession.spanId);
    encoder.u64(revision.supersession.revisionNumber);
  }

  encoder.byte(0x0b);
  const timingTags = {
    unavailable: 0,
    legacy_unspecified: 1,
    app_estimated: 2,
    provider_coarse: 3,
    provider_exact: 4,
  };
  encoder.byte(timingTags[revision.timing.origin]);
  if (revision.timing.origin === "legacy_unspecified") {
    encoder.option(revision.timing.startTime, (value) => encoder.f64(value));
    encoder.option(revision.timing.endTime, (value) => encoder.f64(value));
  } else if (revision.timing.origin !== "unavailable") {
    encoder.f64(revision.timing.startTime);
    encoder.f64(revision.timing.endTime);
  }

  encoder.byte(0x0c);
  const confidenceTags = { unavailable: 0, legacy_unspecified: 1, app: 2, provider: 3 };
  encoder.byte(confidenceTags[revision.confidence.origin]);
  if (revision.confidence.origin !== "unavailable") encoder.f32(revision.confidence.value);

  encoder.byte(0x0d);
  const fidelityTags = { unavailable: 0, legacy_unspecified: 1, app: 2, provider: 3 };
  encoder.byte(fidelityTags[revision.turn.origin]);
  if (revision.turn.origin === "legacy_unspecified") {
    encoder.option(revision.turn.turnId, (value) => encoder.string(value));
    encoder.option(revision.turn.endOfTurn, (value) => encoder.boolean(value));
  } else if (revision.turn.origin !== "unavailable") {
    encoder.string(revision.turn.turnId);
    encoder.boolean(revision.turn.endOfTurn);
  }

  encoder.byte(0x0e);
  encoder.byte(fidelityTags[revision.speaker.origin]);
  if (revision.speaker.origin !== "unavailable") {
    encoder.option(revision.speaker.speakerId, (value) => encoder.string(value));
    encoder.option(revision.speaker.speakerLabel, (value) => encoder.string(value));
  }

  encoder.byte(0x0f);
  encoder.byte(fidelityTags[revision.channel.origin]);
  if (revision.channel.origin !== "unavailable") encoder.string(revision.channel.value);
}

function projectionBasisHashV2(records) {
  const positioned = records.map((record) => ({
    position: record.first_accepted_sequence,
    revision: normalizeRevision(record.revision),
  }));
  for (const record of positioned) {
    if (!Number.isSafeInteger(record.position) || record.position <= 0) {
      fail("missing first Accepted position");
    }
  }
  positioned.sort((left, right) => left.position - right.position);
  for (let index = 1; index < positioned.length; index += 1) {
    if (positioned[index - 1].position === positioned[index].position) {
      fail("duplicate first Accepted position");
    }
  }
  const ordinalBySource = new Map();
  for (const { revision } of positioned) {
    if (revision.payloadKind !== "v2") continue;
    const prior = ordinalBySource.get(revision.sourceId);
    if (prior !== undefined && revision.sourceOrdinal <= prior) {
      fail(revision.sourceOrdinal === prior ? "duplicate source ordinal" : "reversed source ordinal");
    }
    ordinalBySource.set(revision.sourceId, revision.sourceOrdinal);
  }

  const encoder = new Encoder();
  encoder.chunks.push(Buffer.from("audio-graph:projection-basis:v2", "utf8"));
  encoder.byte(0);
  encoder.u64(positioned.length);
  for (const { revision } of positioned) {
    encoder.byte(0xa0);
    encoder.byte(0x01);
    encoder.byte(revision.payloadKind === "legacy_v1" ? 1 : 2);
    encoder.byte(0x02);
    encoder.string(revision.spanId);
    encoder.byte(0x03);
    encoder.string(revision.sourceId);
    encoder.byte(0x04);
    encoder.option(revision.sourceOrdinal, (value) => encoder.u64(value));
    encoder.byte(0x05);
    encoder.string(revision.provider);
    encoder.byte(0x06);
    encoder.string(revision.text);
    encoder.byte(0x07);
    encoder.byte(revision.stability === "partial" ? 1 : 2);
    encoder.byte(0x08);
    encoder.boolean(revision.isFinal);
    encoder.byte(0x09);
    encoder.u64(revision.revisionNumber);
    encodeEvidence(encoder, revision);
    encoder.byte(0xaf);
  }
  return encoder.digest();
}

let failed = false;
for (const golden of catalog.goldens) {
  try {
    const actual = projectionBasisHashV2(golden.records);
    if (actual !== golden.expected_digest) {
      failed = true;
      console.error(`${golden.name}: expected ${golden.expected_digest}, got ${actual}`);
    } else {
      console.log(`${golden.name}: ${actual}`);
    }
  } catch (error) {
    failed = true;
    console.error(`${golden.name}: ${error.message}`);
  }
}

if (failed) process.exit(1);
console.log(`projection-basis-hash-v2 verifier passed: ${catalog.goldens.length} goldens`);
