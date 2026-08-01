# macOS Keychain interaction guard and typed `OSStatus` mapping

Date: 2026-08-01

Seed: `audio-graph-95e0`

Scope: one load-bearing unknown for ADR-0035; research only

## Question and gated decision

**Question.** What exact guard and error boundary can make one serialized macOS
login-Keychain operation honor `ForbidPrompt` without corrupting the
process-wide interaction setting, including operation error and Rust unwind;
and can the locked `apple-native-keyring-store` 1.0.1 / `security-framework`
3.7.0 stack preserve the numeric failures needed by AudioGraph's closed error
model?

**Decision gated.** Whether the macOS implementation child of ADR-0035 may use
the locked provider/convenience guard, or must own a thin checked legacy
Keychain adapter and restoration guard before native credential work begins.

## Recommendation

**Use a thin AudioGraph-owned adapter with raw interaction control and checked
status-preserving Keychain primitives from `security-framework-sys` 2.17.0 and
`security-framework` 3.7.0. Do not use
`security_framework::SecKeychain::disable_user_interaction`, and do not use
`apple-native-keyring-store` 1.0.1 for the replace/delete primitives.** Keep the
ADR-selected user/login file-based Keychain for this migration, but make every
native return status observable and structurally classified inside the adapter.

The `ForbidPrompt` helper must:

1. reject ordinary work if the serialized worker is poisoned;
2. read the prior interaction state;
3. arm restoration to that exact value **before** attempting to set `false`;
4. set `false`, run exactly one operation only after that succeeds, and make one
   checked restoration to the exact prior value;
5. restore from `Drop` during unwind and poison on a failed unwind restoration;
6. on a disable failure, skip the operation but still make one checked restore
   attempt because the setter's resulting state is not proven;
7. on any normal restore failure, discard any secret-bearing success value,
   retain only a safe operation disposition (`not_run`, `succeeded`, or
   `failed`), poison the worker, and return `internal_backend_failure`; and
8. leave `AllowPrompt` untouched by this guard. It is allowed only on the
   separate user-initiated recovery path and remains serialized.

Use numeric `OSStatus`, operation context, and public `Error::code()` where a
high-level error is unavoidable. Never branch on `Display`, `Debug`,
`SecCopyErrorMessageString`, or provider prose. Guard-control failures are
adapter failures regardless of their numeric value; only a credential
operation's status enters the platform-to-domain table below.

**Confidence: high (0.94) for the source-level design and compile-tested state
machine; medium (0.68) for runtime status incidence on supported macOS
releases.** The latter remains a packaged-platform release test, not a claim of
this research.

## Exact guard contract

The worker owns the state transition, not the caller:

```text
AllowPrompt:
  reject_if_poisoned -> operation

ForbidPrompt:
  reject_if_poisoned
  prior = checked GetUserInteractionAllowed
  arm Drop => checked SetUserInteractionAllowed(prior); poison on failure
  checked SetUserInteractionAllowed(false)
    failure -> checked SetUserInteractionAllowed(prior)
             -> return disable failure; poison only if restore failed
  operation (exactly one)
  checked SetUserInteractionAllowed(prior)
    success -> return operation result
    failure -> poison; discard secret result; return restore failure plus
               safe operation disposition
  disarm Drop only after the explicit restore call has been attempted
```

The production type should separate guard control from the operation result.
A suitable shape is `Result<T, GuardFailure<E>>`, where a restore failure
carries only its control phase and `OperationDisposition`, never `T`; this
prevents a successful secret read from being retained inside a failure. The
private native status may feed the classifier, but the public/IPC/loggable
error contains only the closed AudioGraph code and safe recovery action.

### Required outcomes

| Path | Required calls | Operation runs? | Poison? |
| --- | --- | ---: | ---: |
| `AllowPrompt` | operation only | yes | no |
| prior `true`, success/error | get, set `false`, operation, set `true` | yes | no |
| prior `false`, success/error | get, set `false`, operation, set `false` | yes | no |
| get failure | get | no | no; no state was changed |
| disable failure, restore success | get, set `false` fails, set prior succeeds | no | no |
| disable failure, restore failure | get, set `false` fails, set prior fails | no | yes |
| operation returns, restore failure | get, set `false`, operation, set prior fails | yes | yes |
| operation unwinds, restore success | `Drop` sets exact prior | yes | no |
| operation unwinds, restore failure | `Drop` checks status and sets poison | yes | yes |

The worker checks poison before **all** ordinary requests, including an
otherwise interactive request. Only the separately designed explicit recovery
or process restart may clear the state. There is no automatic second restore:
after a failed restore the setting is uncertain, and an implicit retry would
turn a safety failure into untracked mutation.

## Typed `OSStatus` boundary

The mapping is deliberately narrow and operation-aware. Names and numbers are
from Apple's current `Security/SecBase.h`; unknown or context-inconsistent
values fail closed as `internal_backend_failure`.

| Native status | Numeric value | AudioGraph code | Context constraint |
| --- | ---: | --- | --- |
| `errSecItemNotFound` | -25300 | `missing` | Credential find/read only; this is the sole native missing result. |
| `errSecInteractionNotAllowed`, `errSecInteractionRequired` | -25308, -25315 | `locked` | Credential operation only. Under `ForbidPrompt`, both mean user action would be required but is unavailable. |
| `errSecUserCanceled` | -128 | `cancelled` | Interactive operation only. |
| `errSecWrPerm`, `errSecReadOnly`, `errSecAuthFailed`, `errSecReadOnlyAttr`, `errSecDataNotModifiable`, `errSecMissingEntitlement`, `errSecRestrictedAPI` | -61, -25292, -25293, -25309, -25317, -34018, -34020 | `access_denied` | A known permission/authentication/modification denial. |
| `errSecNotAvailable`, `errSecNoSuchKeychain`, `errSecInvalidKeychain`, `errSecNoDefaultKeychain`, `errSecNoStorageModule`, `errSecInDarkWake`, `errSecServiceNotAvailable` | -25291, -25294, -25295, -25307, -25312, -25320, -67585 | `store_unavailable` | Store or required service cannot be used; never `missing`. |
| `errSecUnimplemented`, `errSecWrongSecVersion` | -4, -25310 | `store_unsupported` | The operation/store format is unsupported. |
| `errSecDecode` | -26275 | `corrupt_record` | Credential read/decode only; application-envelope decoding remains the stronger source of this code. |
| `errSecDataTooLarge` | -25302 | `payload_too_large` | Add/update only, after AudioGraph's portable-size precheck. In any other context it is internal failure. |
| `errSecDuplicateItem` | -25299 | `conflict` | Direct add only, and only after an exact preceding find returned `errSecItemNotFound`; otherwise internal failure. |
| every other status | any | `internal_backend_failure` | No prose fallback and no range-based inference. |

Every Get/Set interaction-control failure maps to
`internal_backend_failure` with a safe internal phase (`read_prior`, `disable`,
`restore`), even if its numeric status also appears in the credential-operation
table. A restore failure additionally sets `worker_poisoned`. Raw status numbers
from live failures must not be serialized into logs, IPC, analytics, Seeds, or
user text. The static named-value table in this research is design evidence,
not a runtime diagnostic channel.

The Rust implementation should centralize the private constants. Use
`security-framework-sys::base` constants where 2.17.0 exposes them and define
the missing named constants once from the pinned Apple header; do not scatter
numeric literals through match arms. The raw Get/Set signatures use
CoreFoundation `Boolean` (`u8`) and return `OSStatus` (`i32`).

### Direct primitive contract

The owned adapter need not rewrite safe CoreFoundation lifetime handling. It
may use high-level helpers only when they preserve each `OSStatus`:

- resolve the User-domain default Keychain with a checked result;
- call `find_generic_password`; on success, update through the checked
  `SecKeychainItem::set_password` method;
- call add only when that find's exact numeric code is
  `errSecItemNotFound`; classify and stop on every other find failure;
- for any required physical cleanup, call the raw status-returning
  `SecKeychainItemDelete` and check it rather than the `delete() -> ()`
  convenience method; and
- run each complete find/add-or-update or find/delete sequence as the one
  serialized operation inside the interaction guard.

**[Verified]** The locked sys crate exposes find/add and the status-returning
modify/delete primitives; the high-level `set_password` checks modify status.
[`security-framework-sys` find/add declarations](https://github.com/kornelski/rust-security-framework/blob/5f6e65114b77d5bc161d2b099cad09f2a67609d2/security-framework-sys/src/keychain.rs#L166-L215),
[`security-framework-sys` modify/delete declarations](https://github.com/kornelski/rust-security-framework/blob/5f6e65114b77d5bc161d2b099cad09f2a67609d2/security-framework-sys/src/keychain_item.rs#L28-L44),
[`security-framework` checked `set_password`](https://github.com/kornelski/rust-security-framework/blob/5f6e65114b77d5bc161d2b099cad09f2a67609d2/security-framework/src/os/macos/passwords.rs#L65-L77)

## Evidence

### Apple contract and implementation shape

- **[Documented]** Apple documents the setter as controlling Keychain Services
  functions that would otherwise show UI. `false` makes such calls return an
  error; the documented default is `true`; and Apple warns that failing to
  restore interaction affects other Keychain Services clients.
  [Apple: `SecKeychainSetUserInteractionAllowed`](https://developer.apple.com/documentation/security/seckeychainsetuserinteractionallowed%28_%3A%29?language=objc)

- **[Documented]** Apple documents the getter as returning the current
  interaction permission through an output `Boolean`; both Get and Set return
  `OSStatus` and are deprecated with the `SecKeychain` family.
  [Apple: `SecKeychainGetUserInteractionAllowed`](https://developer.apple.com/documentation/security/seckeychaingetuserinteractionallowed%28_%3A%29)

- **[Verified]** Apple's published header declares the exact C signatures and
  marks both APIs deprecated as of macOS 10.10. The same header describes
  `true`/`false` semantics; therefore restoring a hard-coded `true` is not
  equivalent to restoring the prior state.
  [Apple OSS `SecKeychain.h` at `db15acb`, lines 632-651](https://github.com/apple-oss-distributions/Security/blob/db15acbe6a7f257a859ad9a3bb86097bfe0679d9/OSX/libsecurity_keychain/lib/SecKeychain.h#L632-L651)

- **[Verified]** Apple's published implementation delegates both functions to
  one `globals()` object. Its `Globals` type stores one plain `mUI` Boolean and
  exposes direct get/set accessors; the constructor initializes it to `true`.
  This is process-global mutable state in that source snapshot, not a property
  of one `SecKeychain` object.
  [Apple OSS entry points](https://github.com/apple-oss-distributions/Security/blob/db15acbe6a7f257a859ad9a3bb86097bfe0679d9/OSX/libsecurity_keychain/lib/SecKeychain.cpp#L866-L885),
  [`Globals` storage](https://github.com/apple-oss-distributions/Security/blob/db15acbe6a7f257a859ad9a3bb86097bfe0679d9/OSX/libsecurity_keychain/lib/Globals.h#L43-L69),
  [default and credential selection](https://github.com/apple-oss-distributions/Security/blob/db15acbe6a7f257a859ad9a3bb86097bfe0679d9/OSX/libsecurity_keychain/lib/Globals.cpp#L36-L73)

- **[Inferred]** A single AudioGraph worker is necessary because the setting is
  shared and the Apple source snapshot does not put synchronization around
  `mUI`. It is not sufficient against unrelated same-process code that calls
  the legacy APIs outside AudioGraph's worker; that residual risk is explicit
  below.

- **[Verified]** Apple's header provides stable named integer values for the
  statuses in the table, including both interaction errors, missing,
  cancellation, permissions, unavailable-store, duplicate, and decode cases.
  [Apple OSS `SecBase.h`, lines 324-392](https://github.com/apple-oss-distributions/Security/blob/db15acbe6a7f257a859ad9a3bb86097bfe0679d9/base/SecBase.h#L324-L392)

- **[Documented]** The setter documentation specifically says a suppressed
  legacy call that needs unlock UI can return `errSecInteractionRequired`.
  Apple's result-code reference separately defines
  `errSecInteractionNotAllowed`. Mapping both on a guarded credential operation
  avoids treating prompt suppression as absence.
  [Apple setter documentation](https://developer.apple.com/documentation/security/seckeychainsetuserinteractionallowed%28_%3A%29?language=objc),
  [Apple Security framework result codes](https://developer.apple.com/documentation/security/security-framework-result-codes)

- **[Documented]** TN3137 says the `SecKeychain` APIs always target the
  file-based Keychain, recommends `SecItem` for new work, and describes the
  file-based implementation as heading toward deprecation. It also explains
  that macOS's file-based and data-protection implementations have materially
  different behavior. This supports a bounded legacy adapter now and a separate
  future migration decision, not an unreviewed switch in this Seed.
  [Apple TN3137](https://developer.apple.com/documentation/Technotes/tn3137-on-mac-keychains)

### Exact locked Rust source

- **[Verified]** The repository lock resolves
  `apple-native-keyring-store` 1.0.1, `keyring-core` 1.0.0,
  `security-framework` 3.7.0, and `security-framework-sys` 2.17.0 with registry
  checksums. These, not `latest` documentation, are the evaluated artifacts.
  [`src-tauri/Cargo.lock`](../../src-tauri/Cargo.lock)

- **[Verified]** `security-framework-sys` 2.17.0 exposes the exact raw Get and
  Set functions under `target_os = "macos"`. Its CoreFoundation dependency
  defines `Boolean` as `u8`; the Apple C header likewise defines it as an
  unsigned byte.
  [`security-framework-sys` source at the crate commit](https://github.com/kornelski/rust-security-framework/blob/5f6e65114b77d5bc161d2b099cad09f2a67609d2/security-framework-sys/src/keychain.rs#L236-L246),
  [Apple CoreFoundation `Boolean`](https://github.com/apple-oss-distributions/CF/blob/dc54c6bb1c1e5e0b9486c1d26dd5bef110b20bf3/CFBase.h#L89-L100)

- **[Verified]** The high-level `security-framework` 3.7.0 convenience guard is
  not acceptable: acquisition always writes `false`, while its zero-sized
  guard's `Drop` always writes `true` and ignores the restore status. It neither
  snapshots nor restores an exact prior `false` value.
  [`disable_user_interaction`](https://github.com/kornelski/rust-security-framework/blob/5f6e65114b77d5bc161d2b099cad09f2a67609d2/security-framework/src/os/macos/keychain.rs#L95-L120),
  [`KeychainUserInteractionLock::drop`](https://github.com/kornelski/rust-security-framework/blob/5f6e65114b77d5bc161d2b099cad09f2a67609d2/security-framework/src/os/macos/keychain.rs#L239-L249)

- **[Verified]** `security_framework::base::Error` exposes a public numeric
  `code()` method. Its `Display` path obtains localized text separately, so
  numeric classification is both available and clearly distinct from prose
  parsing.
  [`security-framework` `Error`](https://github.com/kornelski/rust-security-framework/blob/5f6e65114b77d5bc161d2b099cad09f2a67609d2/security-framework/src/base.rs#L27-L85)

- **[Verified]** `security-framework-sys` 2.17.0 defines only a subset of
  `SecBase.h` status constants. The interaction, cancellation, unavailable,
  read-only, and decode constants needed here are not all exposed, which is why
  one private, Apple-header-pinned constant module is required.
  [`security-framework-sys` constants](https://github.com/kornelski/rust-security-framework/blob/5f6e65114b77d5bc161d2b099cad09f2a67609d2/security-framework-sys/src/base.rs#L53-L69)

### Why the locked provider cannot own mutations

- **[Verified]** `apple-native-keyring-store` 1.0.1's legacy store targets the
  User/login Keychain by default and forwards set/get/delete to the
  `security-framework` legacy password API. Its delete method returns success
  after calling `item.delete()`.
  [`apple-native-keyring-store` keychain provider](https://github.com/open-source-cooperative/apple-native-keyring-store/blob/4ffc36fd678617c546b82ef1fad1f3833a6bf2f7/src/keychain.rs#L69-L103),
  [default User domain](https://github.com/open-source-cooperative/apple-native-keyring-store/blob/4ffc36fd678617c546b82ef1fad1f3833a6bf2f7/src/keychain.rs#L176-L209)

- **[Verified]** The called `security-framework` 3.7.0
  `set_generic_password` updates on a successful find but calls add after
  **every** find error, not only `errSecItemNotFound`. A locked, denied,
  unavailable, or internal lookup failure is therefore treated internally as
  an add path, violating ADR-0035's “failure is not absence” invariant before
  AudioGraph can classify the original status.
  [`set_generic_password`](https://github.com/kornelski/rust-security-framework/blob/5f6e65114b77d5bc161d2b099cad09f2a67609d2/security-framework/src/os/macos/passwords.rs#L263-L279)

- **[Verified]** The called legacy `SecKeychainItem::delete` returns `()` and
  discards `SecKeychainItemDelete`'s `OSStatus`. The provider consequently
  cannot report a failed physical delete after lookup.
  [`SecKeychainItem::delete`](https://github.com/kornelski/rust-security-framework/blob/5f6e65114b77d5bc161d2b099cad09f2a67609d2/security-framework/src/os/macos/passwords.rs#L65-L85)

- **[Verified]** The provider's own decoder does structurally inspect
  `Error::code()`. For most statuses it retains the concrete
  `security_framework::base::Error` in a boxed `PlatformFailure` or
  `NoStorageAccess`; item-not-found becomes semantic `NoEntry`. Thus prose
  parsing is unnecessary, but this later mapping cannot recover the original
  lookup error discarded by `set_generic_password` or the delete status never
  returned by `delete`.
  [`decode_error`](https://github.com/open-source-cooperative/apple-native-keyring-store/blob/4ffc36fd678617c546b82ef1fad1f3833a6bf2f7/src/keychain.rs#L360-L373),
  [`keyring-core` boxed error model](https://github.com/open-source-cooperative/keyring-core/blob/eb41b5cd54694c1622d3c30c59f2e87368463151/src/error.rs#L15-L40)

- **[Inferred]** Wrapping this provider with the correct interaction guard would
  suppress prompts, but it would not repair its replace/delete status loss.
  The smallest contract-complete choice is therefore one direct adapter for
  find/add/update/delete plus the guard, rather than a mixed wrapper/raw path.

## Compile-tested proof

**[Verified]** A throwaway, dependency-free Rust crate was built at
`/tmp/audio-graph-95e0-guard` with rustc 1.88.0 and Cargo 1.88.0 on
`x86_64-unknown-linux-gnu`. It models the production FFI behind an
`InteractionApi` spy, arms a `Drop` restoration guard before disable, explicitly
checks normal restoration, records poison through `AtomicBool`, discards the
operation value on restore failure, and classifies integer statuses without any
string operation.

Prototype hashes:

```text
70929447f31acfa885cabe1b8aac383587ee358799868e2493717bf2fee5217a  src/lib.rs
7488e1e24fa7a5ad0314a631741e46edcc71eb5da163da1ca86d4e5b1c08b311  Cargo.toml
```

Command and result:

```text
$ cargo test
running 11 tests
test result: ok. 11 passed; 0 failed; 0 ignored
```

Covered cases:

- `AllowPrompt` performs no interaction-state Get/Set;
- prior `true` and prior `false` are restored exactly after success;
- both prior values are restored after an operation error;
- Get failure runs no setter and no operation;
- disable failure runs no operation and makes one checked exact-state restore;
- disable plus restore failure poisons;
- restore failure after operation success and after operation error poisons and
  retains only the safe operation disposition;
- unwind restores prior `true` and prior `false` through `Drop`;
- unwind restore failure poisons, and a subsequent ordinary request is
  rejected; and
- named integer status classification is operation-aware, maps control failures
  only to internal failure, and maps unknown values to internal failure.

**[Inferred]** This proves Rust ownership, sequencing, unwind, failure, and
classification behavior independent of macOS. It does **not** prove that a
specific packaged macOS build links the intended framework symbols, that every
legacy Keychain path honors the switch on every supported release, which
numeric status a locked/denied live Keychain returns, or that no dialog appears.
Those are runtime/package claims below.

## Adversarial pass

1. **The Apple source snapshot is not the shipped binary.** **[Verified]** It
   shows a process-global Boolean and exact API wiring. **[Inferred]** Apple can
   change internals while preserving the public contract. The recommendation
   depends only on documented Get/Set behavior and treats the source layout as
   support for serialization, not as an ABI promise.

2. **The legacy setting is not an isolation primitive.** **[Inferred]** The
   worker serializes all AudioGraph credential calls, but cannot lock out an
   unrelated in-process library that calls Keychain Services directly. A code
   audit and packaged stress case must ensure no other AudioGraph path bypasses
   the worker. There is no supported cross-library mutex exposed by Apple.

3. **macOS Keychain status selection is contextual.** **[Documented]** Apple
   DTS describes cases where the SecItem shim combines file-based and
   data-protection results and can return item-not-found even when another
   backend returned interaction-not-allowed. The proposed adapter avoids that
   shim by staying on the ADR-selected explicit legacy functions, but live
   status incidence still needs package evidence.
   [Apple DTS: SecItem pitfalls and best practices](https://developer.apple.com/forums/thread/724013)

4. **`locked` is a domain classification, not a native synonym.**
   **[Inferred]** Apple's two interaction statuses establish only that user
   action is prohibited or required. On a guarded credential operation,
   `locked` is the safest existing AudioGraph recovery category because it
   routes to explicit diagnose/unlock and never to missing/fallback. Package
   tests must record actual outcomes for locked and ACL-denied items.

5. **A failed setter has no documented transactional guarantee.**
   **[Inferred]** Assuming a failed disable left the prior value unchanged is
   weaker than one checked restoration attempt. Conversely, repeatedly retrying
   a failed restoration hides uncertainty. The recommended single restore plus
   poison is the conservative boundary.

6. **Unwind safety is not process-crash safety.** **[Verified]** Rust `Drop`
   runs for unwind but not for abort or process termination. **[Inferred]** The
   Apple source models the setting as process state, so process exit does not
   leave a persistent Keychain preference; a hung native call, however, keeps
   the guard armed until it returns. ADR-0035's separate `stalled_worker` rule
   remains necessary.
   [Rust Reference: panic unwinding and destruction](https://doc.rust-lang.org/stable/reference/panic.html#unwinding),
   [Rust Reference: termination without destructors](https://doc.rust-lang.org/reference/destructors.html#process-termination-without-unwinding)

7. **The wrapper failures are correctness failures, not just diagnostic
   quality.** **[Verified]** A lost delete status can report successful cleanup
   that did not happen, while the catch-all update fallback can transform a
   locked/denied lookup into a duplicate/add result. A guard around those calls
   cannot reconstruct the missing information.

After this pass, two additional sources would not change the source-level
recommendation. The remaining uncertainty is empirical and needs the packaged
experiment, not more desk research.

## Rejected alternatives

- **`security_framework::KeychainUserInteractionLock`: rejected.** It restores
  `true` blindly and discards the restore status.
- **Guarding `apple-native-keyring-store` mutations: rejected.** It still loses
  the first non-missing error during replace and every physical-delete result.
- **Restoring `true` unconditionally: rejected.** It can enable UI that another
  client had deliberately disabled before AudioGraph's operation.
- **Skipping restoration on operation error: rejected.** Operation outcome and
  global-state custody are independent obligations.
- **Treating a failed disable as proof of no change: rejected.** Apple does not
  document that guarantee; attempt exact restoration and poison if it cannot be
  confirmed.
- **Parsing localized error text: rejected.** Numeric status is directly
  available; prose is unstable, potentially content-bearing, and forbidden by
  ADR-0035.
- **Mapping every provider `NoStorageAccess` to one domain code: rejected.** Its
  boxed `security_framework::base::Error` retains distinctions such as
  read-only, unavailable, invalid keychain, and write permission.
- **Moving to the data-protection Keychain in this Seed: rejected as scope
  expansion.** Apple recommends it for new work, but ADR-0035 explicitly keeps
  the login/file-based family for migration compatibility and requires a
  separate signed migration decision.

## Open risks and required packaged evidence

Source inspection and the Linux spy establish no macOS release claim. The
existing packaged-platform release gate, `audio-graph-c4c5`, must run the
cheapest decisive experiment after the adapter exists:

1. build the actual signed/package-identity AudioGraph artifact on each
   supported macOS architecture/release;
2. create only dummy AudioGraph credentials under its stable service/account;
3. exercise missing add, existing replace, read, physical cleanup where used,
   delete/tombstone flow, restart, and same-identity upgrade;
4. lock the login Keychain and run background `ForbidPrompt` read and replace
   while a UI observer asserts that no Keychain dialog appears and the call
   returns a closed non-missing code;
5. exercise ACL denial and an explicit `AllowPrompt` cancellation separately;
6. begin with interaction state `true` and `false`, then verify the exact prior
   value after success and native operation failure;
7. record artifact, OS, architecture, signing identity, dependency, and test
   hashes, while keeping raw native prose and dummy bytes out of product logs;
   and
8. fail the macOS parity/release claim on any prompt, missing/failure collapse,
   unchecked mutation result, state-restoration mismatch, or unsupported OS.

Restore failure remains deterministically injectable at the Rust boundary; it
need not be forced against a user's real Keychain. Live Get/Set and locked-state
behavior cannot be decided safely or honestly on this Linux host.

Other open risks:

- Apple has deprecated the control APIs and may eventually remove or alter
  legacy behavior; the target-specific adapter should remain small and gated.
- Same-process Keychain use introduced later can bypass worker serialization;
  repository review must treat new native Keychain calls as a guardrail breach.
- A native call can hang after interaction is disabled. The worker must remain
  stalled and reject retries until return/restart; a deadline cannot restore
  state while the call is still executing.
- The production interaction API must be a direct, non-panicking FFI/status
  adapter. A Rust panic from restoration code while `Drop` is already handling
  an operation panic can abort the process; `Drop` therefore records a failed
  `OSStatus` in poison state and never panics.
  [Rust `Drop` panic guidance](https://doc.rust-lang.org/stable/core/ops/trait.Drop.html#panics)
- The exact recovery UX for an interaction-required result versus a true ACL
  denial is package-dependent. Neither may become `missing` or plaintext
  fallback.

## Out-of-scope discoveries

None requiring a new Seed. The provider's replace/delete status loss directly
changes the gated adapter decision and is therefore in scope. Migration to the
data-protection Keychain is already reserved by ADR-0035 for a separate ADR;
packaged macOS behavior is already tracked by `audio-graph-c4c5`. Per assignment,
this research branch does not edit Seeds.

## Decision

The unknown is resolved: an exact-state, checked, unwind-safe guard is viable,
but the locked convenience guard and provider mutation paths are not. Proceed
with one serialized, AudioGraph-owned legacy Keychain adapter using raw checked
statuses, the state machine above, and private structural mapping. Do not claim
prompt-free packaged behavior until `audio-graph-c4c5` passes.
