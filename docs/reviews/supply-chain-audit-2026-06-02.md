# Supply-chain / dependency CVE audit — 2026-06-02

Independent verification of the dependency tree against the RustSec advisory DB
(via Tavily/Exa research against rustsec.org + GHSA), cross-checked against the
resolved `Cargo.lock` versions and our actual call sites. This is the
previously-unaudited dimension from the drive-to-zero loop; it came back **clean
— no live, reachable advisory.**

## Method

Enumerated the resolved versions of every network/crypto/parse-exposed crate in
`src-tauri/Cargo.lock` (rustls, hyper, h2, reqwest, tokio, openssl, ring, tar,
bzip2, flate2, zip, tokio-tungstenite, serde_json, url, idna, time, chrono,
bytes), then checked each against RustSec 2025/2026 advisories and confirmed (a)
the resolved version vs. the advisory's patched range and (b) whether our code
reaches the affected API.

## Findings — all resolved or not-reachable

| Crate (resolved) | Advisory | Status |
|---|---|---|
| `tar` 0.4.45 | RUSTSEC-2026-0067/0068 (CVE-2026-33055/33056): `unpack_in` symlink chmod + PAX size-header desync | **Patched.** Fix landed in 0.4.45; our lockfile already resolves 0.4.45 (`tar = "0.4"` floated up). Our only call site is `models/mod.rs:719` `Archive::new(decoder).unpack(...)` for trusted model archives. |
| `bytes` 1.11.1 | RUSTSEC-2026-0007: `BytesMut::reserve` integer overflow | **Patched** (fix in 1.11.1; we're on 1.11.1). |
| `time` 0.3.47 | RUSTSEC-2026-0009: DoS via stack exhaustion (CVE-2026-25727) | **Patched** (fix 0.3.47; we're on 0.3.47). |
| `openssl` 0.10.80 | RUSTSEC-2025-0022: UAF in `Md::fetch`/`Cipher::fetch` with `Some(properties)` | **Patched** (fix `>=0.10.72`; we're on 0.10.80). Plus: zero direct `openssl::` usage in our code. |
| `rustls` 0.21.12 (transitive) | CVE-2024-32650: DoS infinite loop in blocking `complete_io` | **Not reachable.** Pure-transitive via the **async** `aws-smithy-http-client`; we have zero direct `rustls`/`complete_io` usage, and the async AWS path does not call `complete_io`. |
| `rustls-webpki` (via rustls 0.21) | RUSTSEC-2026-0098/0099/0104: name-constraint URI bypass / wildcard cert / CRL-parse panic | **Documented-not-reachable** in `.cargo/audit.toml`: AudioGraph talks to AWS/Deepgram/AssemblyAI/Gemini over public CA chains and does not parse CRLs, so the affected paths are unreachable. Unblock: AWS SDK migrates off rustls 0.21. |

## The CI gate already covers this continuously

`.github/workflows/ci.yml` has a **`security-audit` job** (`cargo audit`, hard
gate) running on every push on a Blacksmith Ubuntu runner. `.cargo/audit.toml`
holds the ignore-list — and every entry is categorized with its source crate,
blocker, threat-model, and unblock trigger (the rustls-webpki AWS chain, the
Tauri-v2 GTK3 unmaintained bindings, and unmaintained build/UI helper crates).
PR #20's `cargo audit` check is green. So advisory drift is caught automatically;
this manual pass independently cross-validated the gate and found no gap.

## Duplicate transitive deps (bloat, not vulnerability)

`Cargo.lock` carries duplicate majors — reqwest 0.12+0.13, hyper 0.14+1.9,
rustls 0.21+0.23, h2 0.3+0.4. These are pulled by **upstream crates we don't
control**: the old line comes from `mistralrs` (via hf-hub, openai-harmony) and
the `aws-sdk` (via aws-smithy-http-client); the new line is our direct reqwest
0.13. Not dedupable without upstream major bumps; not a security issue (the old
lines are either patched or not-reachable per the table above). Tracked as the
B32-majors deferral (justified: upstream-gated).

## Verdict

The supply-chain dimension is **clean**. No code change required. The mature
`cargo audit` CI gate + justified ignore-list is the correct steady state.
