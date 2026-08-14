# audio-graph-fd9f implementation report

Date: 2026-08-14

Seed: `audio-graph-fd9f` — Replace rsac sibling path dependency with published
or pinned dependency

Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/fd9f-rsac-current-wave1`

Branch: `work/fd9f-rsac-current-wave1`

Base: `19ba1a1c1cf9ccbd85a973f8b0222abf1cb8fff8`

Implementation tip: `ead343d711ed101f010cf541c7c40e03618609be`

## Outcome

The official rsac releases page was rechecked on 2026-08-14. It still marks
v0.4.4 as the newest stable release and points to commit
`ea2019bba217cab695d45696bc2ca25430b23dc2`:
<https://github.com/Codeseys-Labs/rust-crossplat-audio-capture/releases>

All three desktop target dependencies now pin that full immutable commit with
default features disabled and only `feat_linux`, `feat_windows`, or
`feat_macos`. The application lock resolves rsac 0.4.4 at the same source.
Cargo metadata and the committed lockfile are the only build and release
authority; CI and release no longer clone a sibling rsac checkout or accept an
independent rsac revision input.

No capture source change was required. The current branch already included the
v0.4.4-compatible `PlatformCapabilities::requires_user_consent` test fixture.
No credential-v2 history was merged or patched into this branch.

## Files changed

- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `.gitignore`
- `README.md`
- `docs/CONTRIBUTING.md`
- `docs/RELEASE.md`
- `docs/WINDOWS_QUICKSTART.md`
- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock`

The report itself is the only additional file. `.seeds/issues.jsonl`, capture
code, credentials, and unrelated source/docs were not changed.

## TDD evidence

Pre-agreed seams: locked Cargo package identity and workflow resolution/
attestation authority.

### Red

Before editing, locked metadata succeeded but the semantic assertion for the
approved v0.4.4 source failed:

```text
RED metadata command exit=0
[{"name":"rsac","version":"0.4.1","source":"git+https://github.com/Codeseys-Labs/rust-crossplat-audio-capture.git?rev=7956e6ef24a44672d502e72b0500efb27530e3b9#7956e6ef24a44672d502e72b0500efb27530e3b9"}]
RED metadata semantic assertion exit=1 (expected nonzero)
```

The workflow assertion also failed because both workflows contained independent
revision inputs and sibling-path machinery. Representative output:

```text
.github/workflows/release.yml:44:  RSAC_REPO_URL: https://github.com/Codeseys-Labs/rust-crossplat-audio-capture.git
.github/workflows/release.yml:46:  RSAC_REPO_SHA: a2d3088b0ae8050d1ce79966298cc792c6694ec2
.github/workflows/release.yml:149:      - name: Fetch rsac parent (for path dep)
.github/workflows/ci.yml:40:  RSAC_REPO_SHA: a2d3088b0ae8050d1ce79966298cc792c6694ec2
.github/workflows/ci.yml:54:      - name: Fetch rsac parent (for path dep)
RED workflow single-authority assertion exit=1 (expected nonzero)
```

### Green

After the manifest change and lock regeneration:

```text
{"version":"0.4.4","source":"git+https://github.com/Codeseys-Labs/rust-crossplat-audio-capture.git?rev=ea2019bba217cab695d45696bc2ca25430b23dc2#ea2019bba217cab695d45696bc2ca25430b23dc2"}
GREEN semantic assertions: metadata=v0.4.4/ea2019bb, 3 desktop-only pins, no independent SHA/sibling machinery, locked CI/release commands
Final static gates: fmt/actionlint/metadata/semantic/docs/diff/scope PASS
```

The semantic assertions additionally proved:

- exactly three target-specific pins use the same full revision;
- only the three approved desktop rsac features are enabled;
- no compose, mobile, or private macOS TCC SPI rsac feature is enabled;
- CI/release contain three `cargo metadata --locked` identity checks;
- canonical product Cargo/Tauri workflow commands are locked;
- stale v0.4.1 docs and independently configurable rsac workflow inputs are
  absent from the owned files.

## Gates

All Rust commands used the stable worktree-specific target directory
`src-tauri/target/fd9f-wave1`.

### Locked metadata

Command:

```text
cargo +1.95.0 metadata --manifest-path src-tauri/Cargo.toml --format-version 1 --locked
```

Result: pass. One rsac package, version 0.4.4, exact source revision
`ea2019bba217cab695d45696bc2ca25430b23dc2`.

### Locked cloud check

Command:

```text
cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud --locked
```

Result:

```text
Checking audio-graph v0.1.0-rc.1 (.../src-tauri)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.76s
```

### Focused capture compatibility tests

Command:

```text
cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud source_descriptor_tests --locked -- --nocapture --test-threads=1
```

Result:

```text
running 10 tests
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1496 filtered out; finished in 0.00s
```

### Full locked cloud library suite

Command:

```text
cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud --locked -- --test-threads=1
```

Result:

```text
test result: ok. 1498 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 37.95s
full cloud lib test without xvfb exit=0
```

### Strict Clippy

Command:

```text
cargo +1.95.0 clippy --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud --locked -- -D warnings
```

Result:

```text
Finished `dev` profile [unoptimized + debuginfo] target(s) in 37.53s
```

### Format, workflows, and diff hygiene

Commands:

```text
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
actionlint -config-file .github/actionlint.yaml .github/workflows/ci.yml .github/workflows/release.yml
git diff --check
```

Result: pass; all three exited 0 with no diagnostics.

## Findings

- An optional expanded-suite attempt through `xvfb-run -a` printed the same
  green `1498 passed; 0 failed; 8 ignored` Cargo summary but the wrapper exited
  1 after Cargo completed. The direct serialized Cargo command passed with exit
  0. The required focused capture gate and CI's existing Xvfb-specific cells
  were not changed. This host-only wrapper status is not an fd9f blocker.
- The v0.4.4 lock refresh legitimately changes rsac's resolved dependency
  edges in addition to the rsac package source/version. No unrelated package
  was intentionally updated.

## Remaining cross-platform proof

The Seed must remain open until external workflow evidence records:

- Windows locked compile and capture matrix at the same Cargo-resolved revision;
- macOS locked compile and capture matrix at the same Cargo-resolved revision;
- an approval-gated release dry run whose per-platform manifests attest exactly
  the Cargo-resolved v0.4.4 revision.

No workflow was dispatched and no release was created or published during this
workstream.

## Rollback

Before integration, omit the two fd9f branch commits from fan-in. After the
implementation commit has been integrated, revert the product/workflow/docs
change with:

```text
git revert ead343d711ed101f010cf541c7c40e03618609be
```

This restores the v0.4.1 manifest/lock and the prior workflow/docs behavior
without rewriting shared history. The report-only commit may be retained as
historical evidence or reverted separately by the integrator.

## Open questions

None for the Linux implementation slice. The remaining acceptance questions
are the external Windows, macOS, and release dry-run evidence listed above.
