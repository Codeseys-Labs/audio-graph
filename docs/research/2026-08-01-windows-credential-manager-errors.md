# Windows Credential Manager numeric error seam

Date: 2026-08-01

Owner: `audio-graph-5d37`

Base: `b59c8c8ca0f931510642049d60f25ba4d6d36103`

## Question and gated decision

Does `windows-native-keyring-store` 1.1.0 expose stable, public numeric
failures while also supporting AudioGraph's exact binary Generic credential,
2,560-byte envelope, and Local persistence contract?

This gates whether `audio-graph-fb2b` can use that provider behind a thin
AudioGraph mapper or must own a small Windows `keyring-core` adapter over
`CredReadW`, `CredWriteW`, and `CredDeleteW`.

## Recommendation

**Use a small direct Windows adapter; do not use
`windows-native-keyring-store` 1.1.0 on the production Windows path.** Keep
`keyring-core` as the cross-platform entry contract, but let an AudioGraph-owned
credential type call the Win32 functions directly, capture `GetLastError`
inside the failing call shim, and box a public AudioGraph numeric error type.

Confidence: **high (0.97)** for the wrapper-versus-direct decision. Confidence
is **medium (0.75)** that Windows Credential Manager will organically return
`ERROR_ACCESS_DENIED` or `ERROR_CANCELLED` in supported packaged scenarios;
Microsoft does not list either for these three non-UI functions. The adapter
should classify them defensively if received, but release evidence must not
pretend those outcomes were observed when they were only injected.

The provider is otherwise compatible with the storage shape: it accepts an
explicit target, binary `set_secret`/`get_secret`, an inclusive 2,560-byte
blob, Generic type, and `Local` persistence. Its error API alone is the
disqualifier.

## Decisive evidence

### Exact dependency and release scope

- **[verified]** The worktree lock resolves `keyring-core` 1.0.0,
  `windows-native-keyring-store` 1.1.0, and `windows` 0.62.2 with registry
  checksums in [`src-tauri/Cargo.lock`](../../src-tauri/Cargo.lock). The direct
  Windows dependency is already pinned to 0.62.2, but does not yet enable the
  `Win32_Security_Credentials` feature in
  [`src-tauri/Cargo.toml`](../../src-tauri/Cargo.toml). Adding that feature is
  implementation work, not part of this research branch.
- **[verified]** The 1.1.0 provider release only made search optional; the
  error implementation did not change from 1.0.0. See the upstream
  [v1.0.0...v1.1.0 comparison](https://github.com/open-source-cooperative/windows-native-keyring-store/compare/v1.0.0...v1.1.0)
  and [v1.1.0 release](https://github.com/open-source-cooperative/windows-native-keyring-store/releases/tag/v1.1.0).

### The provider satisfies the data shape

- **[verified]** An explicit `target` modifier becomes the native target
  without service/user concatenation, and a `persistence=local` modifier is
  accepted. The default is Enterprise, so Local must be explicit:
  [credential construction](https://github.com/open-source-cooperative/windows-native-keyring-store/blob/65bf68219cab395e7b508f57df1aa0899d20face/src/cred.rs#L29-L72)
  and [store modifiers](https://github.com/open-source-cooperative/windows-native-keyring-store/blob/65bf68219cab395e7b508f57df1aa0899d20face/src/store.rs#L108-L130).
- **[verified]** `CredPersist::Local` is exactly
  `CRED_PERSIST_LOCAL_MACHINE`; raw secret validation rejects only lengths
  greater than `CRED_MAX_CREDENTIAL_BLOB_SIZE`, so 2,560 bytes pass and 2,561
  fail: [persistence and boundary source](https://github.com/open-source-cooperative/windows-native-keyring-store/blob/65bf68219cab395e7b508f57df1aa0899d20face/src/utils.rs#L23-L29),
  [binary validation](https://github.com/open-source-cooperative/windows-native-keyring-store/blob/65bf68219cab395e7b508f57df1aa0899d20face/src/utils.rs#L96-L104),
  and [binary credential methods](https://github.com/open-source-cooperative/windows-native-keyring-store/blob/65bf68219cab395e7b508f57df1aa0899d20face/src/cred.rs#L90-L112).
- **[verified]** Writes use `CRED_TYPE_GENERIC`, the caller's bytes, and the
  selected persistence; reads copy `CredentialBlob` bytes without string
  decoding: [write path](https://github.com/open-source-cooperative/windows-native-keyring-store/blob/65bf68219cab395e7b508f57df1aa0899d20face/src/utils.rs#L128-L175)
  and [read path](https://github.com/open-source-cooperative/windows-native-keyring-store/blob/65bf68219cab395e7b508f57df1aa0899d20face/src/utils.rs#L227-L255),
  including the [raw blob copy](https://github.com/open-source-cooperative/windows-native-keyring-store/blob/65bf68219cab395e7b508f57df1aa0899d20face/src/utils.rs#L299-L308).
- **[documented]** Microsoft defines `TargetName` plus `Type` as the credential
  identity, Generic blobs as application-defined bytes, the blob maximum as
  `5*512`, and `CRED_PERSIST_LOCAL_MACHINE` as persistence across subsequent
  logon sessions for the same user on the same machine:
  [`CREDENTIALW`](https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw).
- **[documented]** `CredWriteW` creates or replaces a credential with the same
  TargetName and Type: [`CredWriteW`](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credwritew).

### The provider does not expose its numeric error

- **[verified]** The provider does capture `GetLastError` immediately after a
  failed native BOOL result. It maps only `ERROR_NOT_FOUND` to
  `keyring_core::Error::NoEntry` and `ERROR_NO_SUCH_LOGON_SESSION` to
  `NoStorageAccess`; every other code becomes `PlatformFailure` containing
  `PlatformError(pub u32)`:
  [provider error source](https://github.com/open-source-cooperative/windows-native-keyring-store/blob/65bf68219cab395e7b508f57df1aa0899d20face/src/utils.rs#L370-L404).
- **[verified]** `PlatformError` is unreachable to downstream code because it
  lives in private module `utils`; the crate publicly exports only `cred`,
  `CredPersist`, `store`, and `Store`:
  [crate exports](https://github.com/open-source-cooperative/windows-native-keyring-store/blob/65bf68219cab395e7b508f57df1aa0899d20face/src/lib.rs#L91-L97).
  A negative compile probe in
  `/tmp/audio-graph-wincred-provider-public-probe` attempted to name
  `windows_native_keyring_store::utils::PlatformError` and failed with Rust
  `E0603: module utils is private`.
- **[documented]** `keyring-core` deliberately carries platform failures as
  `Box<dyn Error + Send + Sync>` so clients can retrieve a concrete numeric
  platform error. It also exposes the broad `PlatformFailure`,
  `NoStorageAccess`, and payload-free `NoEntry` variants:
  [`keyring-core` 1.0.0 error model](https://github.com/open-source-cooperative/keyring-core/blob/eb41b5cd54694c1622d3c30c59f2e87368463151/src/error.rs#L1-L40).
- **[inferred]** A trait object can be downcast only by naming its concrete
  type. Because the provider's concrete type is private and it supplies no
  numeric accessor, downstream code cannot distinguish code 5, 1223, or an
  unknown code structurally. `Display`, `Debug`, or message parsing is the only
  remaining observation channel, and that is not a stable API or an acceptable
  security boundary.

Verdict: **the wrapper exposes stable structure only for missing (1168) and
unavailable/no-logon-session (1312), not stable public numeric errors in
general.**

## Required direct-adapter shape

The prototype is in `/tmp/audio-graph-wincred-adapter`. It is intentionally not
product code and made no repository dependency or lockfile changes.

1. Implement one exact-target `CredentialApi` object. Reject empty targets,
   embedded NUL, and targets over the Generic UTF-16 limit before FFI. Do not
   reconstruct a target from service/user fields.
2. Use `CRED_TYPE_GENERIC`, an unmodified byte slice, and
   `CRED_PERSIST_LOCAL_MACHINE`. Reject `secret.len() > 2560` before FFI;
   accept 2,560 exactly.
3. Use raw BOOL projections with the exact 0.62.2 `windows` types. In each
   read/write/delete shim, branch on the BOOL and call `GetLastError` as the
   first action on the failure branch. Copy the numeric `WIN32_ERROR.0` before
   returning. Do not log, allocate, format, free, or call any other API first.
4. Box a **public, AudioGraph-owned** `NativeFailure { operation, code, kind }`
   inside `keyring-core::Error`. Callers can downcast it by type; they never
   inspect its prose.
5. Copy a successful read's exact blob bytes, then clear/free native and Rust
   buffers according to the credential-service memory policy. `CredReadW`
   returns one allocated buffer that must be released with `CredFree`:
   [`CredReadW`](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credreadw).

The critical failure shim is this shape:

```rust
let status = unsafe { cred_read_w(target, CRED_TYPE_GENERIC, 0, out) };
if !status.as_bool() {
    let code = unsafe { GetLastError() }; // first failure-branch action
    return Err(NativeFailure::from_win32(Operation::Read, code));
}
```

**[documented]** Microsoft says `GetLastError` is per-thread and must be called
immediately when the failed function's return value says the code is useful;
an intervening successful function can overwrite it:
[`GetLastError`](https://learn.microsoft.com/en-us/windows/win32/api/errhandlingapi/nf-errhandlingapi-getlasterror).

**[verified]** The 0.62.2 projection supplies the exact `CredDeleteW`,
`CredReadW`, `CredWriteW`, `CREDENTIALW`, 2,560-byte, and Local-persistence
types/constants used by the prototype:
[credential functions](https://github.com/microsoft/windows-rs/blob/32c3144490c016fe496a0aed769bce60987a2e9d/crates/libs/windows/src/Windows/Win32/Security/Credentials/mod.rs#L1-L16),
[read](https://github.com/microsoft/windows-rs/blob/32c3144490c016fe496a0aed769bce60987a2e9d/crates/libs/windows/src/Windows/Win32/Security/Credentials/mod.rs#L164-L170),
[write](https://github.com/microsoft/windows-rs/blob/32c3144490c016fe496a0aed769bce60987a2e9d/crates/libs/windows/src/Windows/Win32/Security/Credentials/mod.rs#L336-L340),
[structure](https://github.com/microsoft/windows-rs/blob/32c3144490c016fe496a0aed769bce60987a2e9d/crates/libs/windows/src/Windows/Win32/Security/Credentials/mod.rs#L922-L942),
and [limits/persistence](https://github.com/microsoft/windows-rs/blob/32c3144490c016fe496a0aed769bce60987a2e9d/crates/libs/windows/src/Windows/Win32/Security/Credentials/mod.rs#L1251-L1258).

**[verified]** The generated `windows` wrappers are not stale-error hazards by
themselves: each raw BOOL is immediately converted with `BOOL::ok()`, which
constructs an error from thread-local `GetLastError` in the same call chain:
[generated write wrapper](https://github.com/microsoft/windows-rs/blob/32c3144490c016fe496a0aed769bce60987a2e9d/crates/libs/windows/src/Windows/Win32/Security/Credentials/mod.rs#L336-L340),
[`BOOL::ok`](https://github.com/microsoft/windows-rs/blob/32c3144490c016fe496a0aed769bce60987a2e9d/crates/libs/result/src/bool.rs#L16-L23),
and [`HRESULT::from_thread`](https://github.com/microsoft/windows-rs/blob/32c3144490c016fe496a0aed769bce60987a2e9d/crates/libs/result/src/hresult.rs#L115-L121).
The prototype nevertheless keeps the raw BOOL because HRESULT conversion is an
unnecessary encoding step when the contract is to retain the exact 32-bit
`GetLastError` value, including an unknown code; positive Win32 values are
encoded with a low-16-bit mask by
[`HRESULT::from_win32`](https://github.com/microsoft/windows-rs/blob/32c3144490c016fe496a0aed769bce60987a2e9d/crates/libs/result/src/hresult.rs#L128-L134).

### Closed structural mapping

| Numeric result | `keyring-core` representation | AudioGraph result | Evidence status |
| --- | --- | --- | --- |
| `ERROR_NOT_FOUND` (1168) | `NoEntry` | `missing` | **[documented]** Read/delete explicitly define it as no matching target; [CredReadW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credreadw), [CredDeleteW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-creddeletew). |
| `ERROR_ACCESS_DENIED` (5) | `NoStorageAccess(Box<NativeFailure>)` | `access_denied` | **[inferred]** The numeric meaning is [documented by Microsoft](https://learn.microsoft.com/en-us/windows/win32/debug/system-error-codes--0-499-), but these three credential functions do not list it; classify defensively if observed or injected. |
| `ERROR_CANCELLED` (1223) | `PlatformFailure(Box<NativeFailure>)` | `cancelled` | **[inferred]** The numeric meaning is [documented by Microsoft](https://learn.microsoft.com/en-us/windows/win32/debug/system-error-codes--1000-1299-), but the non-UI credential functions do not list it; do not manufacture it from timeouts or text. |
| `ERROR_NO_SUCH_LOGON_SESSION` (1312) | `NoStorageAccess(Box<NativeFailure>)` | `store_unavailable` | **[documented]** Read/write/delete list a missing credential set or logon session; [CredReadW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credreadw), [CredWriteW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credwritew), [CredDeleteW](https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-creddeletew). |
| Any other numeric code | `PlatformFailure(Box<NativeFailure>)` | content-free `backend_failure` / internal | **[documented]** Microsoft warns function error lists are incomplete and can vary by OS or driver; [GetLastError remarks](https://learn.microsoft.com/en-us/windows/win32/api/errhandlingapi/nf-errhandlingapi-getlasterror). |

Only native 1168 becomes `missing`. Invalid flags/parameters, a bad username,
permission failures, cancellation, and unknown failures must never collapse to
missing.

## Compile proof

- **[verified]** `/tmp/audio-graph-wincred-adapter` depends on exact
  `keyring-core = 1.0.0` and `windows = 0.62.2` with
  `Win32_Foundation` and `Win32_Security_Credentials`. It implements
  `CredentialApi` over direct read/write/delete shims and compiled under
  `rustc 1.96.0`, host `x86_64-pc-windows-msvc`.
- **[verified]** `cargo check --tests` completed on the Windows toolchain.
- **[verified]** `cargo test --lib` ran four pure contract tests on Windows:
  exact UTF-16 target/no embedded NUL, inclusive 2,560-byte binary boundary,
  Local persistence value 2, and structural mapping/downcast for 1168, 5,
  1223, 1312, and unknown/internal. Result: **4 passed, 0 failed**.
- **[verified]** The tests do not call the live Credential Manager. They prove
  compilation and deterministic adapter contracts, not packaged OS behavior.

## Adversarial pass and limitations

- **[documented]** Microsoft explicitly says per-function error-code lists are
  incomplete. The closed mapper must retain the numeric code internally and
  send all unknowns to one content-free backend failure; expanding the mapping
  requires new primary evidence plus a regression test.
- **[inferred]** Access-denied and cancelled mappings are safe classifications
  if those exact codes arrive, but they are not evidence that `CredReadW`,
  `CredWriteW`, or `CredDeleteW` will produce them. Cancellation caused by
  AudioGraph's worker protocol is a separate typed orchestration outcome.
- **[verified]** The provider warns that concurrent same-entry operations and
  immediate persistence changes may reorder or transiently fail. The existing
  ADR requirement for one serialized credential worker remains necessary:
  [provider warning](https://github.com/open-source-cooperative/windows-native-keyring-store/blob/65bf68219cab395e7b508f57df1aa0899d20face/src/lib.rs#L75-L87).
- **[documented]** Windows treats Generic `TargetName` case-insensitively.
  AudioGraph must emit its canonical fixed prefix and stable identifier
  spelling consistently; "exact target" does not make Windows comparison
  case-sensitive: [`CREDENTIALW.TargetName`](https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw).
- **[inferred]** The cheapest decisive runtime experiment is a packaged Windows
  test executable with an injectable native-call seam. Inject each numeric
  failure to prove IPC-safe classification, then separately perform live
  save/read/replace/delete/restart at 2,560 bytes and a missing-target read.
  Treat access-denied/cancelled as injected-only unless a supported real setup
  reproduces them. This experiment is a release gate, not needed to decide the
  adapter shape.

## Rejected alternatives

- **Keep the provider and parse `Display`/`Debug`: rejected.** Its strings are
  not a typed or semver-stable numeric API, may change or localize, and would
  couple security behavior to prose.
- **Call `GetLastError` after the provider returns: rejected.** The provider
  call, Rust error construction, allocation, formatting, or any later API may
  have replaced the thread-local value. Microsoft requires immediate capture.
- **Treat all `PlatformFailure` as missing: rejected.** That violates ADR-0035
  and could silently convert denied/unavailable/internal failure into apparent
  credential absence.
- **Fork or patch the provider: rejected for this decision.** Exporting its
  numeric error would solve reachability, but AudioGraph would own an upstream
  fork for a very small FFI surface. A local adapter is smaller, explicit, and
  can carry the product's closed error contract directly.
- **Use the generated `windows::core::Error` message: rejected.** The 0.62.2
  generated BOOL wrappers do capture thread error in-call, but consuming their
  message is still prose. An exact raw `WIN32_ERROR` copied immediately is the
  narrowest auditable seam.

## Open risks and out-of-scope findings

- Packaged Windows behavior, session transitions, policy denial, and restart
  persistence remain release evidence; this source/compile study does not
  claim them.
- The product adapter must zero temporary write buffers and the returned native
  blob before `CredFree`, without delaying failure-code capture.
- No new out-of-scope discovery required a Seed. Per assignment, this branch
  made no Seed, product, Cargo manifest, or lockfile edits.
