# audio-graph-c65d implementation report

Date: 2026-08-15

## Outcome

The independent `ci/storage-probe` workspace now has a resolver-generated
RustSec remediation and its own local audit policy. `ammonia` is upgraded from
4.1.2 to 4.1.4 and `crossbeam-epoch` from 0.9.18 to 0.9.20. A fresh audit of
the resulting 483-package lock reports zero unignored vulnerabilities,
exactly two configured advisory exceptions, and three allowed warnings.

The two exceptions are deliberately narrower than lockfile reachability:

- `RUSTSEC-2026-0235` applies only to the retained inactive optional
  `rust_decimal 1.42.1 -> rkyv ^0.7.46` edge.
- `RUSTSEC-2023-0071` applies only to native storage-probe builds and
  fresh-process durability checks. Native Linux, macOS, and Windows inverse
  trees contain no RSA path; the wasm graph does contain
  `surrealdb-core -> jsonwebtoken[rust_crypto] -> rsa 0.9.10` and is not
  accepted by this exception.

Both locked storage features compile on Rust 1.95. SurrealKV and RocksDB each
wrote five rows in one process and recovered all five in a separate reader
process.

## Assignment and boundaries

- Seed: `audio-graph-c65d` — audit the storage-probe independent RustSec lock
  graph.
- Exact base: `3606b0910961cb00e3b72b841251aa3f4db60202`.
- Branch: `work/c65d-storage-probe-audit-wave7b`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/c65d-storage-probe-audit-wave7b`.
- Owned implementation paths:
  `ci/storage-probe/Cargo.lock` and
  `ci/storage-probe/.cargo/audit.toml`.
- Owned evidence path: this report.
- Implementation commit:
  `98bbd75` (`audio-graph-c65d: remediate storage probe audit`).

No Seed, product source, product or probe `Cargo.toml`, workflow, generated
file, `src-tauri` file, or unrelated documentation was edited. No merge,
push, workflow dispatch, `sd sync`, or Seed closure occurred.

## TDD audit trail

The test seam was the public `cargo audit` command run from the isolated
workspace. `cargo-audit-audit 0.22.1` fetched 1,216 advisories from advisory-db.

### Initial RED

Command:

```text
cd ci/storage-probe
CARGO_TERM_COLOR=never cargo audit
```

Result: exit 1; 491 scanned dependencies; five vulnerabilities; three allowed
warnings.

```text
RUSTSEC-2026-0193  ammonia 4.1.2
RUSTSEC-2026-0213  ammonia 4.1.2
RUSTSEC-2026-0204  crossbeam-epoch 0.9.18
RUSTSEC-2026-0235  rkyv 0.7.46
RUSTSEC-2023-0071  rsa 0.9.10
error: 5 vulnerabilities found!
warning: 3 allowed warnings found
```

The three allowed warnings were `atomic-polyfill 1.0.3` unmaintained,
`event-listener 5.4.1` unsound, and `spin 0.9.8` yanked.

### Resolver remediation

Only the two exact targeted resolver commands were used; the lockfile was not
hand-edited and no broad `cargo update` ran:

```text
cargo update -p ammonia@4.1.2 --precise 4.1.4
cargo update -p crossbeam-epoch@0.9.18 --precise 0.9.20
```

Cargo reported the requested direct updates and the compatible ammonia parser
subtree transition. The lock moved from 491 to 483 package stanzas.

| Kind | Exact package/version changes |
| --- | --- |
| Targeted | `ammonia 4.1.2 -> 4.1.4`; `crossbeam-epoch 0.9.18 -> 0.9.20` |
| Resolver transitive updates | `cssparser 0.35.0 -> 0.37.0`; `html5ever 0.35.0 -> 0.39.0`; `markup5ever 0.35.0 -> 0.39.0`; `phf_codegen 0.11.3 -> 0.13.1`; `string_cache 0.8.9 -> 0.9.0`; `string_cache_codegen 0.5.4 -> 0.6.1`; `tendril 0.4.3 -> 0.5.1`; `web_atoms 0.1.3 -> 0.2.6` |
| Resolver-pruned stanzas | `cssparser-macros 0.6.1`; `futf 0.1.5`; `mac 0.1.1`; `match_token 0.35.0`; and the redundant `phf`, `phf_generator`, `phf_macros`, and `phf_shared` 0.11.3 stanzas |

No other package/version entry changed. In particular, `surrealdb 3.1.5`,
`rust_decimal 1.42.1`, `rkyv 0.7.46`, and `rsa 0.9.10` are unchanged.

### Intermediate RED

Command:

```text
CARGO_TERM_COLOR=never cargo audit
```

Result: exit 1; 483 scanned dependencies; only two vulnerabilities remained;
the same three warnings remained allowed.

```text
RUSTSEC-2026-0235  rkyv 0.7.46
RUSTSEC-2023-0071  rsa 0.9.10
error: 2 vulnerabilities found!
warning: 3 allowed warnings found
```

### Final GREEN

After adding the workspace-local `.cargo/audit.toml`:

```text
CARGO_TERM_COLOR=never cargo audit
```

Result: exit 0; 483 scanned dependencies; zero unignored vulnerabilities; the
same three allowed warnings.

The machine-readable proof makes the ignored set and zero count explicit:

```text
CARGO_TERM_COLOR=never cargo audit --json | jq '{
  lockfile,
  ignore: .settings.ignore,
  vulnerabilities,
  warning_counts: (.warnings | with_entries(.value |= length))
}'
```

```json
{
  "lockfile": { "dependency-count": 483 },
  "ignore": ["RUSTSEC-2026-0235", "RUSTSEC-2023-0071"],
  "vulnerabilities": { "found": false, "count": 0, "list": [] },
  "warning_counts": { "unmaintained": 1, "unsound": 1, "yanked": 1 }
}
```

This is an explicit policy exception, not a claim that either advisory
disappeared from the lockfile.

## Reachability and exception rationale

### rkyv 0.7.46

Locked metadata reports that `rust_decimal 1.42.1` declares an optional
`rkyv = "^0.7.46"` dependency with default features off. Its active features
are only `default`, `maths`, `serde`, `serde-str`, `serde-with-str`, and `std`;
`rkyv` is absent.

The strongest current inverse-tree check is also empty:

```text
cargo tree --locked --target all --all-features --edges all \
  -i rkyv@0.7.46
warning: nothing to print.
```

The exception is therefore limited to a resolver-retained inactive optional
edge. It must be removed immediately if any inverse path becomes active, the
`rust_decimal/rkyv` feature is enabled, untrusted rkyv 0.7 archives enter
scope, or upstream removes or migrates the optional 0.7 edge. It is not
authorization to deserialize untrusted archives through rkyv 0.7.

### RSA 0.9.10

All-feature inverse trees are empty for the native targets exercised by the
cross-platform storage probe:

```text
cargo tree --locked --target x86_64-unknown-linux-gnu --all-features \
  --edges all -i rsa@0.9.10
warning: nothing to print.

cargo tree --locked --target aarch64-apple-darwin --all-features \
  --edges all -i rsa@0.9.10
warning: nothing to print.

cargo tree --locked --target x86_64-pc-windows-msvc --all-features \
  --edges all -i rsa@0.9.10
warning: nothing to print.
```

The wasm tree is explicitly non-empty:

```text
cargo tree --locked --target wasm32-unknown-unknown --all-features \
  --edges normal -i rsa@0.9.10
rsa v0.9.10
└── jsonwebtoken v10.4.0
    └── surrealdb-core v3.1.5
        └── surrealdb v3.1.5
            └── storage-probe v0.0.0
```

Feature-form output identifies `jsonwebtoken[rust_crypto]` as the activating
wasm path. The exception applies only to native probe compilation and local
fresh-process storage evidence. It does not claim SurrealDB never uses RSA and
does not accept a wasm probe. Remove it immediately for a wasm probe, any
native inverse path, RSA/JWT issuance or decryption, attacker-observable
network timing, or an upstream fix or elimination of the dependency.

## Build and durability evidence

The repository-default Rust toolchain is 1.88, while the probe manifest already
documents that the SurrealDB 3.1.x graph requires Rust 1.94 or newer. The
installed pinned Rust 1.95 toolchain was therefore used for build/run gates.
Every Cargo gate used `--locked`.

| Gate | Result |
| --- | --- |
| `cargo +1.95.0 check --locked --features surrealkv` | exit 0; storage-probe finished in 1m35s |
| `cargo +1.95.0 check --locked --features rocksdb` | exit 0; storage-probe finished in 5m44s |
| SurrealKV writer process | exit 0; `phase=write engine=surrealkv rows_present=5 expected=5` |
| SurrealKV separate reader process | exit 0; `phase=read engine=surrealkv rows_present=5 expected=5` |
| RocksDB writer process | exit 0; `phase=write engine=rocksdb rows_present=5 expected=5` |
| RocksDB separate reader process | exit 0; `phase=read engine=rocksdb rows_present=5 expected=5` |

The writer and reader commands were separate `cargo run` invocations for each
engine. The release builds completed in 5m06s for SurrealKV and 8m19s for
RocksDB; the cached reader invocations completed in 0.64s and 0.28s.

After the gates, `cargo +1.95.0 clean` removed 8,119 generated files and 4.8
GiB from the worktree-local probe target. No generated target artifact remains
in the Git footprint.

## Ownership and workflow approval boundary

`ci/storage-probe` is a separate Cargo workspace with a separate lockfile. Its
policy is loaded only when `cargo audit` runs from that directory; the
`src-tauri/.cargo/audit.toml` policy and the existing application audit job do
not own or prove this graph.

This implementation selects the documented-ownership option in the Seed's
acceptance criteria. The actionable local command is:

```text
cd ci/storage-probe && cargo audit
```

Adding or changing a GitHub Actions audit step remains an approval-gated
workflow change and was explicitly outside this workstream. No workflow was
edited or dispatched. Remote cross-platform CI has therefore not been claimed
for this candidate; an authorized workflow follow-up should invoke the command
above independently from the application audit.

## Footprint, hygiene, and rollback

Implementation footprint relative to the exact base:

```text
ci/storage-probe/.cargo/audit.toml |  29 insertions
ci/storage-probe/Cargo.lock        |  33 insertions, 119 deletions
2 files changed, 62 insertions, 119 deletions
```

The report is the only additional owned artifact. The final range must contain
exactly those three paths.

Implementation-file Betterleaks passed after scanning approximately 120.72 KB
with no leaks. After this report existed, the final three-file Betterleaks
scan covered approximately 132.21 KB and found no leaks. The repository
docs/Seeds secret scanner found zero findings. Both the exact-base
implementation range and the staged report passed `git diff --check`.

```text
bun scripts/check-docs-secret-hygiene.mjs
docs/Seeds secret hygiene scan passed: 0 findings

betterleaks dir --no-banner --redact \
  ci/storage-probe/Cargo.lock \
  ci/storage-probe/.cargo/audit.toml \
  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-c65d-report.md
scanned ~132214 bytes (132.21 KB)
no leaks found

git diff 3606b0910961cb00e3b72b841251aa3f4db60202..HEAD --check
git diff --cached --check
(no output; both exit 0)
```

Rollback of the implementation candidate is the history-preserving command
`git revert 98bbd75`. That intentionally restores the original 491-package
lock and removes the probe policy, so the initial five-finding audit RED is the
expected post-rollback result. It is a candidate rollback, not a security
remediation.

## Findings and open questions

- No additional in-scope vulnerability appeared after the targeted resolver
  updates. The three informational warnings remain visible and unignored by
  advisory ID.
- The rkyv exception remains safe only while the optional 0.7 edge is inactive.
- The RSA exception is native-probe-only; wasm is a proven active path and
  requires separate remediation before any wasm storage probe is acceptable.
- A CI workflow audit step for this independent workspace still requires
  explicit authorization. This report records the exact command and ownership
  boundary but does not alter or dispatch CI.
- Seed `audio-graph-c65d` remains open for conductor review and queue hygiene;
  this worker did not edit or close it.
