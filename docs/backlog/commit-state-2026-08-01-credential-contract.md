# Credential Contract Worktree State

Date: 2026-08-01

Seed: `audio-graph-e11c`

Branch: `work/audio-graph-e11c-credential-contract`

Base and initial HEAD: `a6a436313258e8489065a644f906759d3494abfb`

## Custody

- The primary `master` checkout is broadly dirty user WIP and remains custody-only.
- This clean worktree is the only write surface for the credential-contract slice.
- No dirty-main file will be copied, staged, reset, committed, or synchronized here.
- Seeds are updated from the primary checkout, but `sd sync` is intentionally deferred because it would sweep unrelated work.

## Active scope

Generate one exhaustive Rust-owned credential-domain contract and a checked, plaintext-free TypeScript projection. The contract must cover current credential sets and consumers, auth alternatives, field classification, purpose and audience authority, passive redacted status, safe error codes, revisions, and mutation receipts.

Explicit acceptance includes one atomic AWS credential bundle, explicit Gemini authentication alternatives, mechanical coverage of the current credential vocabulary, and drift/registry/routing/typecheck/formatting/secret-hygiene gates.

## Known baseline evidence

- Integration head `a6a4363` is independently reviewed SHIP.
- Locked metadata resolves rsac 0.4.4 at `ea2019bba217cab695d45696bc2ca25430b23dc2`.
- Serialized Linux cloud Rust tests: 1,458 passed, 0 failed, 7 ignored.
- Credential/settings frontend tests: 154 passed.
- IPC routing contract: 17 passed.
- Windows/macOS capture and release-dry-run evidence remains tracked by `audio-graph-fd9f` and is not claimed by this slice.

## Stop conditions

- Stop and update Seeds if the current credential vocabulary cannot map without changing runtime behavior.
- Do not add native-store, migration, service-lifecycle, renderer mutation, or CI implementation owned by later credential-v2 Seeds.
- Do not generate or expose any plaintext secret readback DTO.
