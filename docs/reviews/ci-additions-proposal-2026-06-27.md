# CI Additions Proposal — 2026-06-27 (awaiting approval)

**Status:** PROPOSAL ONLY. Nothing here is committed or pushed. CI changes are
approval-gated per the mission guardrail; this document is the review package.

Two distinct CI changes are proposed, on top of the **already-reviewed** held CI
diff (`ci.yml`/`release.yml`/`actionlint.yaml` — verdict: safe to commit to the
branch, see the CI review in task #2 / the session log). Keep them separate so
each gets its own sign-off.

---

## Change 0 (already in the dirty tree, reviewed — for context)

The held `ci.yml` already adds a Blacksmith 3-OS matrix
(`blacksmith-4vcpu-ubuntu-2404`, `blacksmith-6vcpu-macos-15`,
`blacksmith-4vcpu-windows-2025`) with `rust-cloud-smoke`,
`rust-optional-feature-smoke` (15-cell), and `tauri-default-smoke`. The two new
changes below slot into this matrix.

**Operational prerequisites (from the CI review — must be true before any remote run):**
1. Blacksmith runners provisioned on the org with those exact labels (else jobs queue forever).
2. Cost sign-off on the nightly 15-cell ML matrix (consider `concurrency: cancel-in-progress`).
3. The `rsac_sha` pin is duplicated in 3 places — keep in lockstep on bumps.
4. First release run via `workflow_dispatch` `dry_run=true` before any tag push.

---

## Change 1 — Seed 2b2c macOS storage-engine leg (the gated storage evidence)

**Why:** ADR-0021's storage decision is `gated-on-evidence` on seed `2b2c`. Linux
(native) and Windows (cross-compile) legs are **done** locally
(`docs/reviews/2b2c-local-linux-evidence-2026-06-27.md`): both `kv-surrealkv` and
`kv-rocksdb` compile+link on both; SurrealKV wins on size; RocksDB-on-Windows
cross-compiled clean (contradicting the ADR's feared failure mode). The **macOS
leg is the only missing platform** and cannot be done off-Apple-hardware (Apple
SDK license), so it must run on the `blacksmith-6vcpu-macos-15` runner.

**Shape (a throwaway, schema-independent evidence job — NOT wired into the product):**

```yaml
  storage-engine-evidence:
    name: 2b2c storage-engine probe (${{ matrix.os }})
    # Manual + nightly only — this is evidence-gathering, never a PR gate.
    if: github.event_name == 'workflow_dispatch' || github.event_name == 'schedule'
    runs-on: ${{ matrix.runner }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - { os: macos,   runner: blacksmith-6vcpu-macos-15 }
          - { os: windows, runner: blacksmith-4vcpu-windows-2025 }   # native run, complements the cross-compile evidence
          - { os: linux,   runner: blacksmith-4vcpu-ubuntu-2404 }    # native baseline
    steps:
      - <checkout + rsac fetch + per-OS system deps, same as the cloud-smoke job>
      # Windows native prereqs surfaced by the cross-compile probe:
      - if: matrix.os == 'windows'
        run: choco install nasm -y          # aws-lc-sys crypto asm; rustc must be >=1.94
      # Throwaway feature add — a scratch step edits Cargo.toml in the runner only,
      # OR (cleaner) a dedicated scratch crate under e.g. ci/storage-probe/ that
      # depends on surrealdb with kv-surrealkv / kv-rocksdb. Prefer the scratch
      # crate so the product Cargo.toml is never touched.
      - name: Build + link kv-surrealkv
        run: cargo build --release -p storage-probe --features surrealkv
      - name: Build + link kv-rocksdb
        run: cargo build --release -p storage-probe --features rocksdb
      - name: Durability probe (write N, kill-9 mid-write, reopen, verify; corrupt, verify failure mode)
        run: <the kill-9 + corruption probe, per engine — Linux already proven, extend to mac/win>
      - name: Record build/link time + stripped size + native-dep inventory
        run: <emit a per-OS evidence table artifact>
      - uses: actions/upload-artifact@<sha>
        with: { name: 2b2c-evidence-${{ matrix.os }}, path: evidence/ }
```

**Acceptance (closes the 2b2c gate):** per-OS (mac/win native, + linux baseline)
build/link result, stripped size delta, native-dep inventory, and a kill-9
durability + corruption result for BOTH engines. Feeds the ADR-0021 decision
rule: advance `48bb` (indexed rewrite, selectable-not-default) only if
`kv-surrealkv` is green on all 3 OSes; else keep the file default.

**Risk:** low — it's an opt-in (`workflow_dispatch`/`schedule`) evidence job that
writes nothing to the product. The only product-repo addition is a scratch
`ci/storage-probe/` crate (or a guarded scratch step).

---

## Change 2 — Seed 0d66 live-audio e2e (virtual devices, all 3 OSes)

**Why:** real device round-trip e2e for `rsac` capture + CPAL playback — today CI
only runs the synthetic-buffer playback resampling test (`ci.yml:248–258`). Full
research + decision in `docs/research/ci-virtual-audio-devices-2026-06-27.md`:
**ADOPT** LABSN/sound-ci-helpers (BSD-3, one composite action, all 3 OSes) +
per-OS shims.

**Shape (gated `live-audio-smoke` job, mirrors the optional-feature matrix gating):**

```yaml
  live-audio-smoke:
    name: Live rsac audio smoke (${{ matrix.os }})
    if: github.event_name == 'workflow_dispatch' || github.event_name == 'schedule'  # not PRs
    runs-on: ${{ matrix.runner }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - { os: linux,   runner: blacksmith-4vcpu-ubuntu-2404 }
          - { os: macos,   runner: blacksmith-6vcpu-macos-15 }
          # windows leg gated on seed d3d3 (VB-CABLE CI licensing) — add when resolved
    steps:
      - <checkout + rsac fetch>
      - uses: LABSN/sound-ci-helpers@v1     # baseline: a device exists, all OSes
      # Linux capture-back shim (LABSN's Pulse path doesn't guarantee a monitor):
      - if: matrix.os == 'linux'
        run: |
          pactl load-module module-null-sink sink_name=ag_sink \
            sink_properties=device.description=ag_sink
          # rsac enumerates PipeWire; the .monitor source is the capture-back path.
      - if: matrix.os == 'macos'
        run: |
          brew install blackhole-2ch || true   # LABSN installs Background Music; BlackHole is the rsac-friendly loopback
          sudo killall coreaudiod               # macOS 14.4+: killall, not kickstart -k
      - name: Live audio smoke (feature-gated)
        run: xvfb-run -a cargo test -p audio-graph --lib --features live-audio-smoke live_audio -- --nocapture --test-threads=1
      - name: Upload enumeration logs on failure
        if: failure()
        uses: actions/upload-artifact@<sha>
        with: { name: audio-enum-${{ matrix.os }}, path: target/audio-smoke-logs/ }
```

**Plus a new `live-audio-smoke` Cargo feature** gating the real-device test code
(so normal builds/tests never open a device). The test: feed known PCM into the
virtual sink → capture back through rsac → assert source-id + format negotiation;
play out via CPAL → verify.

**Windows leg is HELD** behind seed `d3d3` (VB-CABLE expects a paid license for
automated/server use — resolve before enabling). Linux + macOS legs are
unblocked.

**Acceptance (closes 0d66):** workflow_dispatch/scheduled live smokes run on
Linux+macOS (Windows after d3d3); PRs keep lightweight checks; failures attach
enumeration logs. Unblocks `f166`, `09a7`, and the release epic `c395`.

---

## Recommended sequencing

1. Commit the **already-reviewed** held CI diff (Change 0) to the branch — safe per review.
2. Add **Change 1 (2b2c macOS)** — smallest, closes the storage gate; the macOS
   leg is the specific thing the user asked Blacksmith for.
3. Add **Change 2 (0d66 audio)** Linux+macOS legs; hold the Windows leg on `d3d3`.
4. None of these *run* until the operational prereqs (Blacksmith provisioning,
   cost sign-off) are met — those are the human-eyes items from the CI review.
