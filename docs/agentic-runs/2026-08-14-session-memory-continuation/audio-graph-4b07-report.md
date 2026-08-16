# audio-graph-4b07 direct LABSN Windows action

## Status

Seed `audio-graph-4b07` is implemented on its dedicated worktree branch. The
workflow now invokes the exact pinned LABSN composite action on GitHub-hosted
`windows-2025`; Linux and macOS remain on Blacksmith. Local workflow, contract,
mutation, typecheck, and frontend gates pass. Native Windows acceptance remains
pending until the reviewed change is published and the manual evidence workflow
is dispatched.

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
   plus present/healthy `CABLE Input` and `CABLE Output` endpoints. It writes the
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
PASS: direct LABSN action contract and 14 mutations
```

The check rejects all of these in-memory mutations:

- replacing the exact action commit with floating `v1`;
- inventing an unsupported action input;
- removing `always()` from certificate restoration;
- omitting targeted certificate removal;
- making the targeted certificate-removal branch unreachable;
- admitting a pre-existing endpoint;
- accepting only one VB-CABLE endpoint;
- omitting the `VBAudioVACWDM` hardware identity;
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
direct LABSN contract: PASS, 14/14 mutations rejected
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

## Native acceptance still required

Local checks cannot close `audio-graph-4b07` or `audio-graph-2df3`. The published
evidence ref must be dispatched with the already-recorded professional-use
attestation. Closure requires:

- Windows prestate reports zero pre-existing VB-CABLE endpoints;
- the exact pinned LABSN action runs on `windows-2025`;
- the targeted TrustedPublisher certificate state is restored;
- the post-action device and both endpoint canaries pass;
- Windows NTFS runs `12/12` durability and `9/9` crash tests with full logs;
- Linux Blacksmith ext4 remains `42/11`;
- macOS Blacksmith APFS remains `13/11`;
- each platform summary is `PASS` and all three artifacts upload; and
- Blacksmith Testbox inventory is empty after the run.

No workflow was dispatched while producing this implementation report.
