# audio-graph-ccf0 Windows filesystem-probe correction

## Status

Seed `audio-graph-ccf0` is implemented on the dedicated worktree branch. The
workflow correction and all available local gates pass. No workflow was pushed
or dispatched, and no Seed, evidence reference, dependency, product source, or
other workflow was changed.

## Assignment and acceptance criteria

- Seed: `audio-graph-ccf0`.
- Exact base: `fd6d3609da26e50587dc336410ba601c86cfbf45`.
- Branch: `work/ccf0-windows-probe`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/ccf0-windows-probe`.
- Owned implementation file:
  `.github/workflows/2df3-native-durability.yml`.
- Owned report:
  `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-ccf0-report.md`.
- Acceptance: preserve the recorded fixture path and drive root; pass a
  separator-free, Windows-compatible drive token to `fsutil` for `C:` and
  non-`C:` roots; reject missing, relative, UNC, or malformed roots; preserve
  the early nonempty evidence artifact; and leave license, archive, signer,
  DevCon, test-count, full-log, and TrustedPublisher behavior unchanged.

The assignment reported that terminal evidence run `31930286136` failed only
because Windows passed the trailing-backslash root to `fsutil` and received
Error 3. The same assignment reported Linux `42/11` and macOS APFS `13/11` as
passing. Those remote facts were inputs to this bounded correction; this worker
did not access GitHub or dispatch another run.

## Worktree admission

Initial commands:

```bash
git rev-parse --show-toplevel
git status --short
git branch --show-current
git rev-parse HEAD
```

Real output:

```text
/home/codeseys/DevBox/audio-graph/.worktrees/ccf0-windows-probe
work/ccf0-windows-probe
fd6d3609da26e50587dc336410ba601c86cfbf45
```

`git status --short` was empty.

## Implementation

The Windows evidence step still derives and records the exact fixture path and
root before fallible validation or `fsutil` work. It now:

1. refuses an empty root, a path that is not fully qualified, or a root that is
   not exactly a drive-letter root such as `C:\` or `R:\`;
2. trims the root's trailing backslash into a distinct token such as `C:` or
   `R:`;
3. validates the token independently; and
4. invokes `fsutil fsinfo volumeinfo` with the token while leaving
   `fixture_volume` recorded as the original rooted form.

The implementation commit is:

```text
7d5a178b2bf7610ffdf2a5439766522cfa2a6ec7
audio-graph-ccf0: normalize Windows fsutil volume
```

## TDD and mutation-sensitive static verification

The agreed static seam was the parsed `run` body of the named Windows runner,
toolchain, command, and fixture-filesystem evidence step.

An initial validator draft used the wrong job key
`native-durability-evidence`; it failed in the harness and was corrected to the
actual parsed job key `native-durability` before the RED result below was
counted.

### RED

The corrected pre-change Ruby/YAML extractor required the root guards, token
derivation, token guard, and `fsutil` token invocation while rejecting the old
root invocation.

Real output:

```text
red_contract=PASS errors=missing_root_guard,relative_path_guard,drive_root_guard,volume_token_trim,drive_token_guard,fsutil_volume_token,trailing_root_passed_to_fsutil
red_exit=1
```

### GREEN and mutation sensitivity

The same extractor passed after the implementation. It then applied five
in-memory source mutations and evaluated explicit `C:\`, `R:\`, missing, and
relative-root cases.

Real output:

```text
static_windows_volume_contract=PASS
mutation_trailing_slash_root_to_fsutil=REJECTED errors=fsutil_volume_token,trailing_root_passed_to_fsutil,early_artifact_order
mutation_hardcoded_c_drive=REJECTED errors=drive_root_guard,drive_token_guard
mutation_missing_root_allowed=REJECTED errors=missing_root_guard,early_artifact_order
mutation_relative_path_allowed=REJECTED errors=relative_path_guard
mutation_recorded_root_replaced_by_token=REJECTED errors=recorded_fixture_root
volume_case_C=PASS token=C:
volume_case_R=PASS token=R:
missing_and_relative_cases=REJECTED
```

The validator also requires the nonempty fixture record write to remain before
root validation, token derivation, and `fsutil` invocation.

## Syntax and workflow parsing gates

Commands:

```bash
NO_COLOR=1 actionlint -config-file .github/actionlint.yaml .github/workflows/2df3-native-durability.yml
yq eval '.' .github/workflows/2df3-native-durability.yml >/dev/null
ruby -e 'require "yaml"; doc = YAML.safe_load_file(ARGV.fetch(0), aliases: false); abort "not a mapping" unless doc.is_a?(Hash); puts "ruby_yaml_parse=PASS"' .github/workflows/2df3-native-durability.yml
```

The conditional PowerShell-parser gate checks `pwsh` first and then
`powershell.exe`, extracts the named step with `yq`, and passes the body to
`System.Management.Automation.Language.Parser.ParseInput` when either shell is
available.

Real output:

```text
actionlint=PASS
yq_parse=PASS
ruby_yaml_parse=PASS
powershell_parser=SKIP (pwsh and powershell.exe unavailable)
```

## Sensitive-behavior preservation gate

An inline Ruby comparison read the assigned base with `git show`, compared the
current file byte-for-byte before and after the filesystem-probe replacement
boundaries, and counted sensitive retained strings.

Real output:

```text
base_probe_only_footprint=PASS
license_archive_signer_devcon_test_logs_trustedpublisher=BYTE_IDENTICAL_OUTSIDE_PROBE
cargo_test_command_count=8
full_log_state_count=1
trusted_publisher_count=3
```

This proves that every workflow byte outside the bounded probe replacement is
identical to the exact assigned base, including the license gate, archive and
signer verification, DevCon handling, test commands/counts, full-log state
derivation, and TrustedPublisher behavior.

## Repository gates

Commands:

```bash
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli bun run verify:contracts
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli bun run verify:fast
```

Representative real output from `verify:contracts`:

```text
audio source contract is current: /home/codeseys/DevBox/audio-graph/.worktrees/ccf0-windows-probe/src/generated/audioSource.ts
provider registry is current: /home/codeseys/DevBox/audio-graph/.worktrees/ccf0-windows-probe/src/generated/providerRegistry.ts
session data movement contract is current: /home/codeseys/DevBox/audio-graph/.worktrees/ccf0-windows-probe/src/generated/sessionDataMovement.ts
endpoint credential routing contract is current: /home/codeseys/DevBox/audio-graph/.worktrees/ccf0-windows-probe/src/generated/endpointCredentialRouting.ts
speech span revision contract is current: /home/codeseys/DevBox/audio-graph/.worktrees/ccf0-windows-probe/src/generated/speechSpanRevision.ts
```

Representative real output from `verify:fast`:

```text
Checked 174 files in 302ms. No fixes applied.
sd ready --format json: parsed (50)
sd blocked --format json: parsed (96)
sd list --format json: parsed (50)
docs/Seeds secret hygiene scan passed: 0 findings
```

Both commands exited `0`. `verify:fast` also ran TypeScript, all five generated
contract checks, the Seeds JSON-output stress check, docs/Seeds secret hygiene,
and `git diff --check`.

## Secret, diff, cleanup, and footprint gates

Commands:

```bash
betterleaks dir --no-banner --redact .github/workflows/2df3-native-durability.yml
bun scripts/check-docs-secret-hygiene.mjs
git diff --check
git diff --name-only fd6d3609da26e50587dc336410ba601c86cfbf45 --
cargo clean --manifest-path "$PWD/src-tauri/Cargo.toml" --target-dir "$PWD/src-tauri/target"
test ! -e "$PWD/src-tauri/target"
```

Pre-report real output:

```text
scanned ~33410 bytes (33.41 KB)
no leaks found
docs/Seeds secret hygiene scan passed: 0 findings
implementation_footprint=PASS
 .github/workflows/2df3-native-durability.yml | 11 ++++++++++-
 1 file changed, 10 insertions(+), 1 deletion(-)
480M /home/codeseys/DevBox/audio-graph/.worktrees/ccf0-windows-probe/src-tauri/target
Removed 1319 files, 632.8MiB total
worktree_target_absent=PASS
```

`git diff --check` emitted no output and exited `0`. Before the report was
added, the exact base-to-tip footprint contained only the assigned workflow.
After the report commit, the required final footprint is exactly the assigned
workflow and this report.

Final pre-report-commit hygiene output:

```text
scanned ~42286 bytes (42.29 KB)
no leaks found
docs/Seeds secret hygiene scan passed: 0 findings
final_two_path_footprint=PASS
.github/workflows/2df3-native-durability.yml
docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-ccf0-report.md
worktree_target_absent=PASS
```

## Findings and open questions

- No unrelated defect was found or changed.
- Neither `pwsh` nor `powershell.exe` is installed on this Linux host, so a
  local PowerShell AST parse was unavailable. Actionlint, two YAML parsers,
  exact source-shape checks, mutation tests, and the probe-only byte comparison
  are green.
- A new authorized manual Windows workflow dispatch remains necessary to turn
  this local correction into terminal Windows evidence. This worker was
  explicitly prohibited from pushing or dispatching.
