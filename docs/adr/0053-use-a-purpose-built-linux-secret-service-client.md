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
thread. The synchronous entry points return `Unsupported` before running
D-Bus work in an entered Tokio context or a zruntime shared or local task.
The Linux recovery boundary checks this inside `recover` after consuming its
one-use target and before resolving the session bus. The negative checks have
no signal for the main thread or work running under a zbus 5 or `futures`
executor. A positive worker marker remains a coordinated runtime change.

The Linux CI job checks the resolved packages in locked, Linux-filtered
Cargo metadata for default and cloud-only builds, and each baseline with
`diarization`, `sherpa-streaming`, and `surrealdb-embedded` enabled together.
`tokio` and `tracing` are forbidden on zbus 6, and `tracing` is forbidden on
zruntime itself. The legacy zbus 5 package is identified separately.
A timed future on the connection's runtime covers connection setup
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

A `ForbidPrompt` lookup takes the application's typed `EntryLocator` and uses
the exact `xdg:schema`, `service`, and `account` attributes derived from it.
The schema is `com.codeseys.audiograph.credentials.v2`. The `service` value
keeps the existing credential service namespace. Declaring `xdg:schema`
explicitly keeps GNOME Keyring's persisted record attributes stable across
reloads. The previous schema value was used by the dormant client and test
fixtures. The unsupported Linux factory has produced no v2 entries that
would require migration. An exact item always
takes precedence. Only definite absence across both search halves permits
the locked default collection to become the target. Recovery revalidates
the target against the same bus and unique service owner before sending
`Unlock` for that one path. Both GNOME Keyring and KWallet unlock the item's
containing collection even when the request names only the item.

The interactive policy is a private, Linux-only child of the sealed native
recovery module. The process gate supplies `AllowPrompt` under its lease and
refuses recovery after a stall or uncertain commit. Only the child can open
the interactive connection, using a constructor that requires `AllowPrompt`.
The opaque target revalidates a supplied connection. Its descriptive address
stays private, and both connection paths use a shared Unix-transport check.
The ordinary client hides its raw sender and uses a closed D-Bus method
enum. A source inventory tokenizes Rust files throughout the adapters tree
and checks the interactive method literals and the child's use of the sealed
gate API. The child subscribes to `Completed`
from the exact prompt path and pinned unique sender before `Prompt.Prompt`.
It passes the caller's window identifier or an empty string.

The caller supplies a single interactive budget for connection setup, target
revalidation, `Unlock`, signal subscription, and prompt settlement. The
deadline is set in the Linux `recovery_facade` constructor before it builds
the sealed facade. Native lease
acquisition uses the existing serialization on the dedicated credential
worker. All D-Bus waits use executor timers. The caller's atomic cancellation
flag is checked every 25 milliseconds. A queued approval advances to
verification and a queued dismissal ends the interaction without another
`Dismiss`. After cancellation, expiry, or a panic with a known prompt, the
prompt guard keeps the connection alive for one bounded `Dismiss` attempt
when needed. On the ordinary cancellation and expiry paths, approval during
`Dismiss` advances to verification and a dismissed completion acknowledges
the request. Dismissal has up to
500 milliseconds, including at most 250 milliseconds to resubscribe if needed.
Retirement takes up to another 125 milliseconds. These grace periods can
extend past the caller's deadline by up to 625 milliseconds. If approval is
observed, separate verification adds up to five seconds. The total D-Bus
budget is the caller's budget plus up to 5,625 milliseconds, excluding
native-lease acquisition and scheduler delay. A dismissed completion
is a service acknowledgement. An unacknowledged dismissal logs that the
dialog may still be open and preserves the closed cancel or timeout result.
The client cannot determine whether a screen dialog closed. A panic with
an unsettled operation uses the sealed `Uncertain` path.

A completed recovery is verified with a separate five-second
`ForbidPrompt` exact read on a fresh connection. A still-locked or failed read
prevents success. Recovery returns a closed status and drops the zeroizing
read-back buffer.

GNOME Keyring's `Unlock` builds a caller-bound prompt and its `Prompt` handler
starts the keyring's own unlock flow. KWallet creates a caller-bound prompt
for locked wallets and starts its own `openAsync` on `Prompt`. These handlers
have no D-Bus interactive-authorization check. Both calls use `NO_AUTO_START`
without `ALLOW_INTERACTIVE_AUTHORIZATION`. KWallet 6.25.0's `Dismiss` emits
`Completed` and unregisters the prompt object while leaving its backend
`openAsync` request active. A later answer can still unlock the wallet.
Interactive recovery stays unsupported for KWallet pending desktop tests.
The client asks the bus driver for `GetNameOwner(org.gnome.keyring)` with
`NO_AUTO_START` and requires it to match the already pinned unique owner of
`org.freedesktop.secrets`. A missing or differently owned name returns
`Unsupported` before an interactive call. This replaces `/proc` inspection
and works independently of executable paths, deleted binaries after an
upgrade, and `/proc` access. The D-Bus destination remains the pinned owner
throughout the operation.
[GNOME Keyring Unlock](https://github.com/GNOME/gnome-keyring/blob/8561976b6b4662330caeb16c921472a89e891ed1/daemon/dbus/gkd-secret-service.c#L510-L542)
· [GNOME Keyring prompt](https://github.com/GNOME/gnome-keyring/blob/8561976b6b4662330caeb16c921472a89e891ed1/daemon/dbus/gkd-secret-unlock.c#L288-L329)
· [KWallet Unlock](https://github.com/KDE/kwallet/blob/v6.25.0/src/runtime/ksecretd/kwalletfreedesktopservice.cpp#L346-L380)
· [KWallet prompt](https://github.com/KDE/kwallet/blob/v6.25.0/src/runtime/ksecretd/kwalletfreedesktopprompt.cpp#L34-L72)

## Headless qualification

Set the CI `qualify_linux_secret_service` workflow dispatch input to run the
Linux GNOME Keyring step. The step installs the daemon, checks the ignored
test list for the exact path, and runs `gnome_keyring_headless` with
`AUDIOGRAPH_QUALIFY_SECRET_SERVICE=1`. The test starts a private
`dbus-run-session` with a synthetic activation canary, a separate home and XDG
directories, and no graphical display. It first verifies that a read against
the unowned service leaves the canary dormant. It starts the real daemon
explicitly and runs the real same-owner lookup before testing encrypted
create, replace, read, and delete. It locks the isolated collection and checks
that ordinary read and write report `Locked`. Missing executables, failed
startup, and incomplete qualification fail the opted-in run.

This qualifies the tested GNOME Keyring version's headless storage protocol,
locking, and activation behavior. Real prompt UI, human approval and
cancellation, window parenting, PAM, KWallet, and KeePassXC still need desktop
qualification. The Linux factory remains `Unsupported` pending activation.

## Accepted factory integration

Keep the sealed `RecoveryBoundary::recover` and `verify` signatures and the
closed `RecoveryTarget::CredentialStore` value. On the worker, the service
coordinator would re-derive the `EntryLocator` from its own authority or
credential operation that observed `Locked`. A short ordinary process-gate
lease would capture a fresh `LockedTarget` for that locator. Release the
ordinary lease before entering the explicit recovery facade. A changed or
ambiguous lookup stays a closed failure. The target, optional window ID,
and caller deadline would be constructor inputs to a private, one-use Linux
boundary. This keeps locator data out of the sealed trait and out of IPC.

The dormant boundary tracks pending, verification pending, and consumed
states under a poison-safe mutex. A poisoned state fails with the same closed
conflict as a consumed boundary. `recover` takes the pending target before
the worker-context guard and D-Bus dispatch.
Only a completed or already-unlocked target moves to verification pending.
`verify` takes that state and runs the separate `ForbidPrompt` read. On a
terminal error, the facade can still call `verify`, which returns
`RecoveryRequired` without I/O so the original closed error is preserved.
A second `recover` call on the same healthy boundary returns
`Closed(RevisionConflict)` before I/O. A later user action would obtain a
new target and a new boundary. Existing process-gate latches take precedence.

| Linux outcome | Existing sealed result |
| --- | --- |
| Verified after a successful interactive phase | `recover` returns `Ok(())`. `verify` returns `Ok(Ready)`. |
| Target still locked after verification | `verify` returns `Ok(RecoveryRequired)`. The #521 service coordinator associates this with the selected set and keeps the runtime open. |
| User dismissal before cleanup, or caller cancellation with or without acknowledgement | `recover` returns `Err(Closed(Cancelled))`. Missing acknowledgement logs that the dialog may still be open. |
| Deadline with or without acknowledgement | `recover` returns `Err(Closed(Unavailable))`. The domain has no timeout code. Missing acknowledgement logs that the dialog may still be open. |
| Panic | `recover` returns `Err(Uncertain)` after bounded cleanup when a prompt is known. The sealed facade latches the process gate and returns `StalledWorker`. |
| Unavailable, unsupported, or another closed provider failure | The observing phase returns the corresponding `Err(Closed(code))`. Latch-shaped provider failures are reported as `Unavailable`. Only a panic or a pre-existing process-gate latch can return `StalledWorker`. |

Coordinate the positive worker marker with #521. At the start of
`runtime_worker_loop` in `credentials/runtime.rs`, before constructing the
worker, add this one line once `native_keyring` exports the Linux setter for
the adapter's thread-local marker and the adapter requires that marker.

```rust
#[cfg(target_os = "linux")] crate::credentials::adapters::native_keyring::mark_linux_credential_worker();
```

On the reviewed `9d8d7a31` base, the later sealed-file change is confined to
three sites.

1. In `native_interaction.rs` lines 442–460, update the platform factory
   comment and add the Linux `production_keyring_boundary` arm. Exclude
   Linux from the existing ordinary unsupported arm. Bind the Linux
   adapter's closed mutation outcomes before enabling this arm.
2. At lines 477–483, add a Linux-only `production_linux_recovery_facade`
   constructor beside the current zero-argument factory. It would give
   the Linux child's `recovery_facade` the opaque preparation inputs. The
   child already binds the boundary to `process_native_interaction_gate()`.
   The zero-argument
   Windows and macOS path remains the platform-default constructor.
3. Beside line 595, add a Linux-only re-export of that constructor scoped
   to the adapters. Refresh the reviewed digest in `adapters/mod.rs` in the
   same separately reviewed activation change. The capability definitions,
   minting, recovery trait, and facade's acquisition path retain their
   existing signatures and visibility.

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
