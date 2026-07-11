---
status: accepted
date: 2026-07-09
deciders: [AudioGraph maintainers]
---

# ADR-0029: Gate Rebuildable Query Indexes on Measured Demand

## Context and Problem Statement

ADR-0027 makes versioned session files the only canonical MVP store. AudioGraph
may later need indexed full-text search, graph traversal, vector recall, or
cross-session temporal queries, but the current product primarily loads bounded
whole sessions and folds them in memory. Adding an embedded database before a
real query exceeds that path's UX budget would duplicate sensitive data and
create migration, packaging, repair, and deletion obligations without measured
user value.

The SurrealKV and RocksDB experiment proved three-OS build and link feasibility
and promising keyed throughput. It did not prove a production adapter,
transactional concurrent sequencing, schema migration, backup, corruption
recovery, or lifecycle parity. Engine feasibility is not a demand signal.

## Decision Drivers

- Keep canonical capture and review independent from optional query machinery.
- Require a named product query and measured latency problem before adding cost.
- Make index corruption or lag repairable without data loss.
- Preserve deletion, privacy, retention, backup, and restore parity.
- Select an engine from the actual workload rather than prior experimentation.
- Avoid permanent three-OS packaging complexity for unused capability.

## Considered Options

- Add a rebuildable index only after measured product demand
- Ship a SurrealKV derived index as part of the MVP
- Never add an embedded query index

## Decision Outcome

Chosen option: "Add a rebuildable index only after measured product demand",
because it preserves the option to accelerate real cross-session workflows
without speculating on an engine or creating a second source of truth.

A query-index proposal may proceed only when:

1. a committed cross-session product query and representative corpus are named;
2. canonical file replay exceeds an agreed and measured UX latency or memory
   budget;
3. the proposed index has a versioned schema and deterministic full rebuild;
4. index absence, lag, corruption, or rebuild never blocks canonical capture,
   review, export, or deletion;
5. privacy class, retention, backup, restore, and zero-residual deletion parity
   are proven;
6. three-OS packaging, crash, upgrade, and rollback tests pass; and
7. the engine is selected from measured query, size, write-amplification, and
   operational results for that workload.

Any accepted index is disposable and derived from ADR-0027 canonical streams.
It never participates in canonical Accepted acknowledgement. Its stored head
vector identifies exactly how far it has caught up, and a mismatch triggers
rebuild or bounded catch-up rather than authority arbitration.

SurrealKV, SQLite, redb, and other engines remain candidates. No engine is
preselected by this decision.

### Consequences

- **Positive**: Canonical durability and MVP delivery remain independent of a
  database engine.
- **Positive**: Future engine selection is grounded in a real query and corpus.
- **Positive**: Index replacement is a rebuild, not a canonical data migration.
- **Negative**: Cross-session search can remain slower until demand and budgets
  are instrumented.
- **Negative**: The team must build measurements and a deterministic index
  rebuild before shipping the feature.
- **Negative**: A derived index still duplicates sensitive data and therefore
  needs complete privacy, retention, and deletion coverage.
- **Neutral**: Existing engine experiments remain useful evidence but do not
  authorize production selectability.

## Pros and Cons of the Options

### Add a rebuildable index only after measured product demand

- Good, because the actual query determines schema and engine selection.
- Good, because capture never depends on index availability.
- Good, because corruption recovery is deterministic rebuild.
- Bad, because users do not receive indexed cross-session recall immediately.
- Bad, because instrumentation and a representative corpus become prerequisites.

### Ship a SurrealKV derived index as part of the MVP

- Good, because indexed query work can start immediately.
- Good, because basic three-OS build feasibility has been demonstrated.
- Bad, because the production adapter and query workload are not defined.
- Bad, because duplicated data, migrations, packaging, and deletion expand MVP
  risk.
- Bad, because selecting SurrealKV now anchors the schema to an unmeasured use.

### Never add an embedded query index

- Good, because storage has one representation and the smallest operational
  surface.
- Good, because there is no index lag or rebuild state.
- Bad, because useful cross-session search may exceed file replay budgets.
- Bad, because the application would accumulate hand-built scans and traversal
  code as demand grows.

## More Information

This decision is subordinate to ADR-0027: query data is always derived from the
file-canonical session store. `audio-graph-21c4` tracks the demand and
rebuildability gate. The underlying evidence is summarized in
`docs/research/mvp-storage-audit-2026-07-09.md`.
