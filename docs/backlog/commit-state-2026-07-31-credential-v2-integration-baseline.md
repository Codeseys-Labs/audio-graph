# Commit state: credential v2 semantic integration baseline

Date: 2026-07-31  
Seed: `audio-graph-59d1`  
Integration branch: `work/audio-graph-cred-v2-integration`  
Exact base: `f97e19c251e4c227aade1289b2aba56e0d40ffca`

## Purpose and custody boundary

This record is the semantic bridge between the clean credential-v2 worktree
wave and the broadly dirty primary checkout at
`/home/codeseys/DevBox/audio-graph`. It records Git blob identities and the
behavior that later fan-in must preserve; it does not copy any dirty-main
product file.

The integration worktree began clean at the exact base above. The accepted
architecture commits were admitted only after a merge-base footprint check and
landed oldest first:

| Source commit | Integration commit | Contribution |
| --- | --- | --- |
| `0314b8b790d5a16ad961aad7c330c4617cf5f347` | `909744e` | ADR, threat model, discovery, plan, naming, and commit-state docs |
| `147eb6bd4ae1cb88fb4ecdcaa86b9ac7047203fb` | `03f7217` | review-gap closure and Tauri/Rust library evaluation |
| `60cf97993e92592296ab79c6d7a69b036e254324` | `456b678` | ADR acceptance and index update |

Their merge-base contribution is eight documentation files and 1,549
insertions, with no product code, generated output, dependency, lockfile,
vendored artifact, binary, or deletion.

The hashes below are Git blob object IDs. `clean` means the primary checkout
matched the base when sampled. `untracked` means no base blob exists. Any later
hash mismatch is drift and requires a fresh semantic comparison before apply.

## Dirty-main overlap inventory

| Path | Status and blob identity | Relevant semantic hunk and invariant | Future owner and apply strategy |
| --- | --- | --- | --- |
| `src-tauri/src/commands.rs` | modified; base `971c66183761fda249f12ca553dcf35fcb93a1a8`; dirty `b84960449fed1f3591554cf7cbf7bfe2b3420c36` | The v1 save/delete/presence and endpoint-key blocks are base-identical. Dirty credential-adjacent hunks add provider-start gates, session-worker fencing, explicit readiness request sets, caller-scoped probing, injected-store OpenRouter catalog helpers, and AWS profile-path tests. Preserve the invariant that product enablement is checked before client/runtime setup, passive Settings hydration does not probe a persisted deferred route, and a missing saved credential fails before network I/O. | `audio-graph-cffc` owns P0 destination authorization; `audio-graph-f107` owns v2 IPC/lifecycle; `audio-graph-54e7` owns runtime leases. Never copy this 1,300-line dirty delta. Assemble narrow owner diffs, then preserve the provider gates/request scoping and run hostile-origin, deferred-probe, OpenRouter-missing-key, command-registration, and session-fencing tests. |
| `src-tauri/src/state.rs` | modified; base `7361892dec0a89367315b07418c9b32af7dd0733`; dirty `fcb9101514288c301a7ea2b4e1203f34c130037e` | Dirty main changes audio pipeline message types and adds `session_lifecycle` plus `retired_session_workers`; it does not add the v2 service. Exactly one initializer must exist for every `AppState` field, and credential rotation must not weaken the session-worker fence. | `audio-graph-f107`. Apply only the managed `Arc<CredentialService>` slice after the lifecycle work is accepted; compile all manual `AppState` initializers and check for duplicate fields/initializers. |
| `src-tauri/src/lib.rs` | modified; base `35ea26aac73b944a87bddb046d684bfe45ae5e86`; dirty `94dc610ff7c57688d481a8dfc0c2b6e5127aadc9` | Dirty main expands shutdown handles and capture-stop audit behavior. Existing credential command registration/startup hydration remains inherited from base. The assembled app must manage one service instance and register each v2 command once without removing shutdown cleanup. | `audio-graph-f107`. Patch managed state and the invoke handler structurally; assert one service registration, no duplicate command names, and preservation of shutdown-handle construction/tests. |
| `src-tauri/src/error.rs` | modified; base `9c8b62e0357d5de574ffd2d3a2f4321489083439`; dirty `7d718a91706883270965f5202f859c45a30494ae` | Dirty main adds the content-free `ProviderDeferred` variant and serialization tests. V2 credential failures must remain closed/tagged and must not collapse or alter provider-deferred payloads. | `audio-graph-f107` after the shared contract. Add credential variants beside, not instead of, provider errors; rerun both provider and credential serialization/redaction tests. |
| `src-tauri/src/settings/mod.rs` | clean; base and dirty `d90faf9222c219b7f1bb71a7c90dffbceef854ef` | No dirty-main overlap. This is the current inline extraction/redaction, runtime hydration, and settings persistence authority. Old inline shapes must remain readable until verified v2 import, while plaintext must not be re-persisted. | `audio-graph-86e9` owns legacy import; `audio-graph-f107` owns the backend settings-plus-secret transaction bridge. Start from the clean blob and replace authority only behind migration/fault fixtures. |
| `src-tauri/src/asr/cloud.rs` | clean; base and dirty `a3cfa91d2cb37de4a8fc1b17d4121827dd5e8413` | No dirty-main overlap. Cloud ASR currently consumes hydrated/draft material. Future saved-key use must be purpose- and exact-audience-bound and redirect-safe. | `audio-graph-54e7`. Replace resolution with a scoped lease; retain provider error redaction and add denial/no-request plus redirect tests. |
| `src-tauri/src/llm/api_client.rs` | clean; base and dirty `301f7fa9fe231eaba98c2137e4dcab8a626954c3` | No dirty-main overlap. Generic OpenAI-compatible clients are a load-bearing custom-endpoint boundary. A saved key may never be selected from arbitrary endpoint text. | `audio-graph-cffc` hardens origin admission first; `audio-graph-54e7` later consumes scoped leases. Start from the clean blob, disable redirects or reauthorize every hop, and keep custom endpoints draft-only until immutable binding exists. |
| `src-tauri/src/llm/openrouter.rs` | clean; base and dirty `f570a9aeeef81c9ffe08b7fd1fb4238a72700f82` | No dirty-main overlap. Catalog/connection functions accept an explicit key and base URL; command helpers decide whether saved material reaches them. | `audio-graph-cffc` then `audio-graph-54e7`. Keep transport functions explicit, move saved-key authorization to the backend service boundary, and test exact OpenRouter origin plus cross-origin redirects. |
| `src-tauri/src/llm/streaming.rs` | modified; base `44868e78a671f150a84988b90e26354767b92062`; dirty `80a3d476a18f11a5ae3a93128ba703fa95ae423e` | Dirty main adds `StreamRegistry::is_empty()` so session rotation can reject while background output remains. A credential revision fence must compose with, not bypass, that lifecycle check. | `audio-graph-54e7`. Add revision-aware leases/cancellation around the existing registry and preserve the `is_empty` session invariant. |
| `src/components/settings/useSettingsController.tsx` | modified; base `6332904e8f969c81bf0f3ebe7af4d959c5fbdb86`; dirty `2f28f87c23f8eaabbf49ee779fe883946c510d2f` | Dirty main derives explicit readiness provider IDs, filters automatic mount probes to `ui_selectable` routes, allows deferred diagnostics only by explicit user action, and guards non-selectable mode-card actions. Existing saves still loop over scalar credentials. Preserve passive/no-egress hydration, request cancellation, and explicit diagnostic intent. | `audio-graph-2c33`. Replace scalar loops with typed bundle mutations and revision-aware caches while retaining the automatic/manual probe split; semantically port tests instead of copying the controller wholesale. |
| `src/components/settings/CredentialsPanel.tsx` | modified; base `b326832b2bf630f792350f99e9aad3bf8c14c1d6`; dirty `d5f86a77b3d8668f535cbbbfa1aaba010cb2631b` | Manual Run checks/Retest now opts into deferred diagnostics. The action is user initiated and must remain status-only: no stored plaintext crosses IPC. | `audio-graph-2c33`. Keep explicit intent, bind it to typed provider/set status, and surface locked/conflict/recovery states without a generic saved-secret getter. |
| `src/components/ExpressSetup.tsx` | modified; base `cca7380b6fbb0468fdbc099312150226486c069f`; dirty `40faee32c47272786981189e7f35a8cc2ed7a482` | Dirty main removes deferred Anthropic/Gemini-Live choices, reads credential presence without provider readiness on mount/focus, drops stale async responses, and keeps only registry-selectable MVP routes. Its save path still performs credentials then settings as separate calls. | `audio-graph-cae3`. Preserve presence-only passive behavior, selectable-route gating, submission serialization, and draft clearing; replace the split save with the backend prepare/commit transaction. |
| `src/components/SettingsPage.test.tsx` | modified; base `05a76caf07bd3374cd8825e9286ee37491a07071`; dirty `3b2a4bd11eb8f0b3e879aaef0bb0ea7cbfeedb6a` | Dirty tests encode explicit readiness `providerIds`, honest deferred cards, no plaintext loadback, manual deferred diagnostics, and credential replacement routing. | `audio-graph-2c33`. Port assertions to typed v2 status/revisions while preserving the passive/manual distinction; do not snapshot-copy tests that assert the scalar v1 protocol. |
| `src/generated/endpointCredentialRouting.ts` | clean; base and dirty `209ae4df419159e91e1e2458872df4e291093df2` | No dirty-main output drift. This is generated, never an independent edit target. Rust remains canonical and Rust/TypeScript entry counts and fields must match. | `audio-graph-cffc` for the P0 authorization projection, then `audio-graph-e11c` for the shared contract. Change Rust source, run the generator once, and reject hand-edited output. |
| `scripts/generate-endpoint-credential-routing.mjs` | modified; base `9f59cf8b35c885096311558500b5860f37402093`; dirty `2efb270011a9b4227a45b305df3c9494d0e5257b` | Dirty main changes Windows invocation from `cargo.cmd` to `cargo.exe` and adds `--locked`. These toolchain semantics are unrelated to the routing schema but must survive generator fan-in. | Generator owner for `audio-graph-cffc`/`audio-graph-e11c`. Reapply these two lines after source-contract assembly, then run `check:endpoint-credential-routing`; do not copy generated output from a mismatched manifest/lock snapshot. |
| `src-tauri/crates/ipc-contract/src/endpoint_credential_routing.rs` | clean; base and dirty `3fd87bad5786e73db30924f66a254353b337de68` | No dirty-main overlap. The current table is the Rust source for endpoint credential routing, but its text-based inference is the P0 boundary being replaced. | `audio-graph-cffc`, with later consolidation by `audio-graph-e11c`. Implement explicit provider/purpose/audience authorization here or in its replacement source, regenerate TypeScript, and run hostile URL plus Rust/TS parity tests. |
| `src-tauri/Cargo.toml` | modified; base `7860ef2d02e4565bec1015af2a2058a692ce6270`; dirty `bc5b520b2a93ebc645995e1e33712923d8050161` | Dirty main replaces sibling-path `rsac` dependencies with the same pinned git revision on Linux, Windows, and macOS. This is not credential work, but adapter dependencies will collide in the same manifest. | `audio-graph-fb2b` owns only approved credential dependency stanzas. Land the rsac manifest lane first or 3-way apply minimal dependency additions; assert one rsac declaration per target and no unrelated feature/default change. |
| `src-tauri/Cargo.lock` | untracked; no base blob; dirty `7247d3af82aa0fcad96958c436dd3f209d4d0518` | The 13,323-line dirty lock resolves the rsac git pin and current dependency graph. It is custody evidence, not an accepted integration artifact. | Manifest/lock owner for `audio-graph-fb2b`, coordinated with the rsac lane. Never copy this lock wholesale. Regenerate only after accepted manifests converge, inspect package/source deltas, and prove `--locked` gates. |
| `src-tauri/crates/ipc-contract/Cargo.toml` | clean; base and dirty `9d6bd12aec2b69564d32a520223b2f1ed9d5ea08` | No dirty-main overlap. The contract crate remains independently buildable and is the current generator authority. | `audio-graph-e11c`. Add only contract-required dependencies, if any; retain the focused crate tests and generator binary. |
| `src-tauri/crates/provider-registry/Cargo.toml` | clean; base and dirty `47fa465eaef7729381ca3f2b1bf493dad0ea6dce` | No dirty-main overlap. Provider metadata remains a separate capability source; credential v2 must not introduce one-off UI/provider branches. | `audio-graph-e11c` coordinates schema joins without making this crate a secret store. Preserve independent provider-registry generation/tests. |
| `package.json` | modified; base `15e1f98ab82a8eb224cb2bc949cde7382e6b46c6`; dirty `65e9489e9a6c01a1543b0c6020fe5e6de3ead39a` | Dirty main adds `verify:contracts`, `verify:fast`, and serialized local/focused Vitest aliases. No dependency changed. | Contract/UI integrator. Preserve these script additions when the tooling lane lands; credential work should call existing scripts, not rewrite the manifest. |
| `bun.lock` | clean; base and dirty `ee2de6e5ee5332533875cdc60e8949a7c2d9dff0` | No dirty-main overlap and no credential dependency is required by the backend-owned design. | Frontend owners must avoid lock churn unless a separately approved dependency is necessary. Re-run TypeScript/Vitest using the accepted lock. |

## Assembly invariants

1. Dirty-main hashes are custody markers, not a branch or implicit approval.
2. No product file listed above has been copied into this integration branch.
3. P0 saved-key authorization lands before v2 service/runtime migration. Unknown,
   custom, non-secure, or redirected audiences fail closed and do not receive a
   saved secret.
4. `AppState` owns exactly one long-lived credential service. The renderer gets
   redacted status, receipts, and typed failures, never a plaintext saved-secret
   command.
5. Passive UI reads do not prompt, migrate, write, or contact providers. A
   manual diagnostic is explicit and still constrained by backend audience
   authorization.
6. Generated routing artifacts come from their Rust source. Check generator
   parity and semantic entry/field counts after every drifted-file apply.
7. Manifest fan-in is minimal and lockfiles are regenerated from accepted
   manifests; neither the dirty Cargo manifest nor untracked lock is adopted
   wholesale.
8. A clean textual apply is insufficient. Compile/typecheck and run the full
   affected focused suites after each owner lands.

## Baseline gates

The integration footprint is documentation-only and preserves the code at
`f97e19c`.

- `git diff --check f97e19c..HEAD`: passed after the three architecture
  cherry-picks.
- Working-tree `git diff --check`: passed for this baseline record.
- Architecture blocker recheck: clear at source commit `147eb6b`; ADR-0035 was
  accepted at source commit `60cf979` and integrated as `456b678`.
- The changed architecture/baseline documents pass an isolated copy of the
  docs secret-hygiene scanner with zero findings.
- Repository-wide docs/Seeds hygiene still reports six unchanged baseline
  findings in older Seeds/docs; none is introduced by this branch.
- Endpoint-routing generation, provider-registry generation, and the focused
  IPC test could not be re-resolved in this lockless clean worktree. Online
  resolution was blocked by sandbox DNS; offline resolution rejected the
  already-yanked transitive `spin 0.9.8`. This is a baseline/toolchain failure,
  not a changed-code result. The pre-rebuild discovery snapshot remains 49
  credential tests passed, endpoint-routing/provider-registry checks passed,
  and one native smoke ignored.
- Frontend Vitest/typecheck was not rerun because this clean custody worktree
  has no installed frontend dependencies; no install was performed.

Every product workstream must rerun its focused gates with its accepted manifest
and lock snapshot, then rerun them on this integration branch after semantic
fan-in. No P0 implementation branch or product-code change is part of this
baseline.
