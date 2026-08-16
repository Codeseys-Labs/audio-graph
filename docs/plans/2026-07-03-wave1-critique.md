# Wave-1 Critique — P6 Adjudication (2026-07-03)

Adjudicator: P6. Inputs: 9 raw findings from 4 lens reviewers across three
feature branches. All claims re-verified READ-ONLY against the branch snapshots
the reviewers named (none of these branches are merged to master yet):

- `fix/deepgram-flux-alias-and-401-cache` @ `db79129` (PR #39)
- `feat/session-data-movement-ledger-70a3` @ `5ff2c7b` (PR #37)
- `feat/openrouter-routing-telemetry-76bd` (PR #40)

Ranking key: severity 1 (worst) → 4 (trivia); blocking weights above severity.

---

## KEPT — seedworthy

### K1 — delete_credential_cmd leaves stale key in the settings cache (asymmetric with save-path fix)  [sev 2, BLOCKING]

VERIFIED at `db79129`:
- `save_credential_cmd` (commands.rs ~6365-6373) re-hydrates `state.app_settings`
  from the reloaded store after `set_credential`.
- `delete_credential_cmd` (commands.rs ~6384) calls `delete_credential` +
  `bump_provider_credential_epoch` and returns — **no cache re-hydrate**.
- `read_settings_for_session_content` (commands.rs ~1258-1270) clones the cached
  `app_settings` verbatim for a new capture session.

Failure scenario: user revokes/deletes a Deepgram key in Settings, starts a new
session without restarting → the cached `AppSettings` still holds the deleted
key → the session transmits a key the user believes removed (401, or continued
use of a revoked credential). This is the exact inverse of the stale-cache 401
the PR fixed for the save path; the two symmetric writers now diverge. The
readiness chip reads its own store snapshot (commands.rs ~7988), so the UI shows
"no key" while the session silently still uses the old one — the divergence is
invisible in the UI, which makes it worse, not better.

Why kept + blocking: this is a live correctness/trust regression introduced by
the same PR, on the same hot path, reachable by a normal user action. It is the
strongest finding in the set and the cheapest to fix (mirror the save-path
block, or extract a shared helper).

Acceptance: `delete_credential_cmd` takes `State<AppState>` and re-hydrates
`app_settings` from the reloaded store after delete succeeds (identical to the
`save_credential_cmd` block), OR a shared helper does the store→cache re-hydrate
for both writers. A test asserts the cached provider `api_key` is cleared after
a delete.

---

### K2 — `flux-general` (bare stem) passes validation and hits a guaranteed Deepgram 400  [sev 3, non-blocking]

VERIFIED at `db79129`:
- `upgrade_deepgram_model_alias` (deepgram.rs ~665) rescues only the exact
  string `"flux"`.
- `is_valid_deepgram_streaming_model` (deepgram.rs ~687) accepts ANY value
  starting with `flux-` of length > 5.
- v2/listen's enum only accepts `flux-general-en` / `flux-general-multi`.

Failure scenario: user types the very plausible shared stem `flux-general` (the
common prefix of both valid ids). It is not an alias, so it is not upgraded; it
passes the `flux-*` prefix validity check; it is sent verbatim to v2/listen →
400. Same failure class the fix eliminated for bare `flux`, one level down.

Why kept, why non-blocking: real, reachable via ordinary typing, and directly
adjacent to the fix's stated goal. Downgraded from blocking because the PR's own
400-classification arm (deepgram.rs ~591) surfaces an actionable "reselect a
model" toast — degraded UX, not a silent hang or data loss.

Acceptance: tighten `is_valid_deepgram_streaming_model` to accept only the
concrete flux ids (`flux-general-en`/`flux-general-multi`), OR extend the alias
table (`flux-general` → `flux-general-en`). Add a test that `flux-general`
either sanitizes to a valid id or is rejected before the wire.

---

### K3 — ArtifactRef.path_hash contract says sha256 but the producer emits a 64-bit DefaultHasher (`h64:`)  [sev 3, non-blocking]

VERIFIED at `5ff2c7b`:
- `ArtifactRef` doc (session_data_movement.rs:205): "Hex SHA-256 of the artifact
  path/uri (`\"sha256:<hex>\"`)". `sample_event()` uses a `sha256:` literal.
- The only real producer `hash_artifact_path` (persistence/data_movement.rs:38)
  emits `format!("h64:{:016x}", DefaultHasher)`. Its own inline comment even
  says it "documents the algorithm honestly rather than masquerading as
  `sha256:`" — the struct doc + sample were simply never updated to match.

Failure scenario: the generated TS type is the chain-root contract for the
privacy-report frontend (seed 51e0). A downstream consumer that trusts the doc
and parses/labels `path_hash` as sha256, or correlates it against a sha256
computed elsewhere, silently mismatches. A shipped contract lie to a sibling
seed.

Why kept, why non-blocking: it will actively mislead the sibling seed's author,
and the fix is a one-line doc + sample correction. Non-blocking because it is a
comment/schema-doc mismatch (no runtime consumer exists yet) and trivially
correctable before 51e0 lands.

Acceptance: make doc match impl — change the `ArtifactRef.path_hash` doc + the
`sample_event()` literal to describe the `h64:` fingerprint (or switch the
producer to real sha256). The schema comment, `sample_event`, and the emitted
prefix must agree.

---

### K4 — OpenRouter fallback_from_preferred false-positive when slug ≠ display name  [sev 3, non-blocking]

VERIFIED at `feat/openrouter-routing-telemetry-76bd`:
- `fallback_evidence` (openrouter.rs:771-776) is
  `Some(!preferred.eq_ignore_ascii_case(served))`.
- `preferred` is a provider SLUG (from `acceleratorProviderOrder`); `served` is
  the response-body `provider` DISPLAY NAME. The neighbouring `MAX_METADATA_LEN`
  comment (openrouter.rs ~783) itself names `"Amazon Bedrock"` as a real display
  name, confirming names with spaces exist.

Failure scenario: routing order `[amazon-bedrock]`, OpenRouter serves via Amazon
Bedrock and echoes `provider: "Amazon Bedrock"` → `eq_ignore_ascii_case` is
false → `fallback_from_preferred = Some(true)` even though the PREFERRED
provider served the request. A false fallback positive in the exact signal the
struct exists to produce. Single-token providers (cerebras/Cerebras, groq/Groq)
where slug == lowercased-name are the only cases the tests exercise. The repo's
own frontend catalog already reconciles slug↔name drift (docs backlog note);
this backend telemetry does not.

Why kept, why non-blocking: it is a genuine wrong-signal bug on the primary
output field, verified against the code. Non-blocking because the struct is a
declared chain-root for seed 713c and is not yet surfaced or persisted anywhere
— nothing consumes the wrong value today, but 713c will inherit it if unfixed.

Acceptance: `fallback_evidence` normalizes both sides beyond ASCII case (fold
spaces/underscores↔hyphens, or reconcile slug↔display-name the way the frontend
catalog join does) so a served provider matching the preferred under the
project's own equivalence yields `Some(false)`. Add a unit test with a
hyphenated-slug vs spaced-display-name pair (`amazon-bedrock` vs
`"Amazon Bedrock"`) asserting `Some(false)`.

---

## DROPPED — with reasoning

### D1 — "Core re-hydrate-on-save fix has zero test coverage" (raw #2)  [was sev 2, non-blocking]

DROPPED as a standalone seed; FOLDED into K1's acceptance. The observation is
accurate (grep confirms only comments, no assertion exercises save→cache→read),
but it is a test-coverage gap on already-correct behavior, not a defect. The
extractable shared helper it asks for is exactly what K1's acceptance proposes,
and K1 already requires a test asserting the cached key changes. Filing a second
seed for the save-path test would duplicate the delete-path seed's fix. Coverage
should ride along with K1, not as its own backlog item.

### D2 — "looks_secret misses 12-23-char no-prefix secrets" (raw #4)  [was sev 2, non-blocking]

DROPPED. Code claim is correct (`<12` → skip unless "bearer"; catch-all requires
`>= 24`; 12-23 no-prefix falls through). But: (a) `looks_secret`/`redact_message`
is an explicitly-labelled defense-in-depth guard for "carelessly-forwarded
provider error strings" — the schema has no field that *should* hold a secret;
(b) it has ZERO producers (see D3), so no real error string flows through it
today; (c) the fix (lower threshold + entropy gate) risks over-redacting prose,
i.e. trades a latent hypothetical for a live readability regression. This is a
hardening idea for a not-yet-live guard, not a Wave-1 defect. If producers land
(seed d598) and any of them forward provider error text, revisit then — but it
belongs with that seed, not as an independent backlog item now.

### D3 — "Ledger has zero production producers — dead code" (raw #7)  [was sev 3, non-blocking]

DROPPED. Verified true (every caller is under `#[cfg(test)]`). But the commit
message explicitly scopes this PR as "backend ledger + schema only; UI is 51e0;
runtime policy/producers is d598". This is documented, honest scope, not a hidden
regression — nothing that shipped is broken because nothing consumes it. The
raw finding's own acceptance is "tracked follow-up seeds wire the producers,"
i.e. it is already planned work, not a Wave-1 finding. Not seedworthy.

### D4 — "path_hash is recomputable by a ledger holder (not one-way)" (raw #6)  [was sev 3, non-blocking]

DROPPED. Reasoning is sound: `DefaultHasher` (SipHash, fixed zero keys) is
deterministic + 64-bit, and `session_id` is stored plaintext in the same event,
so an attacker holding the ledger can reconstruct-and-hash candidate paths. BUT
this is a design-framing disagreement about a component with no producers and no
real data yet, and it partly conflicts with K3 (K3 wants the doc to describe the
`h64:` fingerprint honestly; the producer's own comment already reframes it as
"redaction display token, not integrity"). Resolving K3 (doc says `h64:` opaque
fingerprint, drop any "cryptographic/one-way" implication) neutralizes the
consumer-misuse risk this raises. Keeping both would be redundant. The residual
"salt with a per-install secret" ask is speculative hardening for empty files —
belongs with the producer seed (d598) if privacy-of-layout is ever a stated
requirement. Folded into K3's framing; not a separate seed.

### D5 — "Append-only ledger has no rotation / size bound" (raw #8)  [was sev 4, non-blocking]

DROPPED. Correct that `append_data_movement_event` → `append_jsonl` is pure
append+fsync with no cap. But it is explicitly latent: no producers emit events,
and the file is deleted with the session. The raw finding itself says
"non-blocking today" and scopes the fix to "before/with the producer seed." This
is tech-debt to attach to d598 (the producer seed) where the growth rate becomes
real and measurable, not a Wave-1 backlog seed against empty code. Sev-4 latent
tech-debt on a component with no live writers does not clear the "real failure
scenario" bar for this wave.

---

## Adjudication summary

- 9 raw findings → 4 kept as seeds, 5 dropped/folded.
- The single blocking item (K1) is a genuine correctness regression the PR itself
  introduced (writer asymmetry on the credential hot path).
- The other 3 kept items (K2 flux-general 400, K3 sha256/h64 contract lie, K4
  OpenRouter fallback false-positive) are real, verified, low-cost-to-fix defects
  that will mislead users or sibling seeds if unfixed, but none block today.
- The 5 dropped findings cluster around the data-movement ledger's not-yet-wired
  state (D2/D3/D4/D5) and a test-coverage gap (D1). They are honest,
  pre-documented scope boundaries or hardening ideas that belong with the
  downstream producer seed (d598), not this wave. Filing them now would create
  backlog against dead code.
