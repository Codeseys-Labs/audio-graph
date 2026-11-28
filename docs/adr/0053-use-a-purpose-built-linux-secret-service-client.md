---
status: accepted
date: 2026-11-28
deciders: [AudioGraph maintainers]
---

# ADR-0053: Use a Purpose-Built Linux Secret Service Client

## Context and Problem Statement

Credential v2 permits desktop prompts only during explicit recovery. The Linux
Secret Service facade used by v1 can unlock implicitly. The credential worker
in [ADR-0052](0052-credential-runtime-worker-and-v2-cutover.md) owns synchronous
store calls and requires separate outcomes for missing, locked, ambiguous,
unavailable, and failed reads.

## Decision Drivers

- Ordinary operations must avoid daemon activation, unlock, and prompt.
- The wire boundary must expose typed, redacted errors and encrypted sessions.
- Linux must qualify for v2 cutover before the legacy path can be retired.

## Considered Options

- **Keep Linux v2 unsupported indefinitely.** This avoids implementing a native
  boundary and leaves the legacy prompt-capable path reachable after cutover.
- **Maintain a pinned upstream fork.** This reuses the upstream protocol
  implementation but still requires changes to prompt dispatch, explicit
  message flags, constant-time DH, and key erasure. A private fork adds
  maintenance and advisory-tracking work.
- **Own the narrow client.** Direct D-Bus calls make the flag and prompt
  contract auditable. AudioGraph takes responsibility for the session
  composition and daemon interoperability.

## Decision Outcome

Use a purpose-built client on zbus 6 and RustCrypto. This supersedes the fork
plan in [#152](https://github.com/Codeseys-Labs/audio-graph/issues/152). The
client requests the Secret Service DH group 2, HKDF-SHA256, and AES-128-CBC
session exclusively. It checks the peer public key, preserves leading zeroes
in the protocol encodings, and zeroizes owned private exponent, shared value,
AES key, and decrypted credential buffers.

The zbus 6 async API runs inside `zbus::block_on` behind a synchronous facade.
The `default-rt` feature selects zruntime, the non-Tokio replacement for the
zbus 5 async-io backend. Call the facade on the dedicated credential worker
thread. A timed future covers connection setup through the final reply and
drops the connection when the deadline expires. Every service message uses
`NO_AUTO_START` and omits interactive authorization. Property caching is off,
and the bus owner is pinned for each transaction.

### Consequences

- The first slice exercises encrypted exact-match reads and the read-only wire
  calls on a private bus. The Linux production factory returns
  `Unsupported` until the qualification gate is met.
- The strongest objection is ownership of the session crypto composition.
  Published RFC 2409, RFC 5869, and NIST SP 800-38A vectors, a leading-zero
  session fixture, peer-key tests, and independent crypto review are required.
  Scripted tests establish protocol behavior and flag coverage. Interop
  requires execution against real daemons.
- Activation requires the remaining mutation and explicit-recovery slices,
  deterministic scripted tests, and the headless GNOME Keyring qualification
  job. Any daemon or version that surfaces UI before `Prompt` or cannot
  settle a pending mutation after disconnect stays unsupported. Real desktop
  dialogs, X11 and Wayland parenting, PAM unlock and relock, KWallet, KeePassXC,
  and packaged builds are tracked in [#153](https://github.com/Codeseys-Labs/audio-graph/issues/153)
  and [#210](https://github.com/Codeseys-Labs/audio-graph/issues/210).
- Linux carries zbus 5 through v1 and zbus 6 through v2 until the Linux
  cutover in [#209](https://github.com/Codeseys-Labs/audio-graph/issues/209).
