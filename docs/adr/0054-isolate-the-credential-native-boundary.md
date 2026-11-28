---
status: accepted
date: 2026-11-28
deciders: [AudioGraph maintainers]
---

# ADR-0054: Isolate the Credential Native Boundary

This addendum changes how [ADR-0053](0053-use-a-purpose-built-linux-secret-service-client.md)
enforces its prompt rule. ADR-0053 still defines the protocol, daemon
qualification, and prompt behavior. The move and its prerequisites are
specified in the [move map](../designs/credential-native-move-map.md).

## Context and Problem Statement

Credential v2 may request a Linux Secret Service prompt only during explicit
recovery. Ordinary reads, writes, and deletes cannot unlock or show a prompt.
The source inventory was intended to protect that rule. Two independent
reviews found 22 bypasses in total. The first twelve were addressed, and ten
new ones passed the revised inventory. Six also passed the full credential
test filter. The second review exercised macro expansion, alternate include
and module paths, computed method names, and access from descendants of the
gate. Three reviewers independently chose the same replacement and fallback.
[Decision on #152](https://github.com/Codeseys-Labs/audio-graph/issues/152#issuecomment-7241091454)

## Decision Drivers

- Exclude raw zbus calls and forged gate capabilities from the app crate by
  checking the compiled dependency and visibility boundaries.
- Keep the authority journal, runtime admission, and public credential
  errors intact during relocation.
- Preserve one redaction scope, the current platform policy, and the
  dormant Linux factory while the new checks are qualified.

## Considered Options

- **Use a workspace crate.** Cargo controls where a dependency is available
  and Rust controls access to the gate internals. This adds a crate boundary
  to the app's native lease and test paths and requires a portable compiler
  input check.
- **Extend the seal to the subtree and add a Clippy lint.** This is the
  fallback and requires fewer app-side changes. It improves the review
  footprint but leaves zbus available throughout the app. Lint expectations
  and macro coverage remain review obligations.
- **Keep expanding the source inventory.** This preserves the layout and
  catches known spellings. The two reviews established that the checked text
  can diverge from what Rust compiles.

## Decision Outcome

Put the gate, the native Keychain and Credential Manager FFI, the platform
policy adapters, the entry boundary, and the Linux Secret Service client in
`src-tauri/crates/credential-native`. Keep the gate as a module with no
production children. Its seal, constructors, fields, and state manipulation
stay private. Sibling policy modules receive borrowed capabilities. An app
type with the same name has no authority over a crate-owned capability.

The app declares no zbus version. Locked Cargo metadata must show exactly
one direct dependent of the resolved zbus 6 package, namely
`credential-native`. This applies to renamed and target-specific edges. The
normal graph must exclude test support. Macros and files compiled as part of
the app resolve dependencies in the app's compilation context.

The Linux wire uses a single private send function taking the pinned
`ServiceOwner`. Ordinary and interactive methods have distinct closed enums.
The interactive enum covers `Unlock`, `Prompt`, and `Dismiss` and its callers
hold `&AllowPrompt`. The ordinary cleanup route can dismiss a returned
mutation prompt without displaying it. The shared send preserves
`NO_AUTO_START`, omits interactive authorization, and disables property
caching. Crate-level Clippy rules for raw calls and include macros provide a
secondary check with a production expectation at the send site.

The app pins a digest of the whole native crate using sorted paths and file
bytes. CI checks the compiler's dependency info for inputs outside the
crate. Review authorizes each digest refresh. A refreshed digest identifies
the bytes reviewed.

### Limits

- Within `credential-native`, changes to D-Bus dispatch still rely on
  review, the digest, and the lint. Rust's crate-level dependency visibility
  applies throughout that crate. The boundary controls this dependency and
  these capabilities. Other D-Bus implementations, raw IPC, and subprocesses
  remain outside that compiler guarantee.
- The guarantee applies to credential v2. The legacy `keyring` backend can
  raise a Linux Secret Service prompt until #209. After #209, extend the
  metadata check to the legacy Secret Service dependencies.
- macOS and Windows native security APIs remain direct dependencies of the
  app. Cargo dependency isolation applies to the Linux transport.
- D-Bus name ownership qualifies the daemon on the session bus. It does
  not authenticate the executable or establish its provenance.

Copy these limits into the new crate's pinned top-level documentation.

### Rollout

1. Land #573 and recheck the move map against the merged tree.
2. **Sealed change A** relocates the implementation while preserving its
   behavior and factory outcomes. It extracts the shared redaction leaf,
   moves the zbus dependency, establishes the app digest and compiler input
   check, and adds explicit package CI. Resolve the lease API and filesystem
   ownership questions in the move map before authorizing this change.
3. **Sealed change B** makes the gate a leaf, moves policy to sibling
   modules, and consolidates Linux dispatch into the typed send function.
   Add the lint, delete the source inventories and tokenizer fixtures this
   decision replaces, preserve behavioral tests, and refresh the digest.
4. Implement the Linux `KeyringBoundary` and worker marker with the outcome
   mapping and lease order in the move map. The factory still returns
   `Unsupported`.
5. Arm the factory in a separate, explicitly authorized gate change. Wire
   the qualified Linux ordinary boundary and the target-scoped recovery
   constructor. Keep the existing prompt capability and worker checks, and
   refresh the reviewed digest in that change. Desktop qualification and
   the other activation gates in #152 still apply.

Use the fallback if the relocation changes more than about fifteen app
files outside `credentials/`, if #573 slips for weeks, or if compiler
dependency info proves unreliable on Windows, macOS, or Linux. The move map
records the measured file count and the outstanding platform evidence.
