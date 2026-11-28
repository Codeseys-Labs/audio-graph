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
pinned for each transaction. zbus sends `Hello` and the `AddMatch` and
`RemoveMatch` used for `Completed` subscriptions to the bus driver without
`NO_AUTO_START`. The driver cannot be activated. The scripted service ledger
observes the messages delivered to the fake Secret Service.

## Noninteractive mutations

Search both halves for an exact attribute match before dispatch, on the same
connection and pinned service owner. A locked, duplicate, or indeterminate
lookup prevents a mutation. For writes, resolve the default collection and
check that it is unlocked. Create with `CreateItem(replace=false)` only after
definite absence. Replace the verified item in that collection with
`Item.SetSecret`. With `CreateItem(replace=true)`, GNOME Keyring and KWallet
can choose the first item matching the requested subset of attributes. GNOME
Keyring also rewrites the selected item's label and attributes, so replacement
could overwrite another application's item. `SetSecret` addresses the exact
verified item directly. Every created item uses the label `AudioGraph
credential`. Delete uses the exact item path and needs no secret or session.
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

A returned mutation prompt is subscribed to before `Dismiss`. A completed
prompt and an exact read-back of the prior state yield `InteractionRequired`.
`Completed(dismissed=false)` also permits a commit when read-back confirms the
intended state. An unsettled prompt or inconclusive read-back latches
`CommitUnknown`. This path calls `Dismiss` as its sole prompt method.

Every dispatch triggers a fresh read-back against the same bus and service
owner within the remaining budget, including on acknowledgement, deadline,
disconnect, or error. A write is confirmed by the exact intended bytes at
the intended item over an encrypted session. A delete is confirmed by
definite absence across both search halves. After a lost reply,
the desired state must differ from the prior state to confirm the mutation.
A terminal rejection plus corroborating read-back is required to report
`NotCommitted`. An unchanged value after a lost reply cannot exclude a late
mutation. Any unverified outcome latches `CommitUnknown` on the native
interaction gate. The client sends the mutation once.

## Explicit recovery

A `ForbidPrompt` lookup captures opaque evidence of one exact locked item or
the locked default collection. Recovery revalidates the target against the
same bus and unique service owner before sending `Unlock` for that one path.
The interactive policy is a private, Linux-only child of the existing sealed
native recovery module. The gate supplies `AllowPrompt` under its lease.
Only the child sends `Unlock` or `Prompt.Prompt`. It subscribes to the pinned
prompt's `Completed` signal before `Prompt.Prompt` and passes the caller's
window identifier or an empty string.

The caller supplies a single interactive budget for connection setup, target
revalidation, `Unlock`, signal subscription, and prompt settlement. The
deadline is measured from facade entry. Native lease acquisition follows the
existing gate on the sole credential worker. All D-Bus waits use executor
timers. The caller's atomic cancellation flag is checked every 25 milliseconds
while waiting. After cancellation or expiration, a known prompt gets one
`Dismiss` attempt and up to 500 milliseconds for completion, including at
most 250 milliseconds to restore the subscription if needed. The closed
outcome distinguishes confirmed and unconfirmed dismissal. Retirement has
up to 125 milliseconds to close the interactive connection, which is then
dropped. A completed recovery is verified with a separate five-second
`ForbidPrompt` exact read on a fresh connection. A still-locked or failed read
prevents success. Recovery returns a closed status and drops the zeroizing
read-back buffer.

GNOME Keyring's `Unlock` builds a caller-bound prompt and its `Prompt` handler
starts the keyring's own unlock flow. KWallet creates a caller-bound prompt
for locked wallets and starts its own `openAsync` on `Prompt`. These handlers
have no D-Bus interactive-authorization check. Both calls use `NO_AUTO_START`
without `ALLOW_INTERACTIVE_AUTHORIZATION`. KWallet 6.25.0's `Dismiss` emits
`Completed` and unregisters the prompt object. Whether its backend dialog
closes needs real desktop qualification, since that handler does not cancel
the outstanding `openAsync` transaction.
[GNOME Keyring Unlock](https://github.com/GNOME/gnome-keyring/blob/8561976b6b4662330caeb16c921472a89e891ed1/daemon/dbus/gkd-secret-service.c#L510-L542)
· [GNOME Keyring prompt](https://github.com/GNOME/gnome-keyring/blob/8561976b6b4662330caeb16c921472a89e891ed1/daemon/dbus/gkd-secret-unlock.c#L288-L329)
· [KWallet Unlock](https://github.com/KDE/kwallet/blob/v6.25.0/src/runtime/ksecretd/kwalletfreedesktopservice.cpp#L346-L380)
· [KWallet prompt](https://github.com/KDE/kwallet/blob/v6.25.0/src/runtime/ksecretd/kwalletfreedesktopprompt.cpp#L34-L72)

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
- Activation requires deterministic scripted tests and the headless GNOME Keyring qualification
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
