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
session exclusively. It checks the peer public key and encodes the outgoing
public key and HKDF input at the full group width. Zeroization of owned
private-exponent, shared-secret, AES-key, and decrypted-credential buffers is
best-effort. HKDF 0.12 retains its PRK and HMAC state, crypto-bigint's
exponentiation leaves window and accumulator temporaries, and
`U256::from_be_slice` can leave a moved temporary.

The zbus 6 async API runs inside `zbus::block_on` behind a synchronous facade.
The `default-rt` feature selects zruntime, the non-Tokio replacement for the
zbus 5 async-io backend. Call the facade on the dedicated credential worker
thread. A timed future on the connection's runtime covers connection setup
through the final reply and drops the connection when the deadline expires.
Every message sent to the Secret Service uses `NO_AUTO_START` and omits
interactive authorization. Property caching is off, and the bus owner is
pinned for each transaction. zbus's `Hello` goes to the bus driver without
flags. The driver cannot be activated, and the scripted peer observes only
messages delivered to the fake Secret Service.

## Noninteractive mutations

Search both halves for an exact attribute match before dispatch, on the same
connection and pinned service owner. A locked, duplicate, or indeterminate
lookup prevents a mutation. For writes, resolve the default collection and
check that it is unlocked. Create with `CreateItem(replace=false)` only after
definite absence. Replace the verified item in that collection with
`Item.SetSecret`. Both GNOME Keyring and KWallet can create an item when
`CreateItem(replace=true)` finds no replacement. `SetSecret` addresses the
verified item path directly. Delete also addresses the verified item path.
The protocol exposes no compare-and-swap across independent desktop clients.
[Secret Service API](https://specifications.freedesktop.org/secret-service/latest-single/)
· [GNOME Keyring](https://github.com/GNOME/gnome-keyring/blob/8561976b6b4662330caeb16c921472a89e891ed1/daemon/dbus/gkd-secret-objects.c)
· [KWallet](https://github.com/KDE/kwallet/blob/v6.25.0/src/runtime/ksecretd/kwalletfreedesktopcollection.cpp)

Every secret encryption draws a fresh 16-byte IV from the OS CSPRNG used for
the private exponent. The production encryption API generates the IV and
sends it with the ciphertext. The caller supplies an owned zeroizing
plaintext buffer. The prior value and encrypted read-back plaintext also use
owned zeroizing buffers.

One ten-second wall-clock deadline covers a mutation and its read-back.
Preparation, dispatch, and any prompt settlement have the first five seconds.
Explicit session cleanup and read-back fit in the remaining time, with up to
125 milliseconds reserved at each connection's cutoff for `Session.Close`.
Reads keep their five-second total deadline and reserve the same cleanup
interval. Close a session once when its path is known and time remains.
Cleanup failure preserves the transaction outcome and the connection is retired.

A prompt returned by a mutation is subscribed to before `Dismiss`. A
dismissed `Completed` signal plus an exact read-back of the prior state yields
`InteractionRequired`. An unsettled prompt or a mismatched read-back latches
`CommitUnknown`. The noninteractive path sends neither `Prompt.Prompt` nor
`Unlock`.

A deadline, disconnect, or error after dispatch triggers a fresh encrypted
read-back against the same bus and service owner within the remaining budget.
A write is confirmed by the exact intended bytes at the intended item, and a
delete by definite absence across both search halves. After a lost reply,
the desired state must differ from the prior state to confirm the mutation.
A terminal rejection plus corroborating read-back is required to report
`NotCommitted`. An unchanged value after a lost reply cannot exclude a late
mutation. Any unverified outcome latches `CommitUnknown` on the native
interaction gate. The client sends the mutation once.

### Consequences

- The first slice exercises encrypted exact-match reads and the read-only wire
  calls on a private bus. The Linux production factory returns
  `Unsupported` until the qualification gate is met.
- The strongest objection is ownership of the session crypto composition.
  Published RFC 2409, RFC 5869, and NIST SP 800-38A vectors, fixtures for
  leading zeroes in the public key and shared secret, peer-key tests, and
  independent crypto review are required. Scripted tests establish protocol
  behavior and service-bound flag coverage. Interop requires execution
  against real daemons.
- Activation requires the remaining mutation and explicit-recovery slices,
  deterministic scripted tests, and the headless GNOME Keyring qualification
  job. Any daemon or version that surfaces UI before `Prompt` or cannot
  settle a pending mutation after disconnect stays unsupported. Real desktop
  dialogs, X11 and Wayland parenting, PAM unlock and relock, KWallet, KeePassXC,
  and packaged builds are tracked in [#153](https://github.com/Codeseys-Labs/audio-graph/issues/153)
  and [#210](https://github.com/Codeseys-Labs/audio-graph/issues/210).
- KeePassXC is unsupported under the no-prompt rule. With its default
  `FdoSecrets/UnlockBeforeSearch` setting, `SearchItems` can open an unlock
  dialog before `Prompt` when no database is unlocked. Locked databases
  contribute no search results, and item attributes include defaults that
  prevent equality with AudioGraph's exact credential attributes.
  [KeePassXC settings](https://github.com/keepassxreboot/keepassxc/blob/cd42236540c1cfca332c0f051d981acfdacf7b4e/src/core/Config.cpp)
  · [Search](https://github.com/keepassxreboot/keepassxc/blob/cd42236540c1cfca332c0f051d981acfdacf7b4e/src/fdosecrets/objects/Service.cpp)
  · [Item attributes](https://github.com/keepassxreboot/keepassxc/blob/cd42236540c1cfca332c0f051d981acfdacf7b4e/src/fdosecrets/objects/Item.cpp)
- Before its March 2026 fix, KWallet's `ksecretd` could send 129-byte
  sign-extended public keys and derive keys from unpadded shared secrets.
  Factory activation for KWallet requires KWallet 6.25.0 or newer, or a
  verified backport of the fix, along with daemon qualification.
  [KWallet fix](https://github.com/KDE/kwallet/commit/307db62637ac2ca78ff670d746433be9381a923b)
- Linux carries zbus 5 through v1 and zbus 6 through v2 until the Linux
  cutover in [#209](https://github.com/Codeseys-Labs/audio-graph/issues/209).
