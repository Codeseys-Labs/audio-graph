# audio-graph-fd9f implementation report

Date: 2026-08-14

Seed: `audio-graph-fd9f` — Replace rsac sibling path dependency with published
or pinned dependency

Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/fd9f-rsac-current-wave1`

Branch: `work/fd9f-rsac-current-wave1`

Base: `19ba1a1c1cf9ccbd85a973f8b0222abf1cb8fff8`

Implementation tip: `ead343d711ed101f010cf541c7c40e03618609be`

Initial report tip: `321f8aee7b0c33207e14c5f39766f389a06a1b18`

Review-fix implementation tip: `2f023d2e8821b0949a5e0ee5b3832982117669dd`

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

## Review-fix round

One bounded review-fix round addressed the release-attestation finding and
tested the proposed local-override path correction before accepting it.

### Release identity and dry-run evidence

The focused static assertions were red before the fix because the release
workflow exposed only an rsac SHA, did not record the package version in each
platform manifest, and did not retain that manifest in dry-run artifacts:

```text
RED version output=2 revision output=2 manifest version=2 manifest revision=2 dry-run retention=2 sibling path=2 (expected nonzero)
```

The workflow now derives the package version and full revision together from
`cargo metadata --locked`. The create job propagates both fields; release step
summaries and draft release notes show both; every platform re-resolves and
compares both; platform manifests contain `rsac_version` and `rsac_revision`;
and `release-dry-run-${platform}` retains that manifest. The existing non-dry
`gh release upload` behavior remains unchanged.

The production metadata parser was executed locally against the committed
lockfile:

```text
runtime metadata identity: version=0.4.4 revision=ea2019bba217cab695d45696bc2ca25430b23dc2
GREEN workflow/docs assertions: version+revision derived, propagated, compared, summarized, manifested, dry/non-dry retained; Cargo-correct ../rsac preserved
```

### Rejected README path finding

The review proposed changing the patch override in
`.cargo/rsac-local.toml` from `../rsac` to `../../rsac`, conditional on Cargo
1.95 verification. A disposable sibling-layout fixture under the ignored
worktree target directory tested that exact proposal with:

```text
cargo +1.95.0 --config <fixture>/audio-graph/.cargo/rsac-local.toml \
  metadata --manifest-path <fixture>/audio-graph/Cargo.toml \
  --format-version 1 --offline
```

With `path = "../../rsac"`, Cargo 1.95.0 attempted to load:

```text
.../src-tauri/target/rsac/Cargo.toml
```

rather than the adjacent fixture sibling:

```text
.../src-tauri/target/fd9f-config-path-proof/rsac/Cargo.toml
```

This proves the config path is based at the project root for this invocation.
For the canonical checkout `/home/codeseys/DevBox/audio-graph`, `../rsac`
resolves to the historical sibling location `/home/codeseys/DevBox/rsac`, while
`../../rsac` would incorrectly resolve to `/home/codeseys/rsac`. The README
therefore retains `../rsac`; the disposable fixture files were removed.

### Review-fix gates

Commands:

```text
actionlint -config-file .github/actionlint.yaml .github/workflows/release.yml
cargo +1.95.0 metadata --manifest-path src-tauri/Cargo.toml --format-version 1 --locked
bun scripts/check-docs-secret-hygiene.mjs
git diff --check
```

Actionlint, runtime metadata extraction, focused workflow/docs assertions, docs
secret hygiene, and diff checks passed. The 1,498-test Rust suite was not rerun
because this review round changes only release evidence wiring and this report;
the dependency graph and Rust code are unchanged.

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

To revert only the review-fix release evidence wiring after integration, run:

```text
git revert 2f023d2e8821b0949a5e0ee5b3832982117669dd
```

## Open questions

None for the Linux implementation slice. The remaining acceptance questions
are the external Windows, macOS, and release dry-run evidence listed above.
