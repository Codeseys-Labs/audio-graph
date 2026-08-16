# audio-graph-52b9 implementation report

## Outcome

Implemented the approved, opt-in native durability evidence workflow at
`.github/workflows/2df3-native-durability.yml`. The workflow is manual-only,
has read-only repository permission, runs the two accepted focused filters on
Linux, macOS, and Windows, and now creates a nonempty evidence preflight before
checkout or other fallible platform work so every matrix cell has an uploadable
artifact when a later step fails.

The initial candidate incorrectly claimed that the always-upload guarantee
covered every failure. Review found that checkout, Rust, cache, or dependency
setup could fail before the evidence directory existed, after which the
summary and upload could also fail without an artifact. Correction round 1
closes that gap and qualifies the original claim to the corrected workflow.

No workflow was dispatched. No branch was pushed. No Blacksmith Testbox,
Docker guest, release, repository setting, secret, or Seed was created,
modified, or closed. Independent review and an explicit later dispatch remain
required before this workflow can produce native platform evidence.

## Assignment and acceptance criteria

- Seed: `audio-graph-52b9`, approval-gated prerequisite of
  `audio-graph-2df3`.
- Exact base: `c69face8155322414eba791cee345ada3209a78a`.
- Branch: `work/52b9-native-durability-workflow-wave7b`.
- Dedicated worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/52b9-native-durability-workflow-wave7b`.
- Owned implementation path:
  `.github/workflows/2df3-native-durability.yml`.
- Owned report path:
  `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-52b9-report.md`.

Acceptance requires one `workflow_dispatch`-only workflow; `contents: read`;
no secret context; exact Linux, macOS, and Windows runners; exact pinned
actions; an input that defaults false and fails Windows closed before platform
work; confirmed Windows-only LABSN use; two exact Rust filters with serial,
nocapture execution; separate native exits and full content-free logs; exact
SHA, platform, toolchain, live fixture filesystem, command, and summary
evidence; always-on per-platform upload with 14-day retention; and no claim
beyond process-crash recovery and outcomes after completed OS barriers.

## Worktree admission

The requested branch and worktree path were confirmed absent before creation.
The exact base object was verified as a commit, then the worktree was created
directly from it. First commands inside the worktree:

```text
$ git rev-parse --show-toplevel
/home/codeseys/DevBox/audio-graph/.worktrees/52b9-native-durability-workflow-wave7b

$ git status --short

$ git rev-parse HEAD
c69face8155322414eba791cee345ada3209a78a

$ git branch --show-current
work/52b9-native-durability-workflow-wave7b
```

The initial status was clean.

## TDD RED

Before creating the workflow, the exact static presence assertion was run:

```bash
test -f .github/workflows/2df3-native-durability.yml
```

Real result:

```text
exit 1
```

The expected workflow was absent, so the RED was exact and did not depend on a
proxy test.

## Implementation

### Trigger, permissions, matrix, and time bounds

The workflow is named `2df3 Native Durability Evidence (Manual)` and has only
the `workflow_dispatch` trigger. It declares only `contents: read`. The matrix
has `fail-fast: false` and exactly these cells:

| OS key | Runner |
| --- | --- |
| `linux` | `blacksmith-4vcpu-ubuntu-2404` |
| `macos` | `blacksmith-6vcpu-macos-15` |
| `windows` | `windows-2025` |

The job timeout is 120 minutes and each focused Cargo step is bounded to 45
minutes. Every Cargo step is continuable; its shell deliberately returns zero
after writing the native Cargo exit so that the second filter runs even when
the first returns nonzero. A later `always()` summary reads both native exits
and supplies the final platform verdict. A step timeout or shell failure also
continues to the second filter; the missing native exit becomes `not_run` and
the summary fails the platform honestly. Before checkout, the first applicable
Unix or Windows step creates `EVIDENCE_DIR` and a nonempty `preflight.txt`.
Both always-summaries also recreate the parent directory defensively before
writing `summary.txt`.

### Exact Cargo execution

Both filters execute from `audio-graph/src-tauri` on every admitted platform:

```text
cargo test --locked -p audio-graph --lib --no-default-features --features cloud canonical_durability -- --nocapture --test-threads=1
cargo test --locked -p audio-graph --lib --no-default-features --features cloud persistence::canonical_crash_harness::tests -- --nocapture --test-threads=1
```

Windows uses `shell: pwsh` for both commands and sets
`AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1`, avoiding the Git-Bash `link.exe`
resolution path. Linux and macOS use Bash. Rust is fixed to the repository pin
for 1.95.0. Checkout, Rust, cache, MSVC, LABSN, and upload actions are full-SHA
pinned to existing repository pins. The checkout SHA
`11bd71901bbe5b1630ceea73d27597364c9af683` is accurately annotated as
`v4.2.2`, not `v5`.

### Evidence contract

Each admitted platform records:

- a nonempty `preflight.txt` before checkout or platform setup;
- checked-out Git SHA, with equality enforced against `GITHUB_SHA`;
- selected runner OS, architecture, name, image identifiers when available,
  and live OS version output;
- verbose `rustc` and Cargo versions;
- the two exact commands;
- a live filesystem probe rooted under the same OS temporary directory family
  used by the crash harness;
- content-free stdout/stderr for each filter that actually starts, stored in a
  nonempty per-filter log;
- separate `canonical_durability.exit` and
  `canonical_crash_harness.exit` files; and
- a summary with both exits, final status, proof boundary, and a derived
  `test_logs=not_run|partial|full` evidence state.

The crash harness itself emits its filesystem observation from the live
fixture parent and content-free length/digest/stage evidence. The workflow does
not dump the process environment.

In the corrected workflow, the upload step is full-SHA pinned, uses
`if: always()`, treats a missing path as an error, retains evidence for 14 days,
and gives every platform/run/attempt a distinct artifact name. Preflight
initialization makes that path nonempty before the first later fallible action.

### Windows legal and audio boundary

`confirm_vb_cable_professional_license` is a required boolean input whose
default is `false`. The first applicable Windows step creates the evidence
preflight; the immediately following license gate overwrites it and the summary
with `REFUSED`, then fails before checkout, LABSN, or platform probing unless
the value is exactly true. The workflow never asks for, accepts, stores, or
logs license keys, files, or other license material.

When confirmed, the Windows cell uses exactly:

```text
LABSN/sound-ci-helpers@d08c889a7bba7d9b1b059f8f76dac4672ea3a9cf # v1.0.4
```

LABSN is not present in the Linux or macOS paths. After it runs, the workflow
enumerates at most 64 present `AudioEndpoint` records and serializes only four
allowlisted fields: status, class, friendly name, and instance ID. A zero-row
inventory fails. This is an ephemeral virtual-device enumeration baseline
only. It makes no capture, playback, default-device, roundtrip, PCM, or `rsac`
claim.

### Supply-chain boundary

The LABSN action source is fixed by commit SHA, but its Windows implementation
downloads mutable `VBCABLE_Driver_Pack43.zip` without an upstream archive hash.
The evidence artifact records that limitation. Pinning the action commit does
not authenticate that downloaded archive. Its admission here is limited to a
privileged, ephemeral GitHub-hosted `windows-2025` runner and the enumeration
purpose above; no persistent runner is used.

The reused apt, Homebrew, and conditional Chocolatey dependency setup also
consults mutable platform package repositories. Full-SHA GitHub Action pins and
Cargo `--locked` do not make those operating-system package acquisitions
hermetic. This workflow is an evidence lane, not a reproducible release build.

### Durability proof boundary

The workflow and every summary state the narrow claim:

```text
process-crash recovery and completed OS-barrier outcomes only
```

The workflow does not simulate sudden power removal and must not be cited as
power-loss proof. On macOS, the harness may produce an explicit typed refusal
when the live fixture is not APFS. On native NTFS, the accepted product
contract is typed pre-mutation refusal for unsupported namespace durability,
not forced mutation past that boundary.

## GREEN verification

### YAML and GitHub Actions syntax

Final commands:

```bash
NO_COLOR=1 actionlint .github/workflows/2df3-native-durability.yml
yq eval '.' .github/workflows/2df3-native-durability.yml >/dev/null
ruby -e 'require "yaml"; doc = YAML.safe_load_file(ARGV.fetch(0), aliases: false); abort "not a mapping" unless doc.is_a?(Hash); puts "ruby_yaml_parse=PASS"' .github/workflows/2df3-native-durability.yml
```

Real result:

```text
actionlint: exit 0, no findings
yq: exit 0
ruby_yaml_parse=PASS
```

The first actionlint iteration correctly rejected a job-level
`${{ runner.temp }}` reference because that context is unavailable there. The
evidence root was changed to the absolute `${{ github.workspace }}` outside
the checked-out `audio-graph/` subtree. The final actionlint result above is
green.

### Static contract assertions

The parsed yq JSON was asserted with Ruby, and raw text deny checks were run
with `rg`. Assertions covered:

- only `workflow_dispatch`;
- the exact required false-default boolean input;
- exact permissions and runners;
- 120-minute job timeout, 45-minute test timeouts, and `fail-fast: false`;
- the exact ordered action set and 40-hex pins;
- Unix and Windows nonempty preflight initialization before checkout;
- the pre-checkout Windows refusal overwrite and Windows-only confirmed LABSN
  condition;
- four continuable Cargo steps with the exact filters, flags, and working
  directory;
- PowerShell and the Windows manifest environment on both Windows tests;
- native exit files, full logs, platform/toolchain/filesystem evidence, and
  proof-boundary text, with truthful missing/partial/full log-state reporting;
- bounded endpoint inventory;
- parent creation in both always-summaries;
- `always()` upload, missing-file error, and 14-day retention;
- no secret context, forbidden trigger, or whole-environment dump.

Real result:

```text
static_contracts=PASS
static_text_guards=PASS
```

## Review correction round 1

### Artifact-ordering RED

Standards and Spec review both blocked the initial candidate on one P1: the
first admitted fallible step was checkout, while evidence initialization lived
after checkout/Rust/cache/dependency setup. The always-summary also assumed its
parent already existed. The unmodified candidate was parsed and checked for
both platform initializers and both summary defenses.

Real result:

```text
REVIEW RED: unix preflight initializer absent before checkout index=1; windows preflight initializer absent before checkout index=1; unix always-summary does not create EVIDENCE_DIR; windows always-summary does not create EVIDENCE_DIR
review_red_exit=1
```

This reproduced the review finding without dispatching the workflow or writing
a test fixture.

### Ordering GREEN and mutation sensitivity

Correction round 1 adds separate Unix and Windows initializers as the first two
matrix steps. Each applicable initializer creates `EVIDENCE_DIR`, writes a
nonempty `preflight.txt`, and validates the file. The false-license Windows
path then overwrites both preflight and summary with `REFUSED` before checkout.
Both always-summaries independently create the parent directory before reading
native exits or writing their verdict.

The parsed static validator was rerun on the corrected document and against
five in-memory mutations. Real result:

```text
ruby_yaml_parse=PASS
static_preflight_contract=PASS
mutation_remove_unix_initializer=REJECTED
mutation_checkout_before_initializers=REJECTED
mutation_remove_windows_preflight=REJECTED
mutation_remove_unix_summary_parent=REJECTED
mutation_remove_windows_summary_parent=REJECTED
checkout_annotation=PASS
```

Removal and reordering therefore fail the test rather than merely matching a
step name. The checkout comment was corrected from an inaccurate `v5`
annotation to `v4.2.2` without changing the action SHA.

Rust product inputs and both Cargo command strings are byte-identical to the
reviewed candidate. Per the correction assignment, the 42-test durability and
11-test crash-harness baselines below were not rerun; repository/static gates
were rerun after the workflow-only command-order correction.

## Review correction round 2

### Log-truthfulness RED

Standards review shipped correction round 1. Spec review found one final P2:
both always-summaries still emitted `logs=full content-free test output`
unconditionally. An early checkout, toolchain, cache, or dependency failure can
correctly leave both native exits `not_run` and create no test log; a failure
between the filters can leave exactly one log. The unmodified round-1 tip was
parsed and evaluated for neither-log, one-log, and two-log behavior.

Real result:

```text
REVIEW P2 RED: unix has no test_logs derivation; unix neither expected=not_run observed=full; unix one expected=partial observed=full; windows has no test_logs derivation; windows neither expected=not_run observed=full; windows one expected=partial observed=full
review_p2_red_exit=1
```

### Log-truthfulness GREEN

Both summaries now inspect the actual
`canonical_durability.log` and `canonical_crash_harness.log` files. A file
counts only when it exists and is nonempty. The emitted state is:

```text
test_logs=not_run  # neither nonempty log exists
test_logs=partial  # exactly one nonempty log exists
test_logs=full     # both nonempty logs exist
```

The previous unconditional full-output line was removed. Status derivation,
native exits, summary failure, legal gating, LABSN use, runners, artifacts, and
Cargo command bodies are unchanged.

The parsed Bash and PowerShell shapes were checked against all four boolean
orientations and against mutations that falsified the partial branch or
weakened either nonempty-file probe. The actual Bash summary was also executed
in isolated, cleaned fixture directories for all four cases. Real result:

```text
ruby_yaml_parse=PASS
static_log_state_contract=PASS
mutation_unix_partial_to_full=REJECTED
mutation_windows_partial_to_full=REJECTED
mutation_unix_remove_nonempty_probe=REJECTED
mutation_windows_remove_nonempty_probe=REJECTED
unix_behavior_neither=not_run
unix_behavior_durability_only=partial
unix_behavior_harness_only=partial
unix_behavior_both=full
```

PowerShell was not installed on the local Linux host, so the Windows branch was
validated through actionlint, YAML parsing, exact branch-shape parsing, and its
four-case truth table rather than local PowerShell execution. Native Windows
behavior remains a later reviewed dispatch concern. Rust product inputs and all
four Cargo step bodies remain byte-identical, so focused/full Rust tests were
not rerun in correction round 2.

### Local Linux focused filters

The local worktree used Rust/Cargo 1.95.0, offline dependency resolution, and
the worktree-local `src-tauri/target`:

```text
rustc 1.95.0 (59807616e 2026-04-14)
cargo 1.95.0 (f2d3ce0bd 2026-03-21)
```

Both exact workflow commands were run sequentially even if the first failed.
Real result:

```text
canonical_durability: 42 passed; 0 failed; 1641 filtered out
canonical_crash_harness: 11 passed; 0 failed; 1672 filtered out
local_canonical_durability_exit=0
local_canonical_crash_harness_exit=0
```

The live crash-harness fixture evidence reported:

```text
platform=linux expected=ext4 observed=ext4 outcome=qualified
```

This is local Linux process-crash/completed-barrier evidence only. The workflow
itself was not run locally and no remote platform conclusion is claimed.

### Five generated contracts and pinned fast gate

The clean worktree reused the exact repository-pinned Seeds dependency
read-only:

```text
@os-eco/seeds-cli@0.4.5
```

Commands:

```bash
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli bun run verify:contracts
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli bun run verify:fast
```

Real result:

```text
audio source contract is current
provider registry is current
session data movement contract is current
endpoint credential routing contract is current
speech span revision contract is current
Checked 174 files. No fixes applied.
TypeScript: PASS
sd ready --format json: parsed (50)
sd blocked --format json: parsed (96)
sd list --format json: parsed (50)
docs/Seeds secret hygiene scan passed: 0 findings
```

Both commands exited 0. No generated contract changed.

Correction round 1 reran the same pinned commands after the workflow/report
edit. Real result remained:

```text
audio source contract is current
provider registry is current
session data movement contract is current
endpoint credential routing contract is current
speech span revision contract is current
Checked 174 files. No fixes applied.
TypeScript: PASS
sd ready --format json: parsed (50)
sd blocked --format json: parsed (96)
sd list --format json: parsed (50)
docs/Seeds secret hygiene scan passed: 0 findings
verify:contracts: exit 0
verify:fast: exit 0
```

Correction round 2 reran both pinned commands after the log-state change. All
five contracts remained current; Biome checked 174 files, TypeScript passed,
Seeds output parsed `50/96/50`, docs-secret hygiene found zero issues, and both
`verify:contracts` and `verify:fast` exited 0.

### Secret, diff, and footprint hygiene

Commands:

```bash
bun scripts/check-docs-secret-hygiene.mjs
betterleaks dir --no-banner --redact .github/workflows/2df3-native-durability.yml
git diff --check
git status --short
```

Pre-report real result:

```text
docs/Seeds secret hygiene scan passed: 0 findings
Betterleaks scanned approximately 16,998 bytes; no leaks found
git diff --check: PASS
?? .github/workflows/2df3-native-durability.yml
```

The generated target was absent before testing. After all Cargo-bearing gates,
cleanup used the explicit manifest and target paths:

```bash
cargo clean --manifest-path "$PWD/src-tauri/Cargo.toml" --target-dir "$PWD/src-tauri/target"
test ! -e "$PWD/src-tauri/target"
```

Real result:

```text
Removed 8292 files, 7.5GiB total
target absent: PASS
```

An earlier root-directory `cargo clean --target-dir ...` attempt refused with
`could not find Cargo.toml`; it removed nothing. The corrected explicit command
above performed the cleanup.

For the initial candidate, owned-file gates were then repeated. Real result:

```text
actionlint: exit 0, no findings
yq: exit 0
ruby_yaml_parse=PASS
docs/Seeds secret hygiene scan passed: 0 findings
Betterleaks scanned approximately 33 KB; no leaks found
git diff --check: PASS
worktree-local target absent: PASS
```

The exact base-to-worktree footprint assertion reported only the two assigned
paths.

Correction round 1 final owned-file gates reported:

```text
actionlint: exit 0, no findings
yq: exit 0
ruby_yaml_parse=PASS
docs/Seeds secret hygiene scan passed: 0 findings
Betterleaks scanned approximately 40 KB; no leaks found
git diff --check: PASS
base footprint: workflow and report only
correction footprint: workflow and report only
```

The correction gate regenerated only the ignored worktree-local Cargo target.
Explicit cleanup removed 1,319 files / 632.9 MiB, and `src-tauri/target` was
absent afterward.

Correction round 2 final owned-file gates also passed actionlint, yq/Ruby,
docs-secret hygiene, diff checks, and exact base/round-2 two-path footprint
assertions. Betterleaks scanned approximately 45 KB with no findings. Its
contract rerun regenerated 1,319 target files / 632.9 MiB; the explicit cleanup
again left `src-tauri/target` absent.

## Commits and footprint

The two existing unpushed commits were reworded with multiline why/gate bodies
using a controlled rebase. The old and new tips had the identical tree
`e5f9f0446abd43625d6b6a686528477b3c3e9471`; the reword introduced no file
change and preserved two linear commits before this correction.

Reworded workflow implementation commit:

```text
2a9645beea4376933ab332209aa4327016ab174e
audio-graph-52b9: add native durability workflow
```

Reworded initial report commit:

```text
1cc5b163507728ca4ff621ae004a40cb382397e7
audio-graph-52b9: record workflow evidence
```

Correction round 1 workflow/report commit:

```text
787cd56b0d61e0df33f8a82199b53d3abc22906d
audio-graph-52b9: preserve failure artifacts
```

Correction round 2 is committed as the fourth linear commit without rewriting
the prior three. Its hash is necessarily reported outside the commit because
the hash covers these report bytes. Final topology is four commits and zero
merges from the exact base.

Pre-report range footprint:

```text
.github/workflows/2df3-native-durability.yml | 353 insertions
```

The final base-to-tip file range remains the workflow plus this report only.

## Findings and open questions

- The workflow is statically and locally verified but not remotely executed.
  Linux, APFS, and NTFS claims still require a reviewed manual run.
- Final corrected-tip review is pending and is a hard pre-dispatch gate.
- Review round 1's single P1 has a mutation-tested correction; rereview of the
  stable tip produced Standards SHIP.
- Review round 2's single Spec P2 has static, behavioral, and mutation-tested
  correction evidence; corrected-tip Spec rereview remains pending.
- The mutable unhashed VB-CABLE archive remains a supply-chain limitation. A
  future upstream content hash or internally mirrored verified archive would
  materially improve provenance, but is outside this Seed.
- The workflow does not prove virtual-audio capture. Any such follow-up belongs
  to the separate live-audio workstream and must not reinterpret the endpoint
  inventory as a stronger result.

No unrelated implementation issue was found or fixed.

## Rollback

Before dispatch, rollback is a normal revert of the out-of-band round-2
correction commit, then `787cd56b0d61e0df33f8a82199b53d3abc22906d`,
`1cc5b163507728ca4ff621ae004a40cb382397e7`, and
`2a9645beea4376933ab332209aa4327016ab174e` if the whole workstream should be
removed. Because this workstream did not push or dispatch, rollback currently
has no remote job, runner, artifact, or credential cleanup.

After a future dispatch, reverting the workflow prevents new runs but does not
delete prior Actions artifacts. Those artifacts expire after 14 days unless an
authorized repository operator removes them sooner.

## Dispatch runbook — not executed

1. Independently review the exact two-file branch tip. Re-run actionlint, YAML
   parse, static assertions, secret gates, diff hygiene, and footprint checks.
2. Integrate or push only through the conductor's separately authorized
   workflow. This implementer did neither.
3. In GitHub Actions, select **2df3 Native Durability Evidence (Manual)** and
   select the exact reviewed ref. Record the expected SHA before pressing Run.
4. Leave `confirm_vb_cable_professional_license=false` unless the operator has
   professional-use authorization for the Windows VB-CABLE CI baseline. Never
   paste or upload license material. False initializes the Windows evidence
   path, overwrites its preflight/summary with `REFUSED`, and then refuses the
   cell before checkout/LABSN/platform work while Linux and macOS remain
   independent because matrix fail-fast is false.
5. For the intended three-platform evidence run, set the boolean true only
   after that authorization is confirmed, then dispatch once.
6. Monitor every Linux, macOS, and Windows job to terminal. Record the run ID,
   job IDs, checked-out SHA, runners, conclusions, and start/end times.
7. Download all three uniquely named artifacts. Verify runner/toolchain,
   command, fixture-filesystem, native-exit, and summary evidence. Require the
   files to agree with `test_logs`: neither log for `not_run`, exactly one
   nonempty log for `partial`, and both nonempty logs for `full`.
8. Require both native exits to be zero for a PASS. Classify a macOS non-APFS
   typed refusal or Windows namespace-durability refusal according to the
   harness contract rather than converting refusal into acceptance.
9. Treat the Windows endpoint JSON only as bounded virtual-device enumeration.
   Do not infer capture, playback, default selection, roundtrip, PCM, or `rsac`
   behavior.
10. Preserve the claim as process-crash recovery and completed OS-barrier
    outcomes on the observed filesystems. Do not call the run power-loss proof.

**Dispatch status: NOT DISPATCHED.**
