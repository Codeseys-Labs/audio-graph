# Releasing AudioGraph

This document covers the full path from "version is good" to signed release
artifacts on a GitHub Release. It's split into three stages:

1. **Cut a release** — bump versions, update CHANGELOG, tag.
2. **Let CI build** — `.github/workflows/release.yml` fires on tag push.
3. **(Optional) sign + notarize** — if the right GitHub secrets are set,
   tauri-action signs everything. Otherwise you ship unsigned artifacts.

## 1. Cut a release

```bash
cd audio-graph

# Bump the three version locations + rotate CHANGELOG.md.
./scripts/bump-version.sh 0.2.0

# Review: should touch package.json + src-tauri/Cargo.toml +
# src-tauri/tauri.conf.json (versions) and CHANGELOG.md (rotation).
git diff

# Commit, tag, push.
git add -A
git commit -m "chore: release 0.2.0"
git tag -a v0.2.0 -m "Release 0.2.0"
git push origin master v0.2.0
```

Tag push → `release.yml` workflow fires automatically.

### Pre-releases

The script accepts any `X.Y.Z[-prerelease]` string:

```bash
./scripts/bump-version.sh 0.2.0-rc.1
git tag v0.2.0-rc.1
```

tauri-action still treats these as draft releases — you have to publish
them by hand from the GitHub Releases page.

Windows release builds ship NSIS plus the standalone executable. MSI is not a
bundle target, so pre-release tags no longer depend on an MSI packaging path.
Still prefer a clean `vX.Y.Z` tag for the first GA.

## 2. What the release workflow does

`.github/workflows/release.yml`:

1. **Resolve release inputs** on Blacksmith Ubuntu 24.04: release tag and pinned
   `rsac` commit SHA.
2. **Parallel builds** on Blacksmith macOS 15, Ubuntu 24.04, and Windows 2025.
3. For macOS, builds a **universal binary** (arm64 + x86_64) so one bundle
   runs on both Apple Silicon and Intel Macs. (The Apple targets are installed
   via `dtolnay/rust-toolchain` with a `targets:` input — a bare
   `rustup target add` did not persist to tauri-action's toolchain.) Linux
   installs `libasound2-dev` (the `alsa-sys`/cpal dependency) alongside the GTK
   stack, matching `ci.yml`.
4. **Bundle targets** are pinned in `tauri.conf.json` → `bundle.targets` =
   `["app", "dmg", "nsis", "appimage", "deb"]` (no implicit "all", no MSI/RPM).

### Artifact taxonomy — standalone primary, installer secondary

Each platform ships a **no-install standalone (primary)** plus a convenience
**installer (secondary)**:

| Platform | Standalone (primary) | Installer (secondary) |
|---|---|---|
| Windows | `AudioGraph_<ver>_x64_standalone.exe` — runs in place; WebView2 is Evergreen on Win11 | NSIS `.exe` |
| macOS | `.app.tar.gz` — unpack → run (drag to /Applications optional) | `.dmg` |
| Linux | `.AppImage` — `chmod +x` → run, single file | `.deb` |

tauri-action emits the bundled artifacts: NSIS, DMG, `.app.tar.gz`, AppImage,
and deb. It does **not** emit the bare Windows `.exe` / Linux ELF, so a
post-build `gh release upload` step attaches those to the same draft release
(`audio-graph[.exe]` from `src-tauri/target/release/`) during non-dry runs.
The Windows standalone is unsigned until Authenticode is procured (§3), so
SmartScreen will warn.

5. **Non-dry runs** (tag push, or manual dispatch with `dry_run: false`) create a
   GitHub Release draft, upload the bundled artifacts, attach the rsac revision
   manifests, and attach the standalone Windows/Linux binaries.
6. **Dry runs** (manual dispatch with `dry_run: true`) skip GitHub Release
   creation and every `gh release upload`/tauri-action publishing step. They run
   `bun run tauri build --ci --no-sign` and upload Actions artifacts named
   `release-dry-run-macos`, `release-dry-run-linux`, and
   `release-dry-run-windows`.
7. Non-dry workflow runs leave the release **as a draft** — you review the
   files, polish the release notes, and hit "Publish release" by hand.

### Manual dispatch

You can also trigger the workflow from the Actions UI (`workflow_dispatch`).
Use this for clean-ref release evidence before pushing a tag: select the branch
or commit to prove, set `dry_run: true`, and optionally override `rsac_sha` with
a full 40-character commit SHA. Dry runs do not create tags, GitHub Releases, or
release uploads.

Manual non-dry dispatch (`dry_run: false`) has the same publishing behavior as a
tag push and should be treated as approval-gated: it creates a draft GitHub
Release for the latest reachable tag on the selected ref and uploads release
artifacts to it.

## 3. Code signing + notarization (optional)

> **Status (B26): engineering-complete, procurement-pending.** The CI
> plumbing is fully wired — `release.yml` forwards all 10 signing secrets
> (6 Apple + 2 Windows + 2 updater) to `tauri-action`, and every secret's
> generation is documented below. The *only* remaining work is external
> credential **procurement**, which cannot be done in code:
>
> 1. **Apple** — enroll in the Apple Developer Program ($99/yr), create a
>    Developer ID Application certificate, generate an app-specific
>    password, then populate the 6 `APPLE_*` secrets (steps below).
> 2. **Windows** — purchase an Authenticode cert from a CA (~$300–$500/yr;
>    EV for instant SmartScreen reputation), then populate the 2
>    `WINDOWS_*` secrets.
> 3. **Updater** (only if/when auto-update is enabled) — generate the Tauri
>    updater keypair locally and populate the 2 `TAURI_SIGNING_*` secrets.
>
> No engineering change is required for any of these — paste the secrets in
> **Settings → Secrets and variables → Actions** and the next tagged
> release signs automatically. Until then, artifacts ship unsigned (see
> below). This is the one backlog item that is intentionally *not* closeable
> by engineering.

Without any signing secrets configured, artifacts still build but:

- **macOS:** users see `"AudioGraph can't be opened because Apple cannot
  check it for malicious software"` on first launch. Right-click → Open
  bypasses, but Gatekeeper will nag. Fatal for any real distribution.
- **Windows:** SmartScreen shows an "Unrecognized app" warning. Users can
  click "More info → Run anyway" but most won't.
- **Linux:** no signing infrastructure to worry about. AppImage + deb
  both ship unsigned by default and nobody notices.

To enable signing, add the corresponding GitHub Actions secrets to the
repository (**Settings → Secrets and variables → Actions**). `release.yml`
already forwards all of these to tauri-action via `env:` — you just need
to populate them.

### Apple (macOS)

You need an **Apple Developer Program membership** ($99/year) and a
**Developer ID Application certificate**.

| Secret | How to get it |
|--------|---------------|
| `APPLE_CERTIFICATE` | Export your Developer ID cert as a `.p12` from Keychain Access, then `base64 -i DeveloperID.p12 \| pbcopy`. Paste the base64 string. |
| `APPLE_CERTIFICATE_PASSWORD` | The password you set when exporting the `.p12`. |
| `APPLE_SIGNING_IDENTITY` | The Common Name (CN) of the cert, e.g. `Developer ID Application: Your Name (TEAMID)`. Get with `security find-identity -v -p codesigning`. |
| `APPLE_ID` | Your Apple ID email. |
| `APPLE_PASSWORD` | An **app-specific password** (not your Apple ID password). Generate at https://appleid.apple.com → "App-Specific Passwords". |
| `APPLE_TEAM_ID` | The 10-character team ID visible at https://developer.apple.com/account → Membership. |

With all six present, tauri-action signs the app bundle AND submits the
DMG for notarization (Apple's out-of-band malware scan) before the
workflow completes. Notarization typically takes 5–15 minutes.

### Windows (Authenticode)

You need an **Authenticode code signing certificate** from a CA like
DigiCert, Sectigo, or SSL.com (~$300–$500/year). EV certs get instant
SmartScreen reputation; OV certs take months of signed-binary telemetry.

| Secret | Notes |
|--------|-------|
| `WINDOWS_CERTIFICATE` | Base64-encoded `.pfx` (PKCS#12). |
| `WINDOWS_CERTIFICATE_PASSWORD` | Password for the `.pfx`. |

tauri-action signs the NSIS installer if the Windows signing secrets are present.
MSI is intentionally not a bundle target.

### Tauri updater signing (separate)

If you later wire up Tauri's built-in auto-updater, it uses its **own**
signing key (distinct from OS code signing). Generate once:

```bash
bun tauri signer generate -w ~/.tauri/audiograph-updater.key
# Prints a public key — put that into tauri.conf.json → plugins → updater.
# Put the private key contents into the TAURI_SIGNING_PRIVATE_KEY secret.
# Put the password you set into TAURI_SIGNING_PRIVATE_KEY_PASSWORD.
```

Without these, the app bundle still builds but the updater can't verify
update signatures (so leave updater disabled until the keys exist).

## 4. Troubleshooting

**`rsac` path dep not found in CI.** The release workflow stages the
parent rsac repo around the audio-graph checkout (same trick as the PR
CI — see `.github/workflows/ci.yml`). If you see `failed to load source
for dependency rsac` in a build log, something in that staging step is
wrong.

**Notarization hangs.** Apple's notary service has had multi-hour
outages. Check https://www.apple.com/support/systemstatus/. If it's
down, cancel the workflow and re-run when the service is healthy.

**Artifacts are unsigned but secrets look right.** tauri-action is strict
about all-or-nothing: missing any one of the 6 Apple secrets → silently
skips signing. Double-check the `APPLE_SIGNING_IDENTITY` exactly matches
`security find-identity -v -p codesigning` output, case and all.

**Draft release doesn't have all artifacts.** The workflow uses
`fail-fast: false` so a macOS notarization failure doesn't kill the
Linux/Windows builds. Check the failed job's log and re-run just that
matrix entry from the Actions UI.

## 5. Checklist for cutting a release

- [ ] All PRs merged; CI green on master.
- [ ] `./scripts/bump-version.sh X.Y.Z` run and diff reviewed.
- [ ] CHANGELOG entries written under the new version section.
- [ ] Release workflow dry-run dispatched from a clean release-prep ref with
      `dry_run: true` and the intended pinned `rsac_sha`; all three
      `release-dry-run-*` Actions artifacts reviewed.
- [ ] `git tag -a vX.Y.Z -m "Release X.Y.Z"`.
- [ ] `git push origin master vX.Y.Z`.
- [ ] Watch the Release workflow finish (typically 20–30 minutes).
- [ ] Review the draft release on GitHub.
- [ ] Download + smoke-test the DMG / NSIS installer / AppImage on real hardware.
- [ ] Publish the release.
