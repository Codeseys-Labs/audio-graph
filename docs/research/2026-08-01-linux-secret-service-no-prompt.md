# Linux Secret Service no-prompt feasibility

Date: 2026-08-01

Owner: `audio-graph-3098`

Status: decision-ready research; implementation and packaged-provider proof
remain gated work

## Question and gated decision

Can AudioGraph implement exact-match Secret Service read, create/replace,
unlock, prompt refusal, prompt dismissal, and staging delete so that background
`CredentialInteraction::ForbidPrompt` operations never request an unlock or
execute a prompt, while explicit `AllowPrompt` operations retain prompt
ownership and report cancellation?

This gates whether the Linux credential adapter should use direct `zbus`, a
narrow wrapper/fork around the pinned Secret Service stack, or remain
unsupported.

## Recommendation

**Use a narrow, revision-pinned fork of `secret-service` 5.1.0 that exposes
deferred prompt results. Do not use the stock
`zbus-secret-service-keyring-store` 1.0.0 for Linux v2.**

The protocol is capable of the required split: search returns locked and
unlocked objects separately, and operations return a prompt object that the
client must explicitly execute. The current high-level crates erase that split
at exactly the wrong boundary. The keyring store automatically unlocks locked
search matches; `secret-service` automatically calls `Prompt` from unlock,
create, and delete helpers. A narrow fork can retain the upstream encrypted
session and secret encoding while returning either a completed result or an
owned, unexecuted prompt handle to AudioGraph.

Confidence is **high (0.90)** for protocol and Rust API feasibility, based on
the specification, current crate source, and a compile-only `zbus` proxy probe.
Confidence is **medium-high (0.80)** for production compatibility: the pinned
GNOME Keyring and KWallet sources both separate prompt construction/dismissal
from the method that starts interactive UI. The harnesses below must still
prove that behavior in packaged desktop sessions and that cancellation settles
under real bus and window-system races.

| Option | Decision | Boundary |
| --- | --- | --- |
| Narrow `secret-service` fork | **Select** | Expose prompt-bearing replies without executing them; retain upstream DH session and crypto code. |
| Direct `zbus` in AudioGraph | Reject for the first implementation | The D-Bus proxy shape compiles, but this would duplicate private session negotiation, AES-CBC framing, and prompt decoding from `secret-service`. |
| Stock `zbus-secret-service-keyring-store` | Reject | Locked matches are automatically unlocked and its error mapping collapses locked, missing-result, and dismissed-prompt states. |
| Linux unsupported | Conditional fallback | Keep production Linux v2 disabled if either real-provider release harness fails a stop condition. |

This is a conditional implementation decision, not a claim that the present
dependency already satisfies the contract. Packaged GNOME and KWallet evidence
is a release gate.

## Non-negotiable interaction contract

`ForbidPrompt` must be enforced as a call-level capability, not as a timeout or
as an instruction passed to a high-level library:

- Never call `org.freedesktop.Secret.Service.Unlock`.
- Never call `org.freedesktop.Secret.Prompt.Prompt`.
- Never set D-Bus `ALLOW_INTERACTIVE_AUTHORIZATION`.
- Set `NO_AUTO_START` on every `ForbidPrompt` D-Bus method call, including
  `OpenSession` and `org.freedesktop.DBus.Properties.Get`. An absent service is
  `unavailable`; a passive status/read must not activate one. This requires a
  no-autostart proxy or `call_with_flags`, not the stock generated calls.
- Treat a result in the locked half of `SearchItems` as `locked`; do not turn it
  into `missing` and do not retry through an interactive path.
- A prompt path returned by create or delete may be dismissed or invalidated,
  but never executed. It is `interaction_required`, not success.
- Never fall back to YAML or another backend after locked, denied, unavailable,
  cancelled, ambiguous, prompt-required, or backend-failure results.

`AllowPrompt` is valid only for an explicit user action. One owner task retains
the connection, prompt object, completion subscription, and cancellation state
until the prompt settles or the connection is retired.

## Exact identity and search

Use only the immutable attributes reserved by
[ADR-0035](../adr/0035-backend-owned-credential-service.md):

```text
service  = com.codeseys.audiograph.credentials
username = v2/<credential-set-id>
         | v2-staging/<operation-id>/<credential-set-id>
         | v2/_authority
```

Secret Service attribute names and values are case-sensitive. `SearchItems`
matches items containing all supplied pairs; an item may contain additional
provider attributes. Therefore, "exact" means exact equality of both reserved
pairs, not whole-map equality.

For every returned object path:

1. Treat the exact `SearchItems` query as the identity lookup promised by the
   protocol. For an unlocked result, also read
   `org.freedesktop.Secret.Item.Attributes` and require both reserved pairs to
   remain equal; provider-added keys are permitted. A mismatched returned item
   is a provider-contract failure, not evidence that the credential is missing.
2. Count distinct object paths across both `unlocked` and `locked` results. The
   same path in both lists is an inconsistent provider reply, not two items.
3. Return `missing` only for zero verified matches while the service is known
   available; return the sole match for one; return `ambiguous_match` for more
   than one. Never select the first item or delete extras.
4. Preserve the locked/unlocked classification. Do not require a property read
   that a provider refuses while locked. If an attribute, state, or secret read
   races to `org.freedesktop.Secret.Error.IsLocked`, return `locked` under
   `ForbidPrompt`.

The label is non-secret display metadata only. It is not an identity key.

## Required protocol and Rust API shape

The adapter needs these exact D-Bus surfaces. `/` is the null object path used
by the specification when no item or prompt is returned.

| Interface | Member | Wire result used by the adapter |
| --- | --- | --- |
| `org.freedesktop.Secret.Service` | `OpenSession(s, v)` | `(v, o)`; use `dh-ietf1024-sha256-aes128-cbc-pkcs7` |
| `org.freedesktop.Secret.Service` | `SearchItems(a{ss})` | `(ao unlocked, ao locked)` |
| `org.freedesktop.Secret.Service` | `Unlock(ao)` | `(ao unlocked_now, o prompt)` |
| `org.freedesktop.Secret.Service` | `ReadAlias(s)` | `o`; `/` means no default collection |
| `org.freedesktop.Secret.Collection` | `CreateItem(a{sv}, (oayays), b replace)` | `(o item, o prompt)` |
| `org.freedesktop.Secret.Collection` | `Locked` property | `b` |
| `org.freedesktop.Secret.Item` | `GetSecret(o session)` | `(oayays)` |
| `org.freedesktop.Secret.Item` | `Delete()` | `o prompt` |
| `org.freedesktop.Secret.Item` | `Locked`, `Attributes` properties | `b`, `a{ss}` |
| `org.freedesktop.Secret.Prompt` | `Prompt(s window_id)`, `Dismiss()` | completion is the `Completed(b dismissed, v result)` signal |

The fork should add policy-neutral deferred methods while leaving existing
upstream methods intact for compatibility. One sufficient public shape is:

```rust
pub enum Deferred<T> {
    Complete(T),
    PromptRequired(PendingPrompt<T>),
}

pub struct PendingPrompt<T> {
    // Owns the same zbus connection, object path, subscribed signal stream,
    // and operation-specific result decoder. Fields remain private.
}

impl<T> PendingPrompt<T> {
    pub async fn execute(self, window_id: &str, cancel: CancelToken)
        -> Result<T, Error>;
    pub async fn dismiss(self) -> Result<PromptDismissal, Error>;
}

pub async fn unlock_deferred(&self, objects: &[OwnedObjectPath])
    -> Result<Deferred<Vec<OwnedObjectPath>>, Error>;
pub async fn create_item_deferred(/* existing arguments */)
    -> Result<Deferred<OwnedObjectPath>, Error>;
pub async fn delete_deferred(&self) -> Result<Deferred<()>, Error>;
```

The actual fork may use different names, but the following properties are
mandatory:

- constructing `Deferred::PromptRequired` does not call `Prompt`;
- the prompt handle owns the connection and subscribes to `Completed` before
  any `Prompt` or `Dismiss` call;
- a prompt result remains operation-typed instead of exposing an unvalidated
  arbitrary `zvariant::Value` to application code;
- stream closure is an error, not `unwrap()`;
- the AudioGraph adapter, not the fork, selects `ForbidPrompt` or
  `AllowPrompt`;
- the fork exposes a no-autostart connection/call path so opening the encrypted
  session cannot silently bus-activate the service; and
- encrypted-session code remains shared with upstream. Do not choose the
  `plain` session merely to make direct `zbus` shorter.

### Compile-only proxy probe

A throwaway crate in `/tmp`, using the locally cached exact `zbus` 5.18.0
source with only its `async-io` feature, compiled these typed proxy/result
shapes with `cargo check --offline`. It also compiled:

- `Proxy::call_with_flags(..., MethodFlags::NoAutoStart, ...)` for
  `SearchItems`;
- `receive_completed().await` before `Prompt`; and
- a `Send` assertion for the generated `CompletedStream`.

This proves that the required proxy and owner-task shape is expressible on the
pinned `zbus`; it does not prove provider runtime behavior.

The API is shown as asynchronous because prompt completion and application
cancellation must be selected concurrently. AudioGraph should drive it from a
small `async-io` executor owned by the existing serialized credential worker
thread; it must not move provider sockets or prompt ownership onto Tauri's UI
executor. A blocking convenience method that internally waits for `Completed`
would recreate the cancellation problem this fork is intended to solve.

## Operation algorithms

### Read under `ForbidPrompt`

1. Open the encrypted session and search the two exact attributes with
   `NO_AUTO_START`.
2. Verify attributes and cardinality across both result lists.
3. Zero matches is `missing`; any verified locked match is `locked`; duplicates
   are `ambiguous_match`.
4. For the sole unlocked match, call `GetSecret`. An `IsLocked` race is
   `locked`; it must not trigger `Unlock`.
5. Decode the protected envelope. Raw bytes and native error prose never enter
   logs or IPC.

An explicit `AllowPrompt` read may perform exactly one owned unlock cycle and
then repeat the exact search. It must not blindly reuse an object path returned
before the prompt.

### Create or replace

1. Preflight exact search and reject ambiguity.
2. Resolve alias `default`; `/` is `unavailable`, not permission to create a
   different collection. Read the collection's `Locked` property.
3. Under `ForbidPrompt`, a locked collection returns `locked`. Under
   `AllowPrompt`, unlock through the owned lifecycle below.
4. Call `CreateItem` with the exact attributes, protected UTF-8 JSON envelope,
   and `replace=true`.
5. A non-root item with root prompt is only a candidate success. Re-search and
   read back the exact revision and operation id before publishing success.
6. A root item with non-root prompt enters prompt refusal or ownership. A lost
   reply, timeout, disconnect, unexpected path pair, or unverifiable readback
   is `commit_unknown`; never retry the mutation blindly.

The specification defines replacement in terms of identical attributes. The
preflight/postflight cardinality checks are still required because external
tools can create duplicate entries outside AudioGraph's lock.

### Staging delete

1. Search the exact generated staging locator. Zero matches is idempotent
   `already_absent`; more than one is `ambiguous_match`.
2. A locked match under `ForbidPrompt` is `locked`. An explicit recovery action
   may unlock it through the owned prompt lifecycle.
3. Call `Delete` only for the sole verified item. `/` means no prompt was
   required; a non-root path enters prompt refusal or ownership.
4. Report deletion only after an exact re-search returns zero. A lost reply or
   unresolved prompt is `commit_unknown`; retain the non-secret pending intent
   for recovery.

This algorithm applies to physical cleanup of staging entries. ADR-0035's
logical active-record delete remains replacement with a tombstone.

## Prompt refusal and owned prompt lifecycle

### `ForbidPrompt`

When a mutation unexpectedly returns a non-root prompt path:

1. Create the prompt proxy on the same connection and subscribe to `Completed`.
2. Do **not** call `Prompt`.
3. Call `Dismiss` and wait for `Completed(dismissed=true)` under a short bounded
   deadline. Dismissal is cleanup; it does not grant permission to execute the
   prompt.
4. If dismissal is rejected, the stream closes, or completion does not arrive,
   retire the connection. The specification makes prompt objects
   connection-specific, so disconnection invalidates the prompt and session.
5. Reconnect and perform a read-only exact readback. Unchanged state is
   `interaction_required`; verified desired state may be accepted; any state
   that cannot be established is `commit_unknown`.

The real-provider harness must prove the assumption that `Dismiss` on a prompt
that was never executed does not display UI. If either provider violates it,
switch that provider's refusal path to immediate connection retirement. If UI
still appears before AudioGraph calls `Prompt`, that provider is unsupported.

### `AllowPrompt`

The safe ordering is:

```text
receive returned prompt path
  -> build proxy on the same connection
  -> subscribe to Completed
  -> call Prompt(window_id)
  -> wait for Completed or an application cancellation request

cancellation request
  -> call Dismiss on the owner task
  -> still drain Completed
  -> return cancelled only after dismissal/completion settles
```

Only the owner task may act on the prompt. Dropping the caller's future is not
remote cancellation. If `Completed` wins the race, decode it once. If
cancellation wins, issue `Dismiss`; an `UnknownObject` response is ignorable
only when a buffered `Completed` already proves that the prompt settled.
Otherwise retire the connection on deadline or stream closure.

Map `dismissed=true` to `cancelled`. For `dismissed=false`, validate the
operation-specific result:

- unlock: the returned object-path array contains the requested target, the
  target is now unlocked, and an exact search still identifies it;
- create: the result is a non-root item path, followed by exact search and
  revision readback; and
- delete: do not trust the variant payload; exact re-search must return zero.

Check `dismissed` before decoding the variant. KWallet 6.28 deliberately emits
a placeholder object-path list from `Dismiss`, including for create prompts, to
avoid a libsecret freeze; a dismissed result is not operation-shaped.

No new request may use a connection whose prompt outcome is unresolved.

## Typed result mapping

Map on typed D-Bus error names and verified state, never on display strings.

| AudioGraph result | Exact evidence required |
| --- | --- |
| `missing` | Exact `SearchItems` plus attribute verification yields zero while the service is live; or `NoSuchObject`/`UnknownObject` followed by that same zero-result search. |
| `locked` | The sole verified result is in the locked list, `Locked=true`, or the provider returns `org.freedesktop.Secret.Error.IsLocked`. |
| `interaction_required` | `ForbidPrompt` receives a non-root prompt path, or D-Bus returns `InteractiveAuthorizationRequired`, and readback shows no committed change. |
| `cancelled` | `Completed(dismissed=true)`, or application-requested cancellation after the owner task has settled the prompt. A timeout alone is not cancellation. |
| `access_denied` | Typed `AccessDenied` or `AuthFailed`. Do not turn it into `missing`. |
| `unavailable` | No session bus; `ServiceUnknown`, `NameHasNoOwner`, or `NoServer`; or `ReadAlias("default")` returns `/`. |
| `unsupported_store` | Required interfaces or DH algorithm are absent, or a provider violates the no-prompt contract in the release harness. |
| `ambiguous_match` | More than one verified item exists across unlocked and locked search results. |
| `commit_unknown` | A mutation request may have reached the provider, but timeout, disconnect, lost reply, unresolved prompt, or failed readback prevents proving the resulting revision/absence. |
| `backend_failure` | Unexpected signature/value/path combination, signal stream closure, crypto/decoding failure, or any unmapped typed provider failure. |

`org.freedesktop.Secret.Error.NoSession` permits one new encrypted session and
retry only before a mutation could have been accepted. After a mutation send,
reconcile by readback instead of retrying. The stock keyring adapter's
`NoStorageAccess`/`PlatformFailure` reduction is too coarse for this table.

## GNOME and KWallet harness

The cheapest decisive residual experiment is a two-layer harness: first a
scripted in-process Secret Service double for exhaustive call-order and race
coverage, then the same packaged adapter against pinned real providers. The
current host has a session-bus client but neither `gnome-keyring-daemon` nor
`kwalletd`, so no real-provider result is claimed here.

### Deterministic protocol harness

Run a private `dbus-daemon` and a scripted `zbus` service that can return every
legal and adversarial response. Assert from an in-memory, content-free call log:

- every `ForbidPrompt` scenario has zero `Unlock`, zero `Prompt`, zero
  `ALLOW_INTERACTIVE_AUTHORIZATION`, and `NO_AUTO_START` on service calls;
- locked/unlocked races and duplicate matches preserve typed outcomes;
- create/delete prompt replies are refused without execution;
- `Completed`-before-cancel, cancel-before-`Completed`, `UnknownObject`, closed
  stream, and deadline races settle once and poison/retire when unresolved;
- post-mutation reply loss produces readback or `commit_unknown`, never an
  automatic retry; and
- logs, receipts, and IPC contain no secret, raw provider error prose, private
  locator, exact value length, or fingerprint.

### GNOME Keyring 48.0

Pin official tag 48.0 at commit
[`adadbad2`](https://gitlab.gnome.org/GNOME/gnome-keyring/-/commit/adadbad2fdeb79a654dca37b31349e2a1d527ef0),
plus the package and image digest. In a Linux CI image, start a private session
bus, temporary `HOME`/XDG directories, and
`gnome-keyring-daemon --foreground --components=secrets`. Reuse the upstream
daemon test pattern and test prompter rather than a developer's desktop
keyring. The upstream `secret-service` crate CI starts only an already-unlocked
GNOME keyring, so it is a baseline, not no-prompt proof.

Run the full packaged Tauri binary through:

- save, exact read, replace, restart/read, staging delete, and idempotent delete;
- locked item and locked collection under `ForbidPrompt`, with zero prompter
  invocations and bounded return;
- explicit unlock followed by simulated user dismissal and acceptance;
- duplicate exact attributes, lock races, missing service, missing bus, and
  missing default alias; and
- returned create/delete prompt refusal plus readback.

### KWallet 6.28.0

Pin official tag v6.28.0 at commit
[`e4483b9a`](https://invent.kde.org/frameworks/kwallet/-/commit/e4483b9a7c9466e3508cf9c11ee9668a3686d030),
plus the package and VM image digest. Use a clean KDE/Plasma VM with Secret
Service enabled, one bus owner, and a password-backed default wallet. Run the
identical matrix and record only redacted method names/outcomes and prompt-
window counts. AudioGraph's JSON envelope is UTF-8, avoiding KWallet's
documented limitation for arbitrary binary keyring payloads in the versioned keyring-store
[`lib.rs`](https://docs.rs/crate/zbus-secret-service-keyring-store/1.0.0/source/src/lib.rs).

Add a separate adversarial GPG-wallet job with a hard deadline. KWallet's issue
tracker records long `OpenSession` delays for GPG wallets, plus historical
hang/freeze defects around an empty wallet name and dismissed first-wallet
creation. The selected 6.28.0 pin contains later fixes, but these cases must
remain regression tests rather than being inferred from version number alone.

### Required acceptance matrix

| Scenario | `ForbidPrompt` expected | `AllowPrompt` expected |
| --- | --- | --- |
| Unlocked unique item | read/replace/delete and verify | same, without prompt |
| Locked unique item | `locked`; no `Unlock`/`Prompt`/window | one owned prompt; accept or `cancelled` |
| Locked default collection | `locked`; no mutation | owned unlock, then mutation/readback |
| Create/delete returns prompt | refuse; dismiss/invalidate; readback | owned prompt; typed completion |
| User dismisses | not applicable because no prompt executes | `cancelled`, connection reusable only after settlement |
| Duplicate exact identity | `ambiguous_match`; no read/mutation | same; prompting cannot resolve identity ambiguity |
| Service/bus/default alias absent | `unavailable`; no fallback | same |
| Reply loss or provider stall | deadline; no retry; possibly `commit_unknown` | dismiss/retire; readback or `commit_unknown` |

Provider/version/image digest, application commit, feature set, call-order
assertions, and window counts are required release evidence. A raw D-Bus trace
must not be persisted because message bodies can contain secrets and private
locators; the harness should emit an allowlisted, redacted event stream.

## Version pins and acceptance boundary

The current repository manifest requests `keyring = "4.1.1"`, while the lockfile
resolves this Linux stack:

| Component | Current pin | Recommendation |
| --- | --- | --- |
| `keyring` | 4.1.6 | Stop using its automatic Linux store for v2; it may remain for untouched v1 code during migration. |
| `keyring-core` | 1.0.0 | Retain as the platform-neutral trait boundary if the Linux adapter can implement it without losing typed AudioGraph detail. |
| `zbus-secret-service-keyring-store` | 1.0.0 | Do not use for Linux v2 operations. |
| `secret-service` | 5.1.0 | Fork this source narrowly and pin an immutable Git revision. |
| `zbus` / `zvariant` | 5.18.0 / 5.13.1 | Keep exact while landing the adapter and provider harness. |
| Secret Service session feature | `secret-service/crypto-rust` plus `zbus/async-io` in the resolved graph | Request `rt-async-io-crypto-rust` explicitly on the fork; do not enable `tokio` globally merely for this adapter because Cargo feature unification would change the runtime path for the graph. |
| GNOME Keyring | 48.0 | Required real-provider job. |
| KWallet | KDE Frameworks 6.28.0 | Required password-wallet job; GPG wallet is a separate adversarial capability. |

The starting crates.io source identities in `Cargo.lock` are:

```text
keyring 4.1.6                                72585bb6cc9bc370d1d545b7e23fcce71dfd4461c5e15275e3cf51bdfd9a980a
keyring-core 1.0.0                           fb1e621458ca9c51aa110bd0339d4751a056b9576bf1253aee1aa560dda0fc9d
zbus-secret-service-keyring-store 1.0.0      4ccede190ba363386a24e8021c7f3848393976609ec9f5d1f8c6c09ef37075b4
secret-service 5.1.0                         9a62d7f86047af0077255a29494136b9aaaf697c76ff70b8e49cded4e2623c14
zbus 5.18.0                                  fe18fb60dc696039e738717b76eaea21e7a4489bbb1885020b43c94236d7e98a
zvariant 5.13.1                              bee2a0bcd2a907786a456fff45aaaaf54c9ba5f50b71ae9ec1a4edd200c94911
```

The implementation record must add the fork's immutable Git revision; a branch
name is not an acceptable pin.

After the harness passes, future crate or provider upgrades must rerun this
matrix. Generic "Secret Service compatible" is not enough evidence to add a
provider to the supported set.

## Evidence

Evidence labels mean: **verified** = directly inspected in the repository,
immutable crate source, or compiled locally; **documented** = stated by the
project decision or an upstream primary specification/document; **inferred** =
reasoned consequence that still needs the named runtime proof.

- **[documented]** The Secret Service specification returns locked and unlocked
  search results separately. Locked objects cannot expose secrets or be
  modified. `Unlock`, `CreateItem`, and `Delete` return prompt paths, and only a
  client call to `Prompt` displays the prompt. Prompt objects are specific to a
  connection; `Dismiss` completes them as dismissed. Sources:
  [lookup attributes](https://specifications.freedesktop.org/secret-service/latest/lookup-attributes.html),
  [unlocking](https://specifications.freedesktop.org/secret-service/latest/unlocking.html),
  [prompts](https://specifications.freedesktop.org/secret-service/latest/prompts.html),
  [Service](https://specifications.freedesktop.org/secret-service/latest/org.freedesktop.Secret.Service.html),
  [Collection](https://specifications.freedesktop.org/secret-service/latest/org.freedesktop.Secret.Collection.html),
  [Item](https://specifications.freedesktop.org/secret-service/latest/org.freedesktop.Secret.Item.html),
  and
  [Prompt](https://specifications.freedesktop.org/secret-service/latest/org.freedesktop.Secret.Prompt.html).

- **[verified]** `secret-service` 5.1.0's `lock_or_unlock` helper executes the
  returned prompt when no object was unlocked immediately; create and delete
  helpers also execute prompts. Its prompt helper subscribes before `Prompt`,
  but provides no caller-owned cancellation handle and unwraps a closed signal
  stream. Its proxy, session, and secret struct modules are private. Sources:
  versioned crate
  [`util.rs`](https://docs.rs/crate/secret-service/5.1.0/source/src/util.rs),
  [`collection.rs`](https://docs.rs/crate/secret-service/5.1.0/source/src/collection.rs),
  [`item.rs`](https://docs.rs/crate/secret-service/5.1.0/source/src/item.rs),
  and [`lib.rs`](https://docs.rs/crate/secret-service/5.1.0/source/src/lib.rs).

- **[verified]** `zbus-secret-service-keyring-store` 1.0.0 calls `unlock_all`
  whenever a search has locked matches, and its collection lookup unlocks a
  locked collection. It maps `Locked`, `NoResult`, and `Prompt` to one
  `NoStorageAccess` class. Sources: versioned
  [`service.rs`](https://docs.rs/crate/zbus-secret-service-keyring-store/1.0.0/source/src/service.rs)
  and
  [`errors.rs`](https://docs.rs/crate/zbus-secret-service-keyring-store/1.0.0/source/src/errors.rs).

- **[verified]** The repository's
  [`Cargo.toml`](../../src-tauri/Cargo.toml) and
  [`Cargo.lock`](../../src-tauri/Cargo.lock) contain the exact dependency pins
  listed above. `cargo tree --locked -e features` shows
  `secret-service/crypto-rust` and the resolved `zbus/async-io` runtime path.

- **[verified]** The compile-only probe established that `zbus` 5.18.0 can
  express all required typed proxies, a `NO_AUTO_START` call, subscribe-before-
  prompt ordering, and a sendable completion stream. The relevant upstream API
  is documented by the versioned
  [`proxy` macro](https://docs.rs/zbus/5.18.0/zbus/attr.proxy.html),
  [`MethodFlags`](https://docs.rs/zbus/5.18.0/zbus/proxy/enum.MethodFlags.html),
  and
  [`SignalStream`](https://docs.rs/zbus/5.18.0/zbus/proxy/struct.SignalStream.html).

- **[documented]** Secret Service defines only a small provider-error set, so
  unavailable/access/call-state distinctions must also use typed D-Bus errors
  and state-confirming searches. Sources:
  [Secret Service errors](https://specifications.freedesktop.org/secret-service/latest/errors.html)
  and versioned
  [`zbus::Error`](https://docs.rs/crate/zbus/5.18.0/source/src/error.rs).

- **[documented]** GNOME libsecret's high-level unlock API may prompt and uses a
  cancellable asynchronous operation. That is useful prior art for explicit
  cancellation, but not proof of a no-prompt call path. Sources:
  [GNOME libsecret Service](https://gnome.pages.gitlab.gnome.org/libsecret/class.Service.html)
  and
  [migration guide](https://gnome.pages.gitlab.gnome.org/libsecret/migrating-libgnome-keyring.html).

- **[documented]** GNOME Keyring's own daemon tests demonstrate a private
  foreground daemon, isolated control directory, fixture keyring, and test
  prompter. This is the appropriate basis for the hermetic GNOME harness.
  Source: upstream
  [`test-service.c`](https://gnome.pages.gitlab.gnome.org/gnome-keyring/coverage/daemon/dbus/test-service.c.gcov.html).

- **[verified]** In GNOME Keyring 48.0, `Prompt` starts asynchronous system-
  prompt initialization, while `Dismiss` emits `Completed(dismissed=true)` and
  disposes the prompt without entering that initialization path. Source:
  tagged
  [`gkd-secret-prompt.c`](https://gitlab.gnome.org/GNOME/gnome-keyring/-/blob/48.0/daemon/dbus/gkd-secret-prompt.c).

- **[verified]** In KWallet 6.28.0, `Prompt` calls the backend's asynchronous
  wallet-open operation, while `Dismiss` sends a targeted dismissed completion
  and unregisters the object without calling that operation. Its dismissed
  result is a placeholder object-path list, so callers must branch on
  `dismissed` before decoding. Source: tagged
  [`kwalletfreedesktopprompt.cpp`](https://invent.kde.org/frameworks/kwallet/-/blob/v6.28.0/src/runtime/ksecretd/kwalletfreedesktopprompt.cpp).

- **[documented]** KDE's tracker records provider-specific failure modes that a
  generic conformance test would miss: GPG-wallet `OpenSession` delay, an
  empty-wallet hang, and a first-wallet dismissal freeze. Sources: KDE bugs
  [458085](https://bugs.kde.org/show_bug.cgi?id=458085),
  [504656](https://bugs.kde.org/show_bug.cgi?id=504656), and
  [504678](https://bugs.kde.org/show_bug.cgi?id=504678).

- **[inferred]** A narrow fork is safer and smaller than raw direct `zbus`
  because `secret-service` already owns the DH exchange, AES session framing,
  and operation-result decoding, while the missing capability is only deferred
  prompt ownership. This recommendation changes if the fork cannot expose that
  boundary without a broad rewrite.

- **[inferred]** The specification's explicit `Prompt` call should make
  `ForbidPrompt` enforceable, but only the GNOME 48.0 and KWallet 6.28.0 window-
  count/call-order harness can establish that the selected implementations do
  not display UI early or mishandle `Dismiss` without `Prompt`.

Two additional general sources would not change this recommendation. The
remaining uncertainty is implementation behavior and is decidable only by the
targeted provider experiment above.

## Rejected alternatives

### Direct `zbus` as the initial production adapter

The typed proxy layer is small and compiles, but a complete adapter would also
need to duplicate or extract `secret-service`'s private session key exchange,
key derivation, encryption/decryption, padding, and result conversion. That
increases security-review and maintenance surface without improving prompt
control over a narrow fork. Reconsider only if the fork delta cannot remain
small or upstream refuses a reusable deferred-prompt API.

### Stock `zbus-secret-service-keyring-store`

Categorically incompatible with `ForbidPrompt`: search can call `Unlock`, and
the lower `secret-service` helper can then call `Prompt`. The store also erases
the typed distinctions required by ADR-0035.

### GNOME libsecret wrapper

Its high-level unlock operations are designed to prompt and cancel, not to make
an auditable Rust-level guarantee that background operations never enter an
interactive path. Adding an FFI layer also does not solve KWallet-specific
behavior or exact AudioGraph error mapping.

### Plain Secret Service sessions

Rejected even where a provider supports them. Sending the protected envelope
unencrypted over the session bus is an unnecessary regression when the pinned
crate already implements the standard DH/AES session.

### Claim Linux unsupported now

Premature: the protocol and pinned `zbus` can express the contract, and a small
fork boundary is identifiable. Unsupported remains the correct fail-closed
runtime/release result for a provider/version that fails the harness.

## Open risks and stop conditions

- **Provider divergence:** stop and mark that provider/version unsupported if
  UI appears during `SearchItems`, property reads, `GetSecret`, or before the
  client calls `Prompt`.
- **Refusal divergence:** if `Dismiss` on a never-executed prompt displays UI or
  cannot settle, use connection retirement. If UI still appears, stop support.
- **Fork breadth:** stop and choose direct `zbus` for separate review, or leave
  Linux unsupported, if deferred replies cannot be exposed without materially
  rewriting session/crypto code.
- **Ambiguity:** stop the operation on multiple exact matches. Do not repair or
  delete external entries automatically.
- **Unknown mutation state:** return `commit_unknown`, keep the recovery intent,
  and prohibit automatic retry until exact readback resolves it.
- **Provider stall:** a deadline does not cancel a sent D-Bus method. Retire the
  connection/worker and prohibit competing mutations until state is reconciled.
- **Unsupported crypto/interface:** missing DH algorithm, required interface,
  or unexpected result signature is `unsupported_store`/`backend_failure`, not
  permission to use `plain` or file fallback.
- **KWallet GPG mode:** do not advertise it as supported unless its separate
  deadline and dismissal job passes. Password-wallet proof does not imply GPG-
  wallet proof.
- **Version drift:** rerun both packaged-provider jobs for any fork, `zbus`,
  GNOME Keyring, or KWallet upgrade.

The release decision is fail-closed: Linux production v2 remains unavailable
until both the deterministic protocol suite and the applicable pinned real-
provider suite pass. A compile check or an already-unlocked happy-path test is
insufficient.

## Out-of-scope discoveries

None. Provider-specific failure modes were folded into this decision's required
harness rather than opened as unrelated work. Per this research worktree's
scope, no Seed records were changed.

## Primary sources

- [Secret Service specification 0.2](https://specifications.freedesktop.org/secret-service/latest-single/)
- [`secret-service` 5.1.0 crate and source](https://docs.rs/crate/secret-service/5.1.0)
- [`zbus-secret-service-keyring-store` 1.0.0 crate and source](https://docs.rs/crate/zbus-secret-service-keyring-store/1.0.0)
- [`zbus` 5.18.0 API](https://docs.rs/zbus/5.18.0/zbus/)
- [GNOME Keyring 48.0 source releases](https://download.gnome.org/sources/gnome-keyring/48/)
- [GNOME Keyring 48.0 prompt implementation](https://gitlab.gnome.org/GNOME/gnome-keyring/-/blob/48.0/daemon/dbus/gkd-secret-prompt.c)
- [GNOME Keyring daemon D-Bus test](https://gnome.pages.gitlab.gnome.org/gnome-keyring/coverage/daemon/dbus/test-service.c.gcov.html)
- [KDE Frameworks 6.28 releases](https://download.kde.org/stable/frameworks/6.28/)
- [KWallet 6.28 prompt implementation](https://invent.kde.org/frameworks/kwallet/-/blob/v6.28.0/src/runtime/ksecretd/kwalletfreedesktopprompt.cpp)
- [KWallet Secret Service plain-session support history](https://bugs.kde.org/show_bug.cgi?id=458341)
- [ADR-0035](../adr/0035-backend-owned-credential-service.md)
- [Credential service library evaluation](2026-07-31-credential-service-library-evaluation.md)
