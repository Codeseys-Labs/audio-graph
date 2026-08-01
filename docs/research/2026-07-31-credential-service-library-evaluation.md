# Tauri and Rust library evaluation for credential service v2

Date: 2026-07-31

Owner: `audio-graph-efeb`

Related work: ADR-0035, `audio-graph-fb2b`, `audio-graph-a6bf`, and
`audio-graph-98a9`

## Decision

Build the credential authority as a Rust service managed once in Tauri
application state. Use `keyring-core` with explicit native adapters for the
production secret bytes. Do not install a renderer-facing credential plugin.

The supporting stack is:

- Tauri `Manager::manage` / command `State` for one long-lived
  `Arc<CredentialService>`;
- `keyring-core` plus explicit Windows, macOS, and Linux adapters;
- `secrecy` and `zeroize` for redacted, short-lived in-memory containers;
- `url::Origin` plus a closed AudioGraph audience enum for exact HTTPS/WSS
  authorization;
- `reqwest::redirect::Policy::none()` on credential-bearing clients;
- one standard blocking thread driven by the already-present
  `crossbeam-channel`, not one `spawn_blocking` task per keychain operation;
- `fs4` for the cross-process journal/mutation lock; and
- `atomic-write-file` behind a narrow persistence wrapper for same-directory
  journal and file-v2 replacement, with explicit permission and durability
  tests on all three desktop platforms.

`uuid`, `serde`, `serde_json`, `thiserror`, `url`, `reqwest`,
`crossbeam-channel`, and `zeroize` are already present in the Rust dependency
graph. `secrecy`, `fs4`, and `atomic-write-file` are proposed additions owned by
the adapter/core workstreams; lockfile changes must stay in those worktrees.

## Evaluation matrix

| Candidate | Decision | Reason |
| --- | --- | --- |
| Tauri managed state | Adopt | It gives commands a reference to one application-owned Rust service. The service, not a plugin guest API, owns lifecycle, serialization, policy, and events. |
| `keyring-core` | Adopt | It exposes binary secret operations and explicit store selection, which AudioGraph needs for typed adapters, injected tests, Windows target/persistence control, and conservative error mapping. |
| Explicit platform keyring stores | Adopt behind AudioGraph adapters | Use `windows-native-keyring-store` with an explicit Local generic target and `apple-native-keyring-store` for the current login-keychain path. Linux needs a thinner Secret Service adapter because the high-level store may unlock/prompt. |
| `secrecy` | Adopt | Secret values require explicit exposure, have redacted formatting, and are not casually serializable. AudioGraph supplies an explicit protected-envelope encoder instead of deriving response serialization. |
| `zeroize` | Retain | `Zeroizing<Vec<u8>>` and zeroizing domain fields reduce copies and lifetime around encoding, native calls, and transport construction. This is hygiene, not a promise that every OS/TLS/provider copy is erased. |
| `url` | Adopt for canonicalization | `url::Origin::Tuple` represents scheme/host/port without path/query substring routing. AudioGraph still applies its own closed provider, HTTPS/WSS, port, and custom-binding policy. |
| reqwest no-follow policy | Adopt | Reqwest follows redirects by default. Credential-bearing clients explicitly use `Policy::none()` so security does not depend on a library default or header-stripping heuristic. |
| Dedicated thread + `crossbeam-channel` | Adopt | Native credential APIs are blocking and must be serialized. A started `spawn_blocking` task cannot be cancelled, so per-call tasks would make timeout/retry ordering harder to reason about. A stalled dedicated worker becomes one typed service state and permits no competing mutation. |
| `fs4` | Adopt behind a wrapper | It provides cross-platform advisory file locks for cooperating AudioGraph v2 processes. The lock covers journal load, expected-revision check, native write/readback, and journal commit. It does not claim protection against credential tools that ignore the lock. |
| `atomic-write-file` | Adopt behind a wrapper, platform-gated | It supplies the same-directory temporary-write/sync/rename shape needed by the non-secret authority journal and explicit file-v2 backend. AudioGraph tests permissions, replacement, directory durability where supported, and crash recovery rather than treating the crate name as proof. |
| Tauri Stronghold plugin | Defer | Stronghold is a capable encrypted vault, but it introduces snapshot-password/KDF or master-key custody plus unlock, backup, and recovery UX that this product has not designed. It becomes relevant if a real protected bundle exceeds the portable native-store limit or encrypted export is required. |
| Tauri Store plugin | Reject for secrets/journal authority | It is a persistent key-value store with frontend APIs and save behavior, not an OS credential service or a security-authoritative crash journal. It may remain suitable for ordinary non-secret UI preferences. |
| `tauri-plugin-keyring` | Reject | Its renderer API can get/set a password for caller-supplied service/user identifiers. That is the inverse of the closed, backend-owned authority required here. |
| `tauri-plugin-keyring-store` | Reject as a dependency | Its Rust-first implementation is useful prior art, but its Stronghold-shaped guest API includes raw account operations and plaintext import/export over IPC. AudioGraph must not expose those capabilities to the renderer. |
| Mobile-oriented Tauri keychain plugins | Reject for this desktop rebuild | They do not replace the required Windows Credential Manager, macOS login Keychain, and Linux Secret Service contract or packaged desktop evidence. |

## Platform adapter notes

### Windows

Use binary `set_secret`/`get_secret` through an explicit generic-credential
target rooted at `Codeseys.AudioGraph.Credentials/`, with Local persistence.
Reject the final protected envelope above 2,560 bytes before calling Windows.
The AudioGraph wrapper maps only known numeric outcomes and otherwise returns a
content-free backend failure; it never parses native error prose.

### macOS

Keep the current login-keychain family for this migration. The stock store does
not expose the interaction policy required by ADR-0035, so the adapter adds a
small Security-framework guard around background calls:

1. read `SecKeychainGetUserInteractionAllowed`;
2. call `SecKeychainSetUserInteractionAllowed(false)`;
3. perform one serialized `ForbidPrompt` operation; and
4. restore the prior value even during unwinding.

The process-global flag means all AudioGraph keychain calls must share the one
worker. Failure to restore is not success; it poisons the worker until explicit
recovery or restart. Because these legacy APIs are deprecated and signing/access
behavior is platform-specific, packaged locked/denied/update tests remain a
release gate. A future move to the data-protection keychain needs its own ADR.

### Linux

Use exact Secret Service attributes and keep labels non-secret. The adapter must
distinguish locked items from missing items without invoking Unlock/Prompt for
background work. Only an explicit user action may enter an interactive path,
and the implementation retains the prompt handle so dismissal/cancellation can
be reported. Missing session bus, missing service, and missing default
collection remain `store_unavailable`, not `missing` and never file fallback.

## Tauri security boundary

Tauri capabilities authorize command invocation; they do not make the renderer
a trusted secret principal. The command surface exposes typed status,
replacement drafts, delete, migration, and explicit recovery. It does not expose
saved-secret get/export, raw service/account selection, arbitrary set locators,
or a generic operation that accepts both a stored-set id and renderer-selected
destination.

Custom saved credentials use a backend-issued `custom.<uuid>` set and keep the
normalized HTTPS/WSS audience inside the protected bundle. Changing that
audience creates a new set with a complete secret. Until `audio-graph-98a9`
lands, custom endpoints remain invocation-draft-only.

## Required proof before dependency acceptance

- dependency audit and feature-minimal build for each target;
- unit tests for redacted formatting, explicit exposure, and zeroization seams;
- cross-process lock contention, timeout, crash-release, stale expected
  revision, and lock-ignoring actor tests;
- journal atomic-replacement and injected interruption tests;
- 2,560/2,561-byte native-envelope boundary tests;
- macOS interaction-guard restore/poison tests plus packaged no-prompt proof;
- Linux locked/absent-service/prompt-dismissed proof; and
- generated IPC/capability audit proving no saved-secret read/export or raw
  locator command is registered.

## Primary sources

- Tauri managed state:
  https://docs.rs/tauri/2.11.0/tauri/trait.Manager.html
- Tauri Stronghold plugin:
  https://v2.tauri.app/plugin/stronghold/
- Tauri Store JavaScript API and Rust crate:
  https://v2.tauri.app/reference/javascript/store/
  and https://docs.rs/tauri-plugin-store/latest/
- `keyring-core`:
  https://docs.rs/keyring-core/1.0.0/keyring_core/
- Windows native store:
  https://docs.rs/windows-native-keyring-store/1.1.0/windows_native_keyring_store/
- Apple native store:
  https://docs.rs/apple-native-keyring-store/1.0.0/apple_native_keyring_store/keychain/
- Secret Service specification:
  https://specifications.freedesktop.org/secret-service/latest-single/
- Apple interaction control:
  https://developer.apple.com/documentation/security/seckeychaingetuserinteractionallowed%28_%3A%29?language=objc
  and
  https://developer.apple.com/documentation/security/seckeychainsetuserinteractionallowed%28_%3A%29?language=objc
- `secrecy` and `zeroize`:
  https://docs.rs/secrecy/0.10.3/secrecy/
  and https://docs.rs/zeroize/1.9.0/zeroize/
- URL origins and reqwest redirects:
  https://docs.rs/url/latest/url/enum.Origin.html
  and https://docs.rs/reqwest/latest/reqwest/redirect/
- Tokio blocking-task cancellation limitation:
  https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html
- `fs4` and `atomic-write-file`:
  https://docs.rs/fs4/1.1.0/fs4/
  and https://docs.rs/atomic-write-file/0.3.0/atomic_write_file/
- Community plugins reviewed and rejected:
  https://github.com/HuakunShen/tauri-plugin-keyring
  and https://docs.rs/crate/tauri-plugin-keyring-store/latest
