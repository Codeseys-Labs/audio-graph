# Projection Basis Hash v2 Canonical Encoding

Status: active companion design for accepted
[ADR-0042](../adr/0042-version-projection-basis-hashes-by-speech-semantics.md).

## Purpose and scope

This document freezes the byte-level encoder, logical-span ordering fold, and
cross-version golden fixtures for Projection Basis transcript hash v2. ADR-0042
owns the architectural decision to version hashes by speech-revision semantics;
this document owns implementation detail that does not belong in an ADR.

It does not change canonical-log framing, define the projection patch schema,
authorize a Session semantics-floor transition, or change provider
selectability. The Session stream and downgrade decision is established in
[ADR-0041](../adr/0041-keep-versioned-speech-revisions-in-one-canonical-stream.md).

Normative words `MUST`, `MUST NOT`, and `SHOULD` apply because ADR-0042 is
accepted.

## Session semantics floor and hash-version selection

The Session has one monotonic durable compatibility floor named
`session_semantics_version`. It expresses the minimum reader and Session
semantics required across canonical transcript payloads, Projection Bases, and
projection patches; it MUST NOT be computed as the maximum payload or hash
version observed in those artifacts. An absent value on a historical Session
means v1. New state writes the value explicitly.

The current floor is derived from a canonical Session-provenance transition
that has become durably Accepted under ADR-0027 and is exposed through the
Session manifest and typed artifact inventory. The only transition defined
here is monotonic v1 to v2. It MUST be durably Accepted before the first v2
transcript append, hash-v2 basis creation, or hash-v2 patch creation or apply.
Retrying an already Accepted v2 transition is an idempotent success; the floor
MUST never decrease.

A v2 floor with no v2 transcript, basis, or patch is a valid guard-ahead state.
A v2 transcript, basis, or patch with a v1 floor is forbidden artifact-ahead
state and MUST be reported as corruption. Writers, basis creators, patch
writers, and patch appliers MUST refuse to create or apply such state; readers
MUST NOT infer or promote the floor from an artifact.

Hash selection is based on this accepted Session semantics floor, not on the
payload variants covered by one basis.

| Basis case | Required algorithm |
| --- | --- |
| Deserialized historical basis with absent `hash_version` | v1 forever |
| Deserialized historical basis with explicit `v1` | v1 forever |
| Newly created basis while the accepted Session floor is v1 | v1 |
| Newly created basis while the accepted Session floor is v2 | v2, even if every covered row is legacy v1 |
| Deserialized basis with explicit `v2` and accepted floor v2 | v2 |
| Deserialized basis with explicit `v2` and floor v1 | typed corruption |
| Unsupported declared version or invalid/missing floor during new-state creation | typed failure |

Basis creation, prompt construction, covered-subset classification, accepted
patch validation, and replay MUST use the same selection rule. A historical v1
patch does not become v2 merely because its Session floor later advances.

## Logical-span order

The encoder consumes latest eligible logical spans, not raw event arrival
order.

1. Replay accepted transcript records in canonical sequence order.
2. On the first Accepted record for a logical `span_id`, store that canonical
   sequence as the span's immutable first position.
3. A later accepted revision replaces the selected semantic revision but
   retains the first position.
4. Sort selected spans by first position. Canonical sequence is unique for
   conforming records, so no tie exists in ordinary operation.
5. Only a compatibility import that cannot supply unique first positions may
   break an equal-position tie by ascending raw UTF-8 `span_id` bytes. The
   import MUST record that degraded condition and exercise a dedicated fixture.
6. For every v2 `source_stream_id`, selected spans in logical order MUST have
   strictly increasing `source_order.ordinal`. A reversal is typed corruption;
   the fold MUST NOT repair it by sorting on ordinal.

Cross-source order therefore comes from first canonical Accepted sequence.
Source order remains an invariant within one source and never pretends to be a
global clock. The normalized v2 projection prompt uses this same logical-span
order.

## Digest envelope

The output is `sha256:` followed by 64 lowercase hexadecimal characters.

The SHA-256 input is exactly:

1. UTF-8 bytes for `audio-graph:projection-basis:v2`;
2. one zero byte (`00`);
3. selected logical-span count as unsigned 64-bit big-endian;
4. each canonical semantic revision record in logical-span order.

The numeric first-position sequence chooses record order but is not encoded.

## Primitive encodings

| Type | Encoding |
| --- | --- |
| Tag or enum | one unsigned byte |
| Boolean | `00` false, `01` true; other values invalid |
| Unsigned integer | unsigned 64-bit big-endian |
| String | unsigned 64-bit big-endian byte length, then exact UTF-8 bytes |
| Optional value | `00` absent; `01` followed by the value; other tags invalid |
| `f64` | IEEE-754 binary64 bits, big-endian |
| `f32` | IEEE-754 binary32 bits, big-endian |

Strings are not trimmed, case-folded, or Unicode-normalized. Rust/JSON strings
already contain valid UTF-8; invalid UTF-8 is rejected before encoding.

All non-finite floats are rejected before hashing. Positive and negative zero
both encode as the all-zero positive-zero bit pattern. Other finite values keep
their exact declared-width IEEE-754 bits; confidence remains `f32` and timing
remains `f64`.

## Semantic revision record

Each record begins with `a0`, emits every field below in the listed order, and
ends with `af`. Every field emits its one-byte field tag before its value.

| Order | Field tag | Field | Value encoding |
| ---: | ---: | --- | --- |
| 1 | `01` | Payload kind | `01` legacy v1, `02` Speech Span Revision v2 |
| 2 | `02` | Stable span id | string |
| 3 | `03` | Source identity | string |
| 4 | `04` | V2 source ordinal | optional unsigned integer; absent for legacy |
| 5 | `05` | Provider identity | string |
| 6 | `06` | Transcript text | string |
| 7 | `07` | Stability | `01` partial, `02` final |
| 8 | `08` | Finality | boolean; legacy preserves `is_final`, v2 derives it from final stability |
| 9 | `09` | Revision number | unsigned integer |
| 10 | `0a` | Supersession | variant below |
| 11 | `0b` | Timing evidence | variant below |
| 12 | `0c` | Confidence evidence | variant below |
| 13 | `0d` | Turn evidence | variant below |
| 14 | `0e` | Speaker evidence | variant below |
| 15 | `0f` | Channel evidence | variant below |

The payload-kind tag is semantic: legacy-unspecified evidence is not equivalent
to provider/app-qualified v2 evidence even when scalar values match.

### Supersession tags

| Tag | Meaning | Payload |
| ---: | --- | --- |
| `00` | absent | none |
| `01` | legacy reference | legacy supersession string |
| `02` | v2 exact reference | span-id string, then revision unsigned integer |

V2 revision 1 requires absent supersession. V2 revision `n > 1` requires the
same span id at exactly `n - 1`. Invalid supersession is rejected before hash
construction.

### Timing tags

| Tag | Meaning | Payload |
| ---: | --- | --- |
| `00` | unavailable | none |
| `01` | legacy unspecified | optional start `f64`, optional end `f64` |
| `02` | app estimated | start `f64`, end `f64` |
| `03` | provider coarse | start `f64`, end `f64` |
| `04` | provider exact | start `f64`, end `f64` |

Legacy start and end retain independent presence so a partial legacy value is
not fabricated into a complete interval.

### Confidence tags

| Tag | Meaning | Payload |
| ---: | --- | --- |
| `00` | unavailable | none |
| `01` | legacy unspecified | value `f32` |
| `02` | app | value `f32` |
| `03` | provider | value `f32` |

### Turn tags

| Tag | Meaning | Payload |
| ---: | --- | --- |
| `00` | unavailable | none |
| `01` | legacy unspecified | optional turn-id string, optional end-of-turn boolean |
| `02` | app | turn-id string, end-of-turn boolean |
| `03` | provider | turn-id string, end-of-turn boolean |

### Speaker tags

| Tag | Meaning | Payload |
| ---: | --- | --- |
| `00` | unavailable | none |
| `01` | legacy unspecified | optional speaker-id string, optional speaker-label string |
| `02` | app | optional speaker-id string, optional speaker-label string |
| `03` | provider | optional speaker-id string, optional speaker-label string |

V2 app/provider speaker evidence requires at least one present non-empty value,
matching the Speech Span Revision contract.

### Channel tags

| Tag | Meaning | Payload |
| ---: | --- | --- |
| `00` | unavailable | none |
| `01` | legacy unspecified | channel string |
| `02` | app | channel string |
| `03` | provider | channel string |

## Intentionally excluded values

The encoder does not include provider item id, transcript segment id,
provider/raw event reference, capture latency, ASR latency, received-at time,
numeric canonical sequence, canonical event/causal ids, basis heads,
payload/record hashes, or AGCL framing. These values are also excluded from the
normalized v2 transcript view sent to projection prompts.

Canonical-log payload and record hashes protect stored representation integrity;
they are not Projection Basis semantic hashes.

## Golden fixtures

These digests are normative. The authoring reference encoder used the primitive
and field tables above. Independent one-shot Bun and Ruby encoders reproduced
the same values during drafting, but those commands are not repository gates.
Activation still requires a checked-in Rust implementation and an independent
implementation or cross-version decoder to reproduce them.

The v2 span ids below are the accepted app-owned ids for source
`source-stream-a` ordinals 1 and 2:

- ordinal 1: `ssp_cbc3c0f3304aaae4a665331575fefad5`
- ordinal 2: `ssp_0a116c7335e72c0b2dc7e6abc6c77750`

### Historical hash-v1 goldens

- Existing projection fixture: `fnv1a64:4eb27818db1f8b3d`.
- Existing framed reader/ledger fixture: `fnv1a64:1708ff3ca940aa59`.
- An absent or explicit-v1 historical basis retains these values after a
  Session floor advance; it is never re-encoded through v2.

### V2 unavailable single span

One v2 final revision at logical position 1: ordinal-1 span above, provider
`fixture-provider`, text `hello`, revision 1, no supersession, app-estimated
timing `[0.0, 1.5]`, and unavailable confidence/turn/speaker/channel.

Expected:
`sha256:b53cfde4bd33d52a6a002a6409ae9b6ea3167aaceb77fa23365d570b2fdc9ffe`.

Encoding timing start as negative zero MUST produce the same digest. Any
non-finite timing or confidence MUST return a typed error and no digest.

### Post-cutover pure legacy basis

One legacy final revision at logical position 1: span `legacy-span-1`, source
`legacy-source`, provider `legacy-provider`, text `legacy hello`, revision 1,
no supersession, legacy timing start `0.0` and end `1.0`, legacy confidence
`0.9` as `f32`, legacy turn id `legacy-turn` with end-of-turn true, legacy
speaker id `speaker-legacy` with absent label, and unavailable channel.

When newly created with the accepted Session floor at v2, expected v2 digest:
`sha256:99ca80697d926d72f1590adbc6b9939b8a97a8f7f46cd8914e9bb83deac0136d`.

The same row in a historical absent/v1 basis continues to use its historical
hash-v1 value instead; payload composition does not select the algorithm.

### Mixed legacy then v2 basis

The pure-legacy fixture above at logical position 1 followed by the v2
unavailable fixture at logical position 2.

Expected:
`sha256:b554723b5bf7b6583fd938d30b4898f12f3d80126b7509fb7b9b8f9df434ac8a`.

Reversing the input event list while retaining their canonical first positions
MUST produce this same order and digest.

### Same-source late revision

The ordinal-1 span is first Accepted at logical position 1. The ordinal-2 span
is first Accepted at logical position 2. A later record revises ordinal 1 after
ordinal 2 has already been accepted. The selected basis remains:

1. ordinal-1 revision 2, text `hello revised`, exact v2 supersession of revision
   1, provider-exact timing `[0.0, 1.5]`, provider confidence `0.875`, provider
   turn `turn-a` ended, provider speaker id `speaker-a`, provider channel
   `left`;
2. ordinal-2 revision 1, text `later span`, no supersession, app-estimated
   timing `[1.5, 2.5]`, unavailable confidence/turn/speaker/channel.

Expected:
`sha256:9aef73e41ec8be266a9c41ca6f1a79a477b153d77191504cb23b447ff6fe9e12`.

An implementation that orders by the latest revision's Accepted sequence would
reverse these spans and fail this golden. A selected same-source order of
ordinal 2 then ordinal 1 is a typed error, not another valid digest.

## Required failure classes

The implementation may choose concrete enum names, but callers must be able to
distinguish at least:

- unsupported hash version;
- missing, unsupported, or non-monotonic Session semantics floor;
- v2 transcript, basis, or patch while the accepted floor remains v1;
- missing first Accepted position;
- duplicate/degraded compatibility position without the explicit tie policy;
- reversed or duplicate same-source ordinal;
- invalid supersession;
- invalid UTF-8 or invalid empty required string;
- non-finite timing/confidence;
- unsupported enum/option/boolean tag during decode;
- golden digest mismatch.

Failures are content-free and never fall back to hash v1.

## Activation evidence

Before the first production v2 append, hash-v2 basis, or hash-v2 patch,
executable gates must prove:

1. both independent encoders reproduce every digest above;
2. historical absent/v1 bases and accepted patches replay byte-for-byte after
   the Session floor advances;
3. basis order, normalized prompt order, and currency covered-subset order are
   identical;
4. mixed append/reopen/recovery reconstructs first positions and source order;
5. same-source late revision retains position and changes only semantic content
   and revision;
6. cross-source input reorder is deterministic;
7. every required failure class returns a typed content-free error; and
8. a v1-to-v2 floor transition becomes durably Accepted before v2 artifact
   creation, then a crash before any v2 artifact reopens as safe guard-ahead,
   retries idempotently, and creates a new pure-legacy basis with hash v2;
9. a crash/recovery fixture presenting independently injected v2 transcript,
   basis, and patch artifacts with a v1 floor rejects each as corruption before
   write, prompt, classification, replay, or apply, without implicit floor
   promotion or v1 fallback; and
10. the immediate predecessor canary opens a v1-floor Session but explicitly
    refuses a guard-ahead v2-floor Session before canonical read or legacy
    fallback.

Until those gates exist and pass, `audio-graph-4249` and
`audio-graph-48de` remain incomplete, and this design alone authorizes no
writer or scheduler activation.
