# B21 — Safe Rust Edition 2021 → 2024 Migration (tail_expr_drop_order focus)

Research date: 2026-05-30 · Toolchain in-repo: `rust-toolchain.toml` channel `1.95.0`
(edition 2024 fully stable since 1.85.0). Crate: `src-tauri/` (`edition = "2021"`).

This report is research only. No source was modified. It grounds every claim in
either primary Rust docs or the *actual* lint output of this codebase.

---

## TL;DR for the implementer

- The 22 warnings are `tail_expr_drop_order` ("relative drop order changing in
  Rust 2024" / "this changes meaning in Rust 2024"). The lint is **allow-by-default**,
  has **no machine-applicable fix**, and `cargo fix --edition` will **not** rewrite
  these sites — they require manual, per-site review.
- The risk is real but narrow: in 2024, a temporary with a *significant* `Drop`
  (a `MutexGuard` / `RwLockReadGuard` / `RwLockWriteGuard` / pinned future / RAII
  guard) created **in a block or function tail expression** is now dropped
  **before** the block's named locals, instead of after. For locks this means a
  guard's lock is **released earlier**; for the pinned-future case a future is
  dropped in a different order.
- This codebase's dominant flagged pattern is `if let Ok(mut guard) = lock.write() { … }`
  (or `.read()`) used **as a tail/match-arm expression** while another guard is a
  local in the same block. Confirmed live at e.g. `src/commands.rs:2677`,
  `src/speech/mod.rs:2363/2371/2390/2400`, `src/gemini/mod.rs:1087`.
- Behaviour-preserving fix: **hoist the temporary into a `let` binding** (named
  locals keep 2021 ordering — they drop in reverse declaration order, *after* the
  tail temporary's new slot), or **explicitly `drop(guard)`**, or rewrite `if let`
  → `match`. Snippets below.
- CI must confirm on **all three OSes and all feature combos** because the *set* of
  flagged sites changes with `cfg(target_os=…)` and feature gates (`local-ml`,
  `diarization`, …). The 22-count is the default-feature build; the cloud-only build
  surfaces ~13. `cargo fix --edition` only sees code that compiles under the flags
  you pass it.

---

## 1. What the Rust 2024 tail-expression drop change actually does

### The rule change

A "tail expression" is the final expression of a block / function body / closure
body that has **no trailing semicolon** (it is the block's value). It also covers
the scrutinee position in some forms.

- **Edition ≤ 2021:** temporaries created while evaluating a tail expression had an
  *ill-specified, extended* scope — they were dropped **after** the block's local
  variables (effectively at the next larger temporary-scope boundary, e.g. end of
  the enclosing statement).
- **Edition 2024:** those temporaries are dropped **at the end of the block,
  *before* the block's named locals.** (RFC 3606.)

So the *relative* order between "a local declared earlier in the block" and "a
temporary born in the tail expression" **flips**.

Authoritative source:
`https://doc.rust-lang.org/edition-guide/rust-2024/temporary-tail-expr-scope.html`
and the rustc lint listing
`https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html#tail-expr-drop-order`.

### Why it matters for locks / channels (observable effects)

The change is observable **only for types with a "significant" `Drop`** — an
explicit `impl Drop` (or a type transitively containing one). Per rustc, `Vec`,
`Box`, `Rc`, `Arc`, `BTreeMap`, `HashMap` are *not* significant unless their generic
payload is. The significant cases relevant here:

| Type | What its `Drop` does (the observable side effect) |
|------|----|
| `std::sync::MutexGuard<T>` | **releases the mutex** |
| `RwLockReadGuard<T>` / `RwLockWriteGuard<T>` | **releases the read/write lock** |
| `crossbeam_channel::Sender` (last clone) | **closes/disconnects the channel** (wakes receivers with `Disconnected`) |
| `tokio::sync::*` guards, pinned futures (`tokio::pin!`) | order of async resource teardown |
| `tar`/file handles, `TranscriptWriter` (owns a file) | flush/close ordering |

If the moment a lock is released moves *earlier* relative to other work in the
block, you can get:
- **Earlier visibility** of mutations to other threads (a different interleaving).
- **Avoidance** of a self-deadlock (the *good* direction — see §the if-let twin
  lint below: the edition guide's headline example is exactly an `RwLock` read-lock
  held too long into an `else` that takes a write lock → deadlock; 2024 fixes it).
- **Reordering of two guards/sends** so that, e.g., a downstream channel observes a
  `Disconnected` or a state change in a different order than today.

The lint cannot tell whether the reorder is benign; it flags *every* significant-Drop
tail temporary and asks the author to decide. rustc's own note: "most of the time,
changing drop order is harmless."

### The canonical compile-vs-runtime split

There are two *different* manifestations, and only one of them is a warning:

1. **Borrow-shortening (compile-time) — the `RefCell` family.** A tail expr like
   `c.borrow().len()` *fails* to compile in 2021 (the `Ref` outlives `c`) but
   *compiles* in 2024 (the `Ref` now drops first). This is a strict improvement and
   does not produce a `tail_expr_drop_order` warning. Inverse: `{ &String::from("x") }.len()`
   compiled in 2021 but is an error in 2024 — fix by hoisting to a `let`.
2. **Side-effect reordering (run-time) — the lock/channel family.** Code compiles in
   *both* editions but the destructor *sequence* changes. **This is the
   `tail_expr_drop_order` warning, and it is all 22 of B21's hits.**

---

## 2. Recommended migration workflow

### 2.1 The two related lints (know which is auto-fixed)

| Lint | Group | Default | `cargo fix --edition` auto-rewrites it? | Applies to |
|------|-------|---------|------------------------------------------|------------|
| `tail_expr_drop_order` | `rust_2024_compatibility` | **allow** | **NO** — emits warnings only; "no semantics-preserving rewrites" exist, so it leaves the code alone for manual review | tail/block-final expr temporaries with significant Drop |
| `if_let_rescope` | `rust-2024-compatibility` | allow | **YES** — auto-rewrites `if let … {} else {}` → `match` to preserve 2021 drop timing | `if let` scrutinee temporaries with significant Drop |
| `let_chains` | (feature, stabilized 1.88) | n/a | n/a — `a && let P = e` chains are now legal; not a migration hazard, but they share the same scrutinee-scope rules | — |

Implication for B21: after `cargo fix --edition`, expect the **`if let` → `match`
auto-rewrites to appear in the diff** (these are the lock-in-scrutinee sites like
`src/commands.rs:2677`, `src/speech/mod.rs:*`, `src/gemini/mod.rs:1087`). The
`tail_expr_drop_order` warnings will **remain as warnings** and must be hand-audited.
Do not assume a clean `cargo fix` means the drop-order risk is resolved.

### 2.2 Step-by-step

Primary source: `https://doc.rust-lang.org/cargo/commands/cargo-fix.html` and the
edition guide. `cargo fix --edition` updates code but **does not** touch the
`edition` field in `Cargo.toml` — that is a deliberate, separate manual step.

```bash
# 0. Start clean (cargo fix refuses a dirty tree unless overridden).
git switch -c b21-edition-2024
git status   # must be clean

# 1. Surface the warnings WITHOUT migrating, so you can audit first.
#    Add to src-tauri/src/lib.rs + main.rs crate roots (temporary, for the audit):
#      #![warn(tail_expr_drop_order, if_let_rescope)]
#    Then, per feature/platform combo:
cargo check --all-features 2>&1 | tee /tmp/b21-default.txt
cargo check --no-default-features --features cloud 2>&1 | tee /tmp/b21-cloud.txt
#    (repeat per --target on each OS; see §4)

# 2. Run the migration. cargo fix only analyzes code that compiles under the
#    flags you give it — so run it once per feature set AND once per target that
#    gates code with cfg(...). This repo has cfg(target_os=...) deps + ML features.
cargo fix --edition --all-features
cargo fix --edition --no-default-features --features cloud
#    On macOS additionally (Metal-gated deps in Cargo.toml):
#      cargo fix --edition --all-features --target aarch64-apple-darwin
#    On Windows:
#      cargo fix --edition --all-features --target x86_64-pc-windows-msvc

# 3. Flip the edition (manual — cargo fix never does this).
#    src-tauri/Cargo.toml:  edition = "2021"  ->  edition = "2024"
#    (NOTE: edition 2024 requires rust-version / toolchain >= 1.85; repo pins 1.95 — OK.)

# 4. Rebuild every combo and run tests on every OS.
cargo build --all-features && cargo test --all-features
cargo build --no-default-features --features cloud && cargo test --no-default-features --features cloud

# 5. Re-run plain `cargo fix` (no --edition) to mop up any new idiom suggestions.
cargo fix --all-features
```

Caveats from the docs: "In some rare cases the compiler is unable to automatically
migrate all code … this may require manual changes after building with the new
edition." And: `cargo fix` "is only capable of fixing code that is normally compiled
with `cargo check`. If code is conditionally enabled with optional features, you will
need to enable those features."

### 2.3 How to audit each `tail_expr_drop_order` site

For every warning, read the diagnostic's three pointers (rustc prints all of them):
1. **The local** that "calls a custom destructor" and "will be dropped later as of
   Edition 2024" (e.g. `graph` / `sleep`).
2. **The temporary** (`#1`, `#2`, …) that "will be dropped earlier in Edition 2024".
3. **"now the temporary value is dropped here, before the local variables in the
   block or statement"** — the new 2024 drop point.

Then ask: *does either destructor have an externally observable effect, and does the
other party care about ordering?* For a `MutexGuard`/`RwLockGuard` the effect is
"lock released"; ask whether anything between the old and new drop points relies on
the lock still being held (re-reads it, sends on a channel under it, signals a
condvar, etc.). If yes → make the ordering explicit (§3). If the two guards protect
**independent** state and nothing observes the interleaving → it is benign; annotate
and `#[allow]` it or accept the change.

---

## 3. Concrete behaviour-preserving patterns (before / after)

The general principle: **named `let` locals always drop in reverse declaration order
and that did not change between editions.** So moving a flagged temporary *out of the
tail position and into a `let`* pins its drop point deterministically and identically
across editions. Three idioms:

### Pattern A — hoist the tail temporary into a `let` (most common fix)

This is the fix the rustc/std docs recommend for `tail_expr_drop_order`.

```rust
// BEFORE (2021 order: another_droppy drops, THEN the Droppy(1) temporary)
fn f() -> i32 {
    let another_droppy = Droppy(0);
    Droppy(1).get()          // <-- tail temporary; drops LAST in 2021, FIRST in 2024
}

// AFTER (identical, edition-independent: locals drop in reverse decl order)
fn f() -> i32 {
    let value = Droppy(1);   // give the temporary a name + a fixed drop slot
    let another_droppy = Droppy(0);
    value.get()              // tail is now a trivial copy; nothing significant to reorder
}
```

### Pattern B — explicit `drop(guard)` to make lock-release timing intentional

Best when the *intent* is "release this lock before doing X". This is the clearest,
self-documenting fix for the Mutex/RwLock cases and is edition-independent.

```rust
// BEFORE — guard lifetime is implicit; 2024 would release it earlier
fn snapshot(&self) -> Snapshot {
    let graph = self.knowledge_graph.lock().unwrap(); // MutexGuard local
    build_snapshot(&self.graph_snapshot.write().unwrap(), &graph) // RwLock temp in tail
}

// AFTER — name the second guard, drop both explicitly in the order you want
fn snapshot(&self) -> Snapshot {
    let graph = self.knowledge_graph.lock().unwrap();
    let mut gs = self.graph_snapshot.write().unwrap();
    let snap = build_snapshot(&gs, &graph);
    drop(gs);     // release write lock first ...
    drop(graph);  // ... then the mutex — explicit, identical on every edition
    snap
}
```

### Pattern C — `if let` scrutinee → `match` (this repo's #1 shape)

This is what `cargo fix --edition` produces for `if_let_rescope`, and the same shape
fixes the tail-position `if let Ok(guard) = lock.write() { … }` blocks here. The
`match` scrutinee temporary is extended to the end of the `match` (= 2021 behaviour).

```rust
// BEFORE — RwLockWriteGuard temporary in an if-let scrutinee, in tail position.
// Real shape from src/speech/mod.rs:2363 / src/commands.rs:2677.
if let Ok(mut status) = ctx.pipeline_status.write() {
    status.asr = StageStatus::Error { message: msg };
}   // 2021: write lock dropped at end of statement; 2024: dropped at `}` (earlier)

// AFTER — explicit, edition-stable. Either bind first:
{
    let mut status = ctx.pipeline_status.write().unwrap(); // or `if let Ok(mut status) = …`
    status.asr = StageStatus::Error { message: msg };
}   // guard's drop point is now the block end, named-local rules, both editions

// ...or the cargo-fix match rewrite (preserves 2021 timing across the else):
match ctx.pipeline_status.write() {
    Ok(mut status) => { status.asr = StageStatus::Error { message: msg }; }
    _ => {}
}
```

The edition guide's own motivating lock example (verbatim) — note the `else` takes a
**write** lock while the `read()` guard from the scrutinee is still alive in 2021,
deadlocking; 2024 releases it before the `else`:

```rust
// Before 2024
fn f(value: &RwLock<Option<bool>>) {
    if let Some(x) = *value.read().unwrap() {
        println!("value is {x}");
    } else {
        let mut v = value.write().unwrap();   // 2021: DEADLOCK (read lock still held)
        if v.is_none() { *v = Some(true); }
    }
    // <--- Read lock is dropped here in 2021
}
```

Here the 2024 change is a *bug fix*. The point: **for each site decide whether
"earlier release" is what you want.** If yes, accept 2024 / take the `match` rewrite
deliberately; if the old "held longer" behaviour was load-bearing, hoist + `drop()`
at the exact point you need.

### Pattern D — pinned-future / async tail (the `deepgram_aura.rs:588` case)

```rust
// BEFORE — `sleep` (tokio::pin!) is a local with a custom Drop; the awaited
// open_ws(...) future + &url/&api_key are tail temporaries that, in 2024, drop
// BEFORE `sleep`. Real site: src/tts/deepgram_aura.rs:588.
tokio::pin!(sleep);
// ...
match open_ws(&url, &api_key).await { /* arms */ }   // temporaries reorder vs `sleep`

// AFTER — bind the awaited result so the future's temporaries drop at a named slot,
// before the match runs; `sleep` then drops in its normal local order.
tokio::pin!(sleep);
let ws = open_ws(&url, &api_key).await;
match ws { /* arms */ }
```

> Audit note: in this repo the `open_ws` reconnect cases (deepgram_aura, gemini) and
> the `pipeline_status.write()` status-update cases are almost certainly benign —
> the reordered temporaries either have no cross-thread observer between old and new
> drop points, or releasing the lock earlier is harmless. They should still be
> rewritten for clarity (Pattern C/D) so the diff documents the decision, rather than
> blanket-`#[allow]`-ing the lint.

---

## 4. CI considerations (why all-platform matters here especially)

### Why drop-order needs all three OSes

1. **The flagged set is platform- and feature-dependent.** `cargo fix`/the lint only
   see code that compiles under the active `cfg`. This crate gates code three ways:
   - `cfg(target_os = "linux"|"windows"|"macos")` dep blocks (rsac features; macOS
     adds Metal-feature `whisper-rs`/`llama-cpp-2`).
   - feature flags `local-ml` (default) vs `cloud`, `diarization`,
     `diarization-clustering`, `sherpa-streaming`, `cuda`, `vulkan`.
   - `cpal::Stream` is `!Send` on Windows (WASAPI/COM) and is driven from a dedicated
     `std::thread` + `crossbeam-channel` command loop — that thread's guard/`Sender`
     drop ordering only exists on the Windows code path.
   A site that is clean on Linux can warn on Windows/macOS and vice-versa. The "22"
   count in the backlog is the **default-feature** build; the **cloud-only** build of
   this repo surfaces ~13 `tail_expr_drop_order` hits (verified locally,
   `cargo check --no-default-features --features cloud -W tail_expr_drop_order`).
   So CI must run the lint/`cargo fix` audit across the **matrix of
   {linux, windows, macos} × {default(local-ml), cloud, +diarization}**.

2. **Drop-order side effects can be timing/scheduler-sensitive at runtime.** A
   reordered lock release or channel disconnect may only manifest as a hang, a
   dropped event, or a flaky test under a particular OS scheduler. Run the full
   `cargo test` suite on each OS after flipping the edition — not just a build.
   (Repo reality: `docs/reviews` / ADR-0007 note Windows can't run the *full* test
   harness — `STATUS_ENTRYPOINT_NOT_FOUND` from native ML link; a subset runs via
   `scripts/run-core-tests.ps1`. For B21, run the **cloud-only** `cargo test` on
   Windows, which links cleanly, plus the core-test subset, and rely on
   Linux/macOS for the local-ml paths.)

### What to grep/watch for in Mutex/mpsc/crossbeam code specifically

This codebase has ~425 `.lock()/.read()/.write()/.send()/.recv()` sites across 30
files (heaviest: `commands.rs` 118, `speech/mod.rs` 39, `tts/deepgram_aura.rs` 34,
`gemini/mod.rs` 31). Focus the audit on:

- **Two-guard blocks** where one guard is a `let` local and a *second* guard is
  acquired in the tail/`if let` (the `commands.rs:2677` shape) — reorder risk is real
  if both protect related state or there is a lock-ordering invariant.
- **`drop(tx)` / last-`Sender`-drop as the disconnect signal.** crossbeam/mpsc
  receivers treat all-senders-dropped as `Disconnected`. If a `Sender` is a tail
  temporary, 2024 may disconnect the channel *earlier* relative to other teardown in
  the block — watch for receivers that race a final send vs the disconnect, and for
  `recv()`/`select!` loops keyed on `Disconnected`.
- **`std::sync::Mutex` poisoning:** the guard's `Drop` is what records poison on a
  panic-unwind. Reordering guard drops slightly changes which guard poisons first on
  a multi-lock panic. Low risk but note it for the `.lock().unwrap()` sites.
- **`tokio::pin!` futures and tokio `*Guard`s** in `async` tail position
  (deepgram/gemini reconnect loops) — Pattern D.
- **`!Send` `cpal::Stream` + crossbeam command loop** (Windows-only) — verify the
  stream/sender drop order on the Windows path explicitly.

### Suggested CI gate

After migration, keep the regression guard on by adding to the crate roots:

```rust
#![deny(tail_expr_drop_order, if_let_rescope)]  // once all sites are reviewed/fixed
```

and run `cargo clippy --all-features -- -D warnings` plus the cloud build in the
existing per-OS CI matrix, so any *new* tail-drop hazard fails the build rather than
silently changing lock timing.

---

## Appendix — Real flagged sites in this repo (cloud-build subset, verified locally)

`RUSTFLAGS="-W tail_expr_drop_order" cargo check --no-default-features --features cloud`
on toolchain 1.95.0 (Windows) reported (excerpt — line:col):

- `src/commands.rs:2677` — `if let Ok(mut gs) = state.graph_snapshot.write()` tail in a
  block whose local `graph` is a `MutexGuard`; diagnostic note points at
  `std/src/sync/poison/rwlock.rs` Drop. (Pattern C.)
- `src/speech/mod.rs:1992, 2363, 2371, 2390, 2400` — `if let Ok(mut status) =
  ctx.pipeline_status.write()` status updates. (Pattern C.)
- `src/gemini/mod.rs:830, 1087` and `src/tts/deepgram_aura.rs:588` — awaited
  `open_ws(...)` reconnect tails alongside a pinned `sleep` / writer-reader locals.
  (Pattern D.)
- `src/asr/assemblyai.rs:488`, `src/asr/deepgram.rs:675`, `src/aws_util/mod.rs:261`,
  `src/playback/mod.rs:308` — assorted significant-Drop tail temporaries.

The full default-feature build (which compiles whisper-rs/llama-cpp-2/mistralrs and
the diarization paths) is where the backlog's count of **22** comes from; run the
audit per feature/target combo per §4. Two additional non-drop 2024 changes also
surfaced and are auto-fixable by `cargo fix --edition`: `ref` in implicitly-borrowing
patterns (`speech/mod.rs:1093`) and `std::env::set_var` now `unsafe`
(`gemini/mod.rs:791`); plus the rsac path-dependency emits its own `expr` macro-fragment
and drop-order warnings (rsac migrates separately).

## Primary sources

- Edition guide — tail expr temporary scope:
  https://doc.rust-lang.org/edition-guide/rust-2024/temporary-tail-expr-scope.html
- Edition guide — if let temporary scope (lock deadlock example + match rewrite):
  https://doc.rust-lang.org/edition-guide/rust-2024/temporary-if-let-scope.html
- rustc lint listing (tail_expr_drop_order, if_let_rescope, "significant Drop" def):
  https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html
- cargo fix --edition behaviour & flags:
  https://doc.rust-lang.org/cargo/commands/cargo-fix.html
- RFC 3606 (tail-expr temporary scope): https://github.com/rust-lang/rfcs/pull/3606
- rustc impl detail: `compiler/rustc_mir_transform/src/lint_tail_expr_drop_order.rs`
