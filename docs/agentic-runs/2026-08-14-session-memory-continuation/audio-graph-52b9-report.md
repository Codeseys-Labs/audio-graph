# audio-graph-52b9 implementation report

## Outcome

Implemented the approved, opt-in native durability evidence workflow at
`.github/workflows/2df3-native-durability.yml`. The workflow is manual-only,
has read-only repository permission, runs the two accepted focused filters on
Linux, macOS, and Windows, and uploads a bounded evidence artifact for every
matrix cell even when the job fails.

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
the summary fails the platform honestly.

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
pinned to existing repository pins.

### Evidence contract

Each admitted platform records:

- checked-out Git SHA, with equality enforced against `GITHUB_SHA`;
- selected runner OS, architecture, name, image identifiers when available,
  and live OS version output;
- verbose `rustc` and Cargo versions;
- the two exact commands;
- a live filesystem probe rooted under the same OS temporary directory family
  used by the crash harness;
- complete content-free stdout/stderr for each filter;
- separate `canonical_durability.exit` and
  `canonical_crash_harness.exit` files; and
- a summary with both exits, final status, and the proof boundary.

The crash harness itself emits its filesystem observation from the live
fixture parent and content-free length/digest/stage evidence. The workflow does
not dump the process environment.

The upload step is full-SHA pinned, uses `if: always()`, treats a missing path
as an error, retains evidence for 14 days, and gives every platform/run/attempt
a distinct artifact name.

### Windows legal and audio boundary

`confirm_vb_cable_professional_license` is a required boolean input whose
default is `false`. The first Windows step refuses before checkout, LABSN, or
platform probing unless the value is exactly true. The refusal writes only a
reason/status artifact. The workflow never asks for, accepts, stores, or logs
license keys, files, or other license material.

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
- the first-step Windows refusal and Windows-only confirmed LABSN condition;
- four continuable Cargo steps with the exact filters, flags, and working
  directory;
- PowerShell and the Windows manifest environment on both Windows tests;
- native exit files, full logs, platform/toolchain/filesystem evidence, and
  proof-boundary text;
- bounded endpoint inventory;
- `always()` upload, missing-file error, and 14-day retention;
- no secret context, forbidden trigger, or whole-environment dump.

Real result:

```text
static_contracts=PASS
static_text_guards=PASS
```

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

After adding this report, the final owned-file gates were repeated. Real
result:

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

## Commits and footprint

Workflow implementation commit:

```text
340859e40ea784e4b93f6803a0bb45723fc72f2b
audio-graph-52b9: add native durability workflow
```

Pre-report range footprint:

```text
.github/workflows/2df3-native-durability.yml | 353 insertions
```

This report is committed separately so the workflow implementation remains a
single-file logical change. The final two-file range is the workflow plus this
report only.

## Findings and open questions

- The workflow is statically and locally verified but not remotely executed.
  Linux, APFS, and NTFS claims still require a reviewed manual run.
- Independent review is pending and is a hard pre-dispatch gate.
- The mutable unhashed VB-CABLE archive remains a supply-chain limitation. A
  future upstream content hash or internally mirrored verified archive would
  materially improve provenance, but is outside this Seed.
- The workflow does not prove virtual-audio capture. Any such follow-up belongs
  to the separate live-audio workstream and must not reinterpret the endpoint
  inventory as a stronger result.

No unrelated implementation issue was found or fixed.

## Rollback

Before dispatch, rollback is a normal revert of the workflow implementation
commit, followed by a separate revert of this report commit if the report
should also be removed. Because this workstream did not push or dispatch,
rollback currently has no remote job, runner, artifact, or credential cleanup.

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
   paste or upload license material. False intentionally refuses the Windows
   cell before checkout/LABSN/platform work while Linux and macOS remain
   independent because matrix fail-fast is false.
5. For the intended three-platform evidence run, set the boolean true only
   after that authorization is confirmed, then dispatch once.
6. Monitor every Linux, macOS, and Windows job to terminal. Record the run ID,
   job IDs, checked-out SHA, runners, conclusions, and start/end times.
7. Download all three uniquely named artifacts. Verify that each contains
   runner/toolchain, command, fixture-filesystem, both full logs, both native
   exits, and the final summary.
8. Require both native exits to be zero for a PASS. Classify a macOS non-APFS
   typed refusal or Windows namespace-durability refusal according to the
   harness contract rather than converting refusal into acceptance.
9. Treat the Windows endpoint JSON only as bounded virtual-device enumeration.
   Do not infer capture, playback, default selection, roundtrip, PCM, or `rsac`
   behavior.
10. Preserve the claim as process-crash recovery and completed OS-barrier
    outcomes on the observed filesystems. Do not call the run power-loss proof.

**Dispatch status: NOT DISPATCHED.**
