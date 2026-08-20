# audio-graph-4b07 direct LABSN Windows action

## Status

Seed `audio-graph-4b07` is implemented on its dedicated worktree branch. The
workflow now invokes the exact pinned LABSN composite action on GitHub-hosted
`windows-2025`; Linux and macOS remain on Blacksmith. Local workflow, contract,
mutation, typecheck, and frontend gates pass. Terminal native run `31967064623`
passes all three platforms and satisfies the bounded 4b07/2df3 acceptance
criteria.

## Assignment and scope

- Seed: `audio-graph-4b07`.
- Exact base: `80f72b3fe960188adeb609f7603a4d3164a82386`.
- Branch: `work/4b07-wdk-devcon-wave7b`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/4b07-wdk-devcon-wave7b`.
- Owned workflow: `.github/workflows/2df3-native-durability.yml`.
- Owned regression check: `scripts/check-2df3-labsn-action.mjs`.
- Package integration: `package.json`.
- Owned report: this file.

The product-owner direction superseded the initial WDK-replacement wording:
LABSN must perform Windows virtual-audio setup through its pinned GitHub action.
Windows remains on GitHub-hosted `windows-2025`; no attempt is made to run
LABSN on Blacksmith. No license material is requested, stored, or logged.

## Why the direct action still needs caller-owned evidence

The exact action
`LABSN/sound-ci-helpers@d08c889a7bba7d9b1b059f8f76dac4672ea3a9cf`
declares no inputs or outputs. Its Windows script downloads a mutable VB-CABLE
archive, imports its bundled certificate into `TrustedPublisher`, and invokes a
bundled DevCon helper without exposing a structured install result or cleanup
hook. Therefore action-step success alone is not accepted as installation
proof.

The workflow records that boundary explicitly:

- caller archive-integrity verification: false;
- caller catalog-signature and member verification: false;
- caller DevCon-signature verification: false;
- pre-canary setup-proof claim: false;
- passing evidence class: bounded device and endpoint enumeration only;
- excluded claims: capture, playback, default-device selection, roundtrip, and
  rsac PCM behavior.

The prior manual implementation's archive, catalog, member, and Microsoft
DevCon provenance claims were removed because direct action use cannot honestly
retain them.

## Implementation

The Windows path now runs these ordered steps after the license gate and runner
preflight:

1. `Record LABSN Windows prestate` records the exact action pin, the fact that
   the action has no inputs or outputs, the pinned certificate's raw-SHA-256
   match count in `LocalMachine\TrustedPublisher`, and the pre-action VB-CABLE
   endpoint count. It refuses an already-visible VB-CABLE endpoint to avoid a
   trivial false positive; the final evidence remains a post-action presence
   canary, not a causal device-transition proof.
2. `Install Windows virtual audio baseline with pinned LABSN action` invokes the
   exact commit directly with no fictitious `with:` inputs. It uses
   `continue-on-error: true` only so the cleanup step can run before the job
   rejects a failed action outcome.
3. `Restore LABSN TrustedPublisher state` runs under `always()`. It hashes
   certificate raw bytes, removes only matches for the pinned certificate when
   that target was absent before the action, re-enumerates the target, records
   cleanup evidence, and rejects a restoration or action failure only after the
   evidence file exists. It makes no whole-store-equivalence claim.
4. `Record bounded allowlisted Windows endpoint inventory` starts and bounds a
   wait for `Audiosrv`, then requires a present/healthy `VBAudioVACWDM` device
   plus a present/healthy render endpoint (`CABLE Input` or Pack43's exact
   `Speakers (VB-Audio Virtual Cable)` alias) and `CABLE Output` capture
   endpoint. It writes the
   observed bounded inventory before any validation throw. Only after every
   check passes does it create the separate
   `windows-installation-canary.txt` PASS artifact. The supply-boundary artifact
   itself explicitly says no setup proof has yet been claimed. Only after that
   independent presence canary passes may the Windows `12/12` durability and
   `9/9` crash-harness filters run.

The action-installed driver/device remains only for the lifetime of the
ephemeral GitHub-hosted job. The workflow does not invent an unsupported LABSN
uninstall interface or upload action/download/driver contents.

## TDD evidence

The public seam is the parsed workflow contract comprising the named prestate,
direct action, `always()` cleanup, endpoint canary, and first Windows test step.

### RED

The regression check was written before the workflow change. Against the exact
base it exited nonzero:

```text
error: missing workflow step: Record LABSN Windows prestate
Bun v1.3.14 (Linux x64)
```

The base also still contained the manual LABSN checkout and reimplemented
archive/DevCon path, so it could not satisfy the direct-action contract.

### GREEN and mutation sensitivity

After implementation:

```text
$ bun scripts/check-2df3-labsn-action.mjs
PASS: direct LABSN action contract and 18 mutations
```

The check rejects all of these in-memory mutations:

- replacing the exact action commit with floating `v1`;
- inventing an unsupported action input;
- removing `always()` from certificate restoration;
- omitting targeted certificate removal;
- making the targeted certificate-removal branch unreachable;
- admitting a pre-existing endpoint;
- making an empty Windows endpoint class a fatal prestate query;
- accepting only one VB-CABLE endpoint;
- omitting Pack43's exact render-endpoint alias;
- widening the exact capture-endpoint alias;
- omitting the `VBAudioVACWDM` hardware identity;
- making an absent PnP hardware-ID property fatal;
- omitting the `Audiosrv` readiness requirement;
- admitting a failed LABSN action outcome;
- overclaiming caller archive verification;
- claiming installation proof before the canary;
- omitting the final PASS canary artifact; and
- reintroducing caller-side manual download/install behavior.

It also freezes the Linux `42/11`, macOS `13/11`, and Windows `12/9` matrix
counts and requires the endpoint canary to precede the Windows tests.

## Local workflow and repository gates

Commands:

```bash
bun run check:2df3-labsn-action
actionlint -config-file .github/actionlint.yaml .github/workflows/2df3-native-durability.yml
yq eval '.' .github/workflows/2df3-native-durability.yml >/dev/null
ruby -e 'require "yaml"; YAML.parse_file(ARGV.fetch(0))' .github/workflows/2df3-native-durability.yml
SEEDS_CLI_ROOT=$PWD/node_modules/@os-eco/seeds-cli bun run verify:fast
bun run test:local
git diff --check
```

Observed results:

```text
direct LABSN contract: PASS, 18/18 mutations rejected
actionlint: PASS
yq parse: PASS
Ruby YAML parse: PASS
Biome: 174 files PASS
TypeScript: PASS
generated contracts: 5/5 current
Seeds JSON stress: ready 50, blocked 96, list 50
docs/Seeds secret hygiene: 0 findings
frontend: 70 files, 968 tests PASS in 105.15s
git diff --check: PASS
```

The implementation worktree initially had no `node_modules`. An ephemeral
worktree-local symlink to the integration checkout's repo-pinned dependencies
was used for the repository gates and is removed before final custody.

No PowerShell executable is installed locally, so local AST parsing is
unavailable. The YAML bodies are parsed by actionlint and the Windows
PowerShell bodies must receive their executable parser/runtime acceptance in
the GitHub-hosted `windows-2025` job.

## Independent review

Parallel Standards and Spec reviews initially found one blocking truthfulness
issue and three evidence/test-quality issues:

- the supply-boundary artifact claimed setup proof before the canary ran;
- failed endpoint/device validation did not preserve the observed inventory;
- cleanup evidence omitted individual native exits and its failure stage; and
- the mutation suite did not prove the targeted cleanup branch or final PASS
  artifact target.

The correction keeps the supply boundary non-affirmative, emits the PASS
canary only after the inventory and all checks, preserves observed inventory
on validation failure, records cleanup exits/stage, and tests both control-flow
seams. The report's earlier causal-transition wording was also narrowed to
presence-only evidence.

Both final-cap re-reviews returned `SHIP` with no P0, P1, or P2 findings. Final
local gates remained green at the reviewed snapshot: 14/14 mutations,
`verify:fast`, actionlint, yq, Ruby YAML parsing, Betterleaks, docs/Seeds secret
hygiene, and `git diff --check`.

## First native rerun correction

Run `31965666406` exercised the reviewed snapshot. Linux Blacksmith ext4 and
macOS Blacksmith APFS passed, but Windows failed in prestate before LABSN ran.
On an audio-empty `windows-2025` runner, `Get-PnpDevice -Class AudioEndpoint
-PresentOnly -ErrorAction Stop` raises `No matching Win32_PnPEntity objects`
instead of returning an empty collection. LABSN was skipped; Windows durability
and crash tests were not run.

A new RED required empty AudioEndpoint prestate to be a valid zero-result
observation. The corrected query enumerates present PnP devices and filters the
result by class, so an empty endpoint class becomes `Count = 0` while genuine
enumeration failures still fail closed. The direct-action check now rejects
15/15 mutations. This correction does not weaken the zero-preexisting-endpoint
gate or alter LABSN, cleanup, canary, test, runner, or Unix behavior.

## Second native rerun correction

Run `31966148208` proved the empty-endpoint prestate correction and the direct
LABSN path on GitHub-hosted `windows-2025`: prestate recorded zero endpoints and
zero target certificates; the pinned action reported success; it added one
target certificate; caller cleanup removed it with native exit `0`; and the
target count returned to zero. Linux Blacksmith ext4 and macOS Blacksmith APFS
again passed.

The Windows presence canary then failed while scanning unrelated present PnP
devices. `Get-PnpDeviceProperty` can return an object without a `Data` property,
and `Select-Object -ExpandProperty Data` made that normal absence fatal before
the VB-CABLE device could be selected. Windows tests therefore remained not run.

A new RED requires absent hardware-ID properties to be safe empty observations.
The corrected canary inspects the returned object's property names before
reading `Data`; the exact `VBAudioVACWDM` match remains mandatory for PASS. The
direct-action check now rejects 16/16 mutations. This does not weaken action
outcome, device status, endpoint, service, certificate-restoration, or native
test gates.

## Third native rerun correction

Run `31966577145` proved the guarded hardware-ID enumeration and preserved a
complete failed-canary inventory. Windows again passed zero/zero prestate,
direct pinned LABSN, targeted certificate restoration, NTFS, one healthy
`VBAudioVACWDM` device, two healthy VB-CABLE endpoint records, and running
`Audiosrv`. Linux Blacksmith ext4 and macOS Blacksmith APFS again passed.

The recorded Windows endpoints were `CABLE Output (VB-Audio Virtual Cable)` and
`Speakers (VB-Audio Virtual Cable)`. The Pack43 archive used by the pinned LABSN
action names its render endpoint `Speakers`, while the initial canary required
the alternative `CABLE Input` name. That name mismatch prevented the PASS
artifact and left Windows tests not run.

A new RED requires both exact render aliases to remain recognized. The canary
now accepts only `CABLE Input (VB-Audio Virtual Cable)` or the observed exact
Pack43 `Speakers (VB-Audio Virtual Cable)` name for render, while continuing to
require exact `CABLE Output` capture, healthy status, two VB-CABLE records, the
healthy hardware ID, and `Audiosrv`. The direct-action check now rejects 18/18
mutations; this is an evidence-backed alias correction, not a broad friendly-name
match or relaxed virtual-audio gate.

Standards review found that the first correction anchored only the capture
prefix while this report described the full alias as exact. The final capture
predicate is now exactly anchored to
`CABLE Output (VB-Audio Virtual Cable)`, and an 18th mutation prevents that
predicate from widening back to a prefix match.

## Terminal native acceptance

Manual workflow run `31967064623` completed `success` at exact evidence head
`5f34a8656db4f1da59e9ba367b401bb81045d653`.

| Platform | Job | Runner | Result |
|---|---:|---|---|
| Windows | `95213738611` | GitHub-hosted `windows-2025` | PASS |
| Linux | `95213738702` | `blacksmith-4vcpu-ubuntu-2404` | PASS |
| macOS | `95213738599` | `blacksmith-6vcpu-macos-15` | PASS |

Windows evidence:

- exact head and NTFS fixture evidence passed;
- prestate was zero target certificates and zero VB-CABLE endpoints;
- the exact pinned LABSN action executed successfully;
- targeted certificate state was `0 -> 1 -> removal exit 0 -> 0`, with
  `publisher_state_restored=true`;
- one healthy `VBAudioVACWDM` device, one ready Pack43 render alias, one ready
  exact capture alias, and running `Audiosrv` produced a PASS presence canary;
- canonical durability passed `12/12` and the crash harness passed `9/9`, with
  native and tee exits zero and full nonempty logs; and
- the Windows summary was `PASS` with `test_logs=full`.

Linux ext4 passed `42/42` durability and `11/11` crash tests. macOS APFS passed
`13/13` durability and `11/11` crash tests. Every platform uploaded a nonempty
artifact:

- Windows artifact `9268887195`, archive SHA-256
  `051454680a5b3b123d025014ab466b8ce791ab2a216c0b31a0ecbd584be76ac2`;
- Linux artifact `9268793546`, archive SHA-256
  `3e86aa85d37f42c0c4367970a3f28b8088c98af333d63e33db6e96593fdc9f06`;
- macOS artifact `9268790371`, archive SHA-256
  `0ca62140abca19cec4f8938a5997a620bccb564c475eb60860f7631a941c4705`.

The audited evidence bundle is under
`/tmp/audio-graph-run-31967064623.kMyPkZ`. Betterleaks-style review found no
license material; only the fixed `license_material=not_requested_or_logged`
marker appeared. `blacksmith testbox list --all` reported zero active
Testboxes.

The accepted evidence remains intentionally bounded: endpoint/device/service
presence only, not PCM capture, playback, default selection, roundtrip, or rsac;
process-crash recovery and completed OS-barrier outcomes only, not power loss;
and a SHA-pinned action source whose mutable Pack43 download is explicitly not
caller-verified for archive, catalog, or DevCon provenance.
