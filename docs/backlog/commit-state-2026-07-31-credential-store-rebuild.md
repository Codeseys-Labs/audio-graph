# Commit state: credential-store rebuild

Date: 2026-07-31

Coordinating Seed: `audio-graph-a0f6`

Architecture Seed: `audio-graph-efeb`

## Clean architecture worktree

- Worktree: `/tmp/audio-graph-wt-efeb`
- Branch: `work/audio-graph-efeb-credential-architecture`
- Base HEAD: `f97e19c251e4c227aade1289b2aba56e0d40ffca`
- Base relationship: `master` and `origin/master` were equal when this worktree was created.
- Purpose: hold only the credential threat model, superseding ADR, implementation plan, and discovery evidence selected for the repository.

## Dirty integration checkout

The primary checkout at `/home/codeseys/DevBox/audio-graph` was already broadly dirty before this run. At frame time it contained 80 modified tracked files and 33 untracked entries spanning MVP capture, persistence, projection, provider, frontend, documentation, and Seeds work. Those changes are user/integration work and must not be swept into this branch.

The core `src-tauri/src/credentials/mod.rs`, `src-tauri/src/settings/mod.rs`, `src-tauri/src/aws_util/mod.rs`, and endpoint-credential-routing contract were unchanged from `f97e19c` at discovery time. Credential consumers in `commands.rs`, provider-registry code, and frontend settings overlap newer dirty integration behavior and therefore require semantic integration rather than blind file replacement.

No product code, staging, commit, push, workflow edit, or `sd sync` has been performed in the dirty checkout during the architecture phase.

## Active queue

- `audio-graph-a0f6`: rebuild the credential store as a backend-owned secret service.
- `audio-graph-efeb`: threat model, contracts, migration, ADR, and worktree plan.
- `audio-graph-cffc`: block renderer-directed saved-credential egress to untrusted endpoints.
- `audio-graph-f70b`: preserve legacy OpenAI Realtime inline credentials during migration.
- `audio-graph-873d`: make AWS credential replacement and deletion one atomic typed bundle.
- `audio-graph-c420`: stop collapsing credential-backend failures into missing credentials.
- `audio-graph-98a9`: implement immutable protected origin bindings for saved custom credentials; custom routes are draft-only until it lands.
- `audio-graph-16fc`: decide AudioGraph versus A.R.I.A. naming independently from technical storage identity.

The previously identified MVP persistence/release sequence remains recorded on `audio-graph-99eb`; the credential focus does not close or supersede those Seeds.

## Discovery evidence

Read-only cartographer reports are staged outside the repositories while architecture is synthesized:

- `/tmp/audio-graph-credential-discovery/backend-map.md`
- `/tmp/audio-graph-credential-discovery/frontend-consumer-map.md`
- `/tmp/audio-graph-credential-discovery/security-platform-map.md`
- `/tmp/audio-graph-credential-discovery/native-store-research.md`

The selected Tauri and Rust library decisions are durable in
`docs/research/2026-07-31-credential-service-library-evaluation.md`.

The discovery baseline found a release-blocking saved-key egress path, hidden-write presence queries, non-atomic credential bundles, fail-open backend reads, incomplete legacy migration, stringly typed error/status contracts, long-lived secret hydration in settings, and product-name coupling in the keychain service and filesystem paths.

## Known verification

The read-only discovery lanes reported:

- focused Rust credential suite: 49 passed, 0 failed, 1 ignored real-keychain smoke;
- endpoint credential-routing contract checks: passed;
- provider-registry contract checks: passed;
- Rust formatting and documentation secret-hygiene checks: passed;
- frontend TypeScript/Vitest discovery gates were not runnable in the clean baseline because frontend dependencies were absent; no dependency installation was attempted.

These results describe the pre-rebuild baseline only. They do not establish native-store behavior in packaged Windows, macOS, or Linux applications.

## Guardrails

- Rust owns secret storage, resolution, provider binding, migration, and long-lived provider use.
- React receives only typed non-secret status and mutation receipts; it never receives stored plaintext.
- OS credential storage remains the production desktop default.
- Legacy `credentials.yaml` becomes import-only; any live file backend is explicit and separate.
- Display branding never silently changes keychain service, account, bundle-id, or filesystem identity.
- Write-capable implementation workers use one clean worktree each.
- CI/workflow changes remain approval-gated.
- Maximum one adversarial review/fix round per implementation wave before conductor backflow.
