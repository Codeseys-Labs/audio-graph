---
status: accepted
date: 2026-07-09
deciders: [AudioGraph maintainers]
---

# ADR-0033: Enforce MVP Provider Enablement at Every Content-Bearing Start

## Context and Problem Statement

The provider registry separates implementation status from the MVP product set
through `MVP_SELECTABLE_PROVIDERS` and each descriptor's `ui_selectable` flag.
The original rollout treated that flag as picker-only: saved deferred settings
remained dispatchable. That permits a legacy configuration, direct frontend
store call, DevTools invocation, or direct Tauri command to start a provider
that the product labels "Not in MVP". For realtime agents and ASR providers,
that can also send audio or transcript context outside the device before the
provider has passed the MVP capture, durability, privacy-route, and recovery
gates.

Saved configurations must remain inspectable and recoverable, and an already
active legacy session must remain stoppable. Those migration needs do not
justify allowing a new content-bearing session to bypass the product gate.

## Decision Drivers

- Make "Not in MVP" true at the IPC boundary, not merely visual copy.
- Prevent stale UI state and direct command invocation from bypassing policy.
- Keep one generated registry decision rather than hand-maintained UI branches.
- Preserve saved configuration inspection, credential maintenance, and health
  diagnostics needed to prepare later provider enablement.
- Preserve stop, cancel, and cleanup for already-active legacy sessions.
- Require explicit provider promotion after content-egress and data-path proof.
- Return structured, recoverable, content-free errors.

## Considered Options

- Keep `ui_selectable` as a picker-only hint and allow all implemented runtimes
- Mirror the MVP registry gate at frontend actions and backend start commands
- Remove deferred provider runtimes and settings variants from release builds

## Decision Outcome

Chosen option: "Mirror the MVP registry gate at frontend actions and backend
start commands", because defense in depth prevents accidental content egress
while retaining implemented adapters and saved settings for later promotion.

For the MVP, membership in `MVP_SELECTABLE_PROVIDERS` governs both new UI
selection and new content-bearing runtime starts. Every start path must resolve
its actual ASR, LLM, TTS, or realtime-agent descriptor and reject a descriptor
whose `ui_selectable` value is false before opening a provider transport or
subscribing it to processed audio.

The backend Tauri command is authoritative. Frontend controls and store actions
mirror the same generated flag to provide immediate, accessible feedback, but
their checks are not a security or policy boundary.

The gate does not block:

- loading, displaying, editing, migrating, or deleting saved provider settings;
- non-content-bearing credential presence and explicitly requested health/model
  diagnostics that already follow the privacy audit contract;
- stop, cancel, disconnect, drain, or cleanup of an active legacy session; or
- deterministic fixtures that invoke provider adapters through an explicit
  test-only harness without real content egress.

Promotion requires the Provider Addition Content-Egress Checklist, capture and
source compatibility, parser and runtime fixtures, privacy-route evidence,
failure/cancellation/recovery tests, and the relevant ADR-0032 evidence tier.
The provider is enabled by changing the single registry table and regenerating
the frontend registry, not by adding a one-off command exception.

Errors use a structured deferred-provider code containing only provider id and
display name. They do not contain credentials, endpoints, source labels,
captured content, or provider response bodies.

### Consequences

- **Positive**: Product copy, controls, store actions, and direct IPC agree on
  which providers can start.
- **Positive**: Persisted legacy settings cannot silently transmit content.
- **Positive**: Later provider promotion remains a one-table, testable change.
- **Positive**: Teardown and configuration recovery remain available.
- **Negative**: A previously working legacy provider configuration can no
  longer start until that provider passes promotion gates.
- **Negative**: Every new content-bearing start command must identify and test
  its descriptor before transport setup.
- **Negative**: `ui_selectable` now carries product-enablement consequences
  beyond rendering, so its name is narrower than its enforced semantics.
- **Neutral**: Implemented deferred adapters stay compiled and testable; this
  decision does not remove provider code.

## Pros and Cons of the Options

### Keep `ui_selectable` as a picker-only hint and allow all implemented runtimes

- Good, because legacy configurations continue to run without migration.
- Good, because backend commands need no additional gate.
- Bad, because "Not in MVP" is false when state bypasses the picker.
- Bad, because direct commands can egress content through unapproved routes.
- Bad, because frontend and backend policy inevitably drift.

### Mirror the MVP registry gate at frontend actions and backend start commands

- Good, because backend enforcement survives stale or hostile frontend state.
- Good, because frontend mirroring gives immediate user feedback.
- Good, because one registry table controls promotion.
- Bad, because saved deferred routes remain inspectable/editable but cannot
  start content-bearing work until promoted.
- Bad, because start-command coverage must remain exhaustive.

### Remove deferred provider runtimes and settings variants from release builds

- Good, because disabled providers cannot be invoked accidentally.
- Good, because the shipped binary has a smaller runtime surface.
- Bad, because saved settings become difficult to inspect or migrate.
- Bad, because provider preparation and fixtures require separate builds.
- Bad, because build-feature state becomes a second product-enable registry.

## More Information

This decision tightens the earlier picker-only interpretation tracked by
`audio-graph-ad56` and `audio-graph-da33`. It works with ADR-0028's session
lifecycle boundary and ADR-0032's validation tiers. Deferred provider promotion
remains subordinate to `docs/designs/provider-architecture.md` and the
content-egress checklist.
