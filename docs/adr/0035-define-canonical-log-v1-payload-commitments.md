---
status: accepted
date: 2026-07-10
deciders: [AudioGraph maintainers]
---

# ADR-0035: Define Canonical Log V1 Payload Commitments with Key-Canonical JSON

## Context and Problem Statement

ADR-0027 requires stable canonical-event identifiers, payload hashes, record
hashes, and replay across application versions. The canonical-log v1 prototype
first converted a typed payload to `serde_json::Value` and hashed
`serde_json::to_vec` directly. That representation can retain object insertion
order when the dependency graph enables `serde_json/preserve_order` and sort
keys when it does not. JSON object order is not semantic, so a transitive Cargo
feature change could make an unchanged stored event fail its payload hash.

The v1 commitment encoding must be deterministic before any runtime writer can
produce forward-only framed data. It also needs immutable fixtures that prevent
the writer and reader from drifting together unnoticed.

## Decision Drivers

- Replay the same event across supported operating systems and dependency
  feature graphs.
- Treat JSON object member order as non-semantic while preserving array order.
- Keep typed Rust payloads and human-inspectable JSON in the MVP store.
- Detect on-disk format drift independently of the current implementation.
- Avoid adding a new serialization dependency before the first runtime reader
  migration.

## Considered Options

- Hash and store the serializer's current object order.
- Recursively key-canonicalize JSON objects before v1 storage and hashing.
- Adopt RFC 8785 JSON Canonicalization Scheme immediately.
- Commit a stable binary encoding while storing JSON only as a display copy.

## Decision Outcome

Chosen option: "Recursively key-canonicalize JSON objects before v1 storage
and hashing", because it removes the demonstrated Cargo-feature dependency,
keeps the current typed JSON contract, and can be frozen with exact byte and
hash fixtures without introducing another format implementation.

Canonical log v1 sorts every JSON object recursively by key before the payload
is placed in the wire envelope or hashed. Arrays retain their original order;
scalar values retain the encoding produced by the repository-pinned
`serde_json` version. JSON text with duplicate object keys is never a valid
canonical v1 input: typed writers cannot produce it, and framed readers reject
it rather than choosing a first- or last-key interpretation. Readers normalize
valid object order before payload-hash validation and typed decoding; they do
not reject a record only because its object members arrived in a different
order.

The v1 test corpus stores exact frame bytes, payload hashes, record hashes, and
stream heads. It runs with `serde_json/preserve_order` deliberately enabled so
the normalizer, rather than a sorted map implementation, supplies the proof.
Golden fixtures may be re-blessed only when a reviewed decision either fixes a
fixture error without changing the v1 contract or introduces a new format
version plus a compatibility reader. A serializer upgrade that changes v1
scalar, string, or number bytes requires a new canonical-log format version; it
must not silently rewrite v1 expectations. No framed canonical v1 runtime data
has shipped, so this decision freezes the contract before the first runtime
writer is authorized.

### Consequences

- **Positive**: Semantically equivalent JSON objects have one v1 commitment
  regardless of insertion order or `preserve_order` feature unification.
- **Positive**: Golden bytes and hashes detect coordinated writer/reader drift.
- **Positive**: Existing typed payload decoding and human-readable artifacts
  remain intact.
- **Negative**: Append and replay perform recursive normalization and may
  allocate another payload-sized value before writing or hashing.
- **Negative**: V1 still freezes the pinned `serde_json` scalar/string encoding;
  adopting full RFC 8785 semantics later requires a new format version.
- **Negative**: Applications cannot use object insertion order as payload
  meaning inside a v1 canonical event.
- **Negative**: Raw JSON producers must reject duplicate object keys before
  constructing a canonical v1 event.
- **Neutral**: Array order remains semantic and is not reordered.

## Pros and Cons of the Options

### Hash and store the serializer's current object order

- Good, because it adds no normalization work.
- Good, because the exact input order remains visible.
- Bad, because transitive `serde_json` features can change the commitment.
- Bad, because JSON object order becomes accidental application semantics.

### Recursively key-canonicalize JSON objects before v1 storage and hashing

- Good, because it is deterministic under both ordered and sorted map builds.
- Good, because it preserves the existing JSON and typed payload model.
- Bad, because it adds recursive work and allocation on append and replay.
- Bad, because scalar canonicalization remains coupled to a pinned serializer.

### Adopt RFC 8785 JSON Canonicalization Scheme immediately

- Good, because it provides a published cross-language canonicalization rule.
- Good, because it defines object ordering and number serialization together.
- Bad, because it adds a new implementation or dependency to the MVP storage
  trust boundary.
- Bad, because current Rust domain payloads and fixtures have not been audited
  for every RFC 8785 number and Unicode edge case.

### Commit a stable binary encoding while storing JSON only as a display copy

- Good, because a binary schema can provide strong language-neutral stability.
- Good, because integrity need not depend on JSON serialization behavior.
- Bad, because canonical authority and the human-readable artifact can diverge.
- Bad, because it introduces schema tooling and migration cost before the file
  store's existing durability blockers are closed.

## More Information

- Governing storage decision: ADR-0027.
- Validation evidence policy: ADR-0032.
- Implementation Seed: `audio-graph-b481`, child of `audio-graph-90f3`.
