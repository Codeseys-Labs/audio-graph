# CI Virtual Audio Devices + Loopback — Options Report

**Date:** 2026-06-27
**Seed:** audio-graph-0d66 ("Live rsac audio smoke tests on CI-capable runners") — blocks audio-graph-f166, audio-graph-09a7, audio-graph-c395.
**Question:** Can every CI runner (GitHub-hosted OR Blacksmith) boot with a virtual audio device + loopback so audio-graph can (a) enumerate a capture device, (b) feed known PCM in, (c) capture it back through `rsac`, and (d) play audio out and verify — true e2e audio in headless CI?

**Answer:** Yes on all three OSes, unattended, with no reboot — by **adopting** off-the-shelf virtual drivers. Nothing needs to be built. The single highest-leverage finding is **LABSN/sound-ci-helpers**, a maintained BSD-3 composite Action that already does all three OSes and is green on the current runner fleet (verified 2026-06-22).

---

## TL;DR recommendation per OS

| OS | Adopt | Mechanism | Reboot? | Headless? | Works on GitHub-hosted? | Notes |
|----|-------|-----------|---------|-----------|------------------------|-------|
| **Linux** | **PipeWire null-sink + `.monitor`** | userspace `pactl load-module module-null-sink` over PipeWire (PPA already installed) | No | Yes | Yes (userspace) | `rsac` capture **is** PipeWire — this is the only path that satisfies rsac. `snd-aloop` is **not** loadable on GitHub-hosted runners. |
| **macOS** | **BlackHole 2ch** (`brew install blackhole-2ch`) | user-space CoreAudio HAL plugin (AudioServerPlugin, **not a kext**) + `sudo killall coreaudiod` | No | Yes | Yes (GitHub even preinstalls it) | Loopback device: output→input on one device. GPLv3. |
| **Windows** | **VB-CABLE** (cert-pre-trust + silent install) | signed WDM driver, `Import-Certificate … TrustedPublisher` then `-i -h` / `pnputil`, then `net start audiosrv` | No | Yes | Yes (proven by live workflows) | Bare runners have **no** audio device + audio service off; a virtual driver is mandatory. WASAPI loopback alone is NOT enough. |

**Cross-cutting recommendation:** wrap all three behind **`LABSN/sound-ci-helpers@v1`** as the baseline ("a device exists"), and add a thin per-OS shim only where audio-graph needs a *guaranteed loopback capture-back* (Linux null-sink `.monitor`; Windows `net start audiosrv` + endpoint poll; macOS is fine as-is). See [§7](#7-how-this-plugs-into-seed-0d66).

---

## 0. Why this is non-trivial: the runner baseline

CI runners do **not** ship a sound card. Each OS fails differently on a bare runner, which is why a virtual device must be created at job time:

- **Linux (GitHub-hosted):** the Azure VM kernel is built **without ALSA/sound support** and GitHub has **no plans to rebuild it** — so `modprobe snd-aloop`/`snd-dummy` fails (`.ko` absent). Confirmed repeatedly, most recently Feb 2026. ([actions/runner-images#13610](https://github.com/actions/runner-images/issues/13610), [#8295](https://github.com/actions/runner-images/issues/8295), [#1114](https://github.com/actions/runner-images/issues/1114), [Azure/AKS#2335](https://github.com/Azure/AKS/issues/2335)). The only Linux option on hosted runners is a **userspace** sound server (PulseAudio/PipeWire).
- **macOS (GitHub-hosted):** images **already preinstall BlackHole 2ch** as the default device — precisely *because* there is no hardware and a kext (Soundflower) "requires user interaction." ([actions/runner-images#3526](https://github.com/actions/runner-images/issues/3526), [PR#3542](https://github.com/actions/runner-images/pull/3542/files)). But the preinstalled device flakes (~30% null-device init failures on macos-15: [#13668](https://github.com/actions/runner-images/issues/13668)), so install it explicitly.
- **Windows (GitHub-hosted):** `Get-CimInstance Win32_SoundDevice` returns nothing, and the **Windows Audio service is not started by default** on `windows-2022+`. GitHub policy: *"We do not pre-install any virtual hardware… you can add virtual hardware during runtime."* ([actions/runner-images#6983](https://github.com/actions/runner-images/issues/6983), [#2528](https://github.com/actions/runner-images/issues/2528)).

audio-graph's matrix runs on Blacksmith (`blacksmith-4vcpu-ubuntu-2404`, `blacksmith-6vcpu-macos-15`, `blacksmith-4vcpu-windows-2025`). Blacksmith images carry the **same Apple/Windows platform constraints** (no kext without reboot; Windows audio off by default), but you control the image, so you can pre-bake certs / drivers as an optimization. None of the recommendations *require* a custom image, however — all three work on stock GitHub-hosted runners too.

**How rsac/CPAL map to these:**
- `rsac` Linux capture is **PipeWire** (node monitors, device-change events on the PipeWire loop thread) — *not* raw ALSA. So the Linux virtual device must be a PipeWire node. ([rsac README](https://github.com/Codeseys-Labs/rust-crossplat-audio-capture/blob/master/README.md))
- `rsac` Windows capture is **WASAPI process/system loopback** — which still needs a render endpoint to exist.
- `rsac` macOS capture is **CoreAudio** — needs an enumerable input device.
- Playback (`src-tauri/src/playback/mod.rs`) is **CPAL** on `default_host()` (WASAPI / CoreAudio / ALSA→PipeWire bridge). It plays into the default output device.
- Today CI only runs the **playback resampling regression** under `xvfb-run` (synthetic buffers, no real device round-trip): `.github/workflows/ci.yml` lines 248–258. Seed 0d66 is about adding the *real* device round-trip.

---

## 1. LABSN/sound-ci-helpers — the turnkey baseline (all 3 OSes)

**Repo:** <https://github.com/LABSN/sound-ci-helpers> · **License:** BSD-3-Clause · **Maintainer:** Eric Larson (`larsoner`, MNE-Python). **Last commit 2026-06-22** (bumped runner list + `actions/checkout@v7`); latest release **v1.0.4 (2025-04-29)**; floating `@v1` recommended.

It is a **composite Action** that dispatches to one of three per-OS scripts by `runner.os` ([action.yml](https://github.com/LABSN/sound-ci-helpers/blob/main/action.yml)):

```yaml
- uses: LABSN/sound-ci-helpers@v1   # place after actions/checkout, before audio tests
```

What it does per OS:
- **Linux** ([linux/setup_sound.sh](https://github.com/LABSN/sound-ci-helpers/blob/main/linux/setup_sound.sh)): `apt install pulseaudio libportaudio2 dbus-x11 libasound-dev`, then `systemctl --user restart pulseaudio.*`. Relies on PulseAudio's auto/dummy sink. **No loopback monitor is configured** — a sink exists but capturing back what you played is not guaranteed. No reboot.
- **macOS** ([macos/setup_sound.sh](https://github.com/LABSN/sound-ci-helpers/blob/main/macos/setup_sound.sh)): installs **Background Music** cask (Kyle Neideck's AudioServerPlugin, like BlackHole), then `sudo launchctl kickstart -kp system/com.apple.audio.coreaudiod || sudo killall coreaudiod`. No reboot.
- **Windows** ([windows/setup_sound.ps1](https://github.com/LABSN/sound-ci-helpers/blob/main/windows/setup_sound.ps1)): installs **VB-CABLE** — bundles VB-Audio's signing cert (`vbcable.cer`) + a `devcon.exe`, runs `certutil -addstore TrustedPublisher vbcable.cer` to suppress the device-software dialog, then `devcon install vbcable\vbMmeCable64_win7.inf VBAudioVACWDM`. VB-CABLE is properly signed, so **no Secure Boot / test-signing reboot** is needed. (LABSN deliberately chose VB-CABLE over Scream because Scream's signature is problematic.)

**Coverage:** YES — all three OSes from one action. **Maintenance:** actively maintained; its own `test.yml` matrix is green on `ubuntu-22.04/24.04/26.04`, `windows-2022/2025`, `macos-14/15/26` as of 2026-06-22 ([test.yml](https://github.com/LABSN/sound-ci-helpers/blob/main/.github/workflows/test.yml), verifying a real output device via `python -m sounddevice`).

**Caveat for audio-graph's e2e needs:** LABSN guarantees "a device exists" (enough for enumeration + playback-into smokes). For a **capture-back assertion** (record what you played), the Linux path's Pulse-only setup needs a null-sink `.monitor` added, which it does not do. So: use LABSN as the baseline, and on Linux add a null-sink + monitor shim (§2). On Windows/macOS the LABSN devices already loop back.

### Other community / per-OS actions

| Action / pattern | OSes | What it sets up | Source |
|---|---|---|---|
| **LABSN/sound-ci-helpers** | Linux+macOS+Windows | Pulse / Background Music / VB-CABLE | above |
| AlekseyMartynov/action-vbcable-win | Windows | VB-CABLE composite action (ships cert + devcon) | <https://github.com/AlekseyMartynov/action-vbcable-win> |
| duncanthrax/scream (DIY) | Windows | self-signed Scream driver (render-only) | <https://github.com/duncanthrax/scream> |
| ExistentialAudio/BlackHole (DIY) | macOS | `brew install blackhole-2ch` | <https://github.com/ExistentialAudio/BlackHole> |
| Pulse null-sink inline | Linux | `module-null-sink` + `~/.asoundrc` | nircoe/soundcoe, FurryGoods/pvspeaker-esm (below) |

There is **no other maintained 3-OS marketplace action**; everything else is single-platform.

**How comparable projects test audio in CI:**
- **RustAudio/cpal** (our playback dep) — CI is **build/lint only; opens no audio devices**. ([quality.yml](https://github.com/RustAudio/cpal/blob/master/.github/workflows/quality.yml)). So audio-graph doing real-device CI is *ahead* of upstream cpal; LABSN is the closest community-blessed pattern.
- **nircoe/soundcoe** — canonical Pulse-null-sink recipe on `ubuntu-latest` ([ci-linux.yml](https://github.com/nircoe/soundcoe/blob/main/.github/workflows/ci-linux.yml)).
- **FurryGoods/pvspeaker-esm** (Picovoice) — *"GitHub Actions runners do not have sound cards, so a virtual one must be created"* → Pulse null-sink ([nodejs.yml](https://github.com/FurryGoods/pvspeaker-esm/blob/main/.github/workflows/nodejs.yml)).
- **diwic/alsa-rs** — has `modprobe snd-aloop` **commented out** in CI: direct evidence it can't be loaded on GitHub runners.
- **drowe67/freedv-gui** — production VB-CABLE silent install on Windows ([cmake-windows.yml](https://github.com/drowe67/freedv-gui/blob/master/.github/workflows/cmake-windows.yml)).
- **Taskcluster generic-worker** — does `modprobe snd-aloop` *only because it controls the worker image* ([loopback_audio_linux.go](https://github.com/taskcluster/taskcluster/blob/v64.3.0/workers/generic-worker/loopback_audio_linux.go)).

---

## 2. Linux

### Options

| Approach | GitHub-hosted | Blacksmith (custom image) | Root | Headless | rsac (PipeWire) sees it? | CPAL (ALSA) sees it? |
|---|---|---|---|---|---|---|
| **PipeWire null-sink + `.monitor`** ✅ recommended | ✅ userspace | ✅ (PPA already installed) | No (apt needs sudo) | ✅ (no seat; wants D-Bus session) | **✅ yes (required)** | via `pipewire`/`pulse` bridge PCM or cpal `pipewire` feature |
| PulseAudio null-sink + `.monitor` | ✅ userspace | ✅ | No | ✅ | ❌ (no PipeWire) | via ALSA `pulse` plugin |
| `snd-aloop` kernel loopback | ❌ `.ko` absent (Azure kernel) | ⚠️ only if image ships `snd-aloop.ko` (unverified) | Yes (modprobe) | ✅ | only if WirePlumber ALSA monitor adopts it | ✅ real card |
| `snd-dummy` | ❌ same | ⚠️ same | Yes | ✅ | rarely | ✅ card, but **no loopback** |

**Why PipeWire wins for audio-graph:** rsac captures via PipeWire, and CI already installs PipeWire dev libs from `ppa:pipewire-debian/pipewire-upstream` (ci.yml lines 73/160/222). Adding the runtime daemons (`pipewire`, `wireplumber`, `pipewire-pulse`) brings up a null-sink whose `.monitor` is a PipeWire node rsac enumerates **and** a Pulse source that CPAL's ALSA bridge can play into — one device, all consumers.

**Key facts** (sources):
- A null sink's `.monitor` is a real capture source — the standard "output→input" plumbing. ([freedesktop PulseAudio modules](https://www.freedesktop.org/wiki/Software/PulseAudio/Documentation/User/Modules/))
- PipeWire's pulse-compat means all `pactl` commands work unchanged. ([pipewire pulse module](https://pipewire.pages.freedesktop.org/pipewire/page_module_protocol_pulse.html))
- On non-systemd systems you launch the trio manually (`pipewire & wireplumber & pipewire-pulse &`); WirePlumber just must start after pipewire. ([WirePlumber running](https://pipewire.pages.freedesktop.org/wireplumber/daemon/running.html))
- A null-sink needs **no logind seat** (seats only matter for claiming real hardware), but it **wants a D-Bus session bus** — wrap in `dbus-run-session`. ([Void docs](https://docs.voidlinux.org/config/media/pipewire.html))
- CPAL: address the `pipewire`/`pulse` bridge PCM (or build cpal with the `pipewire` feature), **not** raw ALSA `default`, or you hit `DeviceBusy` because the sound server holds `default` exclusively. ([cpal README](https://github.com/RustAudio/cpal/blob/master/README.md))

### Linux CI snippet (Blacksmith ubuntu-2404; portable to GitHub ubuntu-24.04)

```yaml
- name: Install PipeWire runtime daemons
  run: |
    sudo apt-get update
    # dev libs already via ppa:pipewire-debian/pipewire-upstream; add daemons + tools
    sudo apt-get install -y pipewire pipewire-pulse wireplumber pipewire-alsa \
      pulseaudio-utils dbus-x11

- name: Start virtual audio + run live rsac smoke (PipeWire null-sink + monitor)
  working-directory: audio-graph/src-tauri
  run: |
    export XDG_RUNTIME_DIR="$(mktemp -d)"; chmod 700 "$XDG_RUNTIME_DIR"
    # Run daemons AND the test in ONE dbus session so the sink outlives the test:
    dbus-run-session -- bash -euc '
      pipewire & sleep 1
      wireplumber & pipewire-pulse & sleep 2
      pactl load-module module-null-sink sink_name=virtual_speaker \
            sink_properties=device.description=virtual_speaker
      pactl set-default-sink   virtual_speaker
      pactl set-default-source virtual_speaker.monitor
      pactl info; pactl list short sinks; pactl list short sources   # enumeration log
      xvfb-run -a cargo test -p audio-graph --no-default-features \
        --features cloud,live-audio-smoke rsac_live -- --nocapture --test-threads=1
    '
```

Do **not** add `modprobe snd-aloop` — it fails on GitHub-hosted runners and buys nothing for rsac's PipeWire capture on Blacksmith.

### Build vs adopt (Linux)
**Adopt — there is nothing to build.** A Linux loopback device is one `pactl load-module` (userspace) or one `modprobe` (kernel, in-tree `sound/drivers/aloop.c`). The only engineering is the daemon-launch/headless plumbing above.

---

## 3. macOS

### Options

| Tool | License | Driver type | Reboot? | Headless install? | Loopback (in+out)? | CI verdict |
|---|---|---|---|---|---|---|
| **BlackHole 2ch** ✅ | GPLv3 (free) | user-space **AudioServerPlugin / HAL plugin** (not a kext) | **No** — `sudo killall coreaudiod` | ✅ `brew install blackhole-2ch` (notarized .pkg, passwordless sudo) | ✅ yes | **Adopt** |
| Soundflower | MIT | **kext** | Yes + System Settings approval | ❌ "requires user interaction" | yes | Dead (2014), no Apple Silicon |
| Loopback (Rogue Amoeba) | Commercial (~$119) | HAL plugin | No | ❌ no headless/CLI path; paid | yes | Skip |
| ScreenCaptureKit / Core Audio process taps | Apple SDK | n/a | No | ❌ needs TCC Screen-Recording / audio prompt | output-capture only, not a selectable input device | Skip for e2e |

**Why BlackHole:** it is the *exact* AudioServerPlugin you'd otherwise build, given away free under GPLv3. It is **not a kext** — the maintainer built it specifically to replace kext-based Soundflower using the user-space `AudioServerPlugIn` API ([BlackHole#450](https://github.com/ExistentialAudio/BlackHole/issues/450)). It installs as a bundle in `/Library/Audio/Plug-Ins/HAL`, needs **no SIP change, no kext approval, no reboot** — just a `coreaudiod` restart ([README](https://github.com/ExistentialAudio/BlackHole)). It is a single device that is simultaneously playback target and capture source. GitHub trusts it enough to bake it into the macOS runner images ([#3526](https://github.com/actions/runner-images/issues/3526)).

**CI gotchas:**
- On macOS **14.4+**, `launchctl kickstart -k` for coreaudiod is deprecated and fails with `Operation not permitted` — use **`sudo killall coreaudiod`** instead ([cask issue](https://github.com/Homebrew/homebrew-cask/issues/171570), [Apple 14.4 notes](https://developer.apple.com/documentation/macos-release-notes/macos-14_4-release-notes#Core-Audio)).
- The Homebrew cask's `caveats { reboot }` is advisory, not enforced — the device registers after the daemon restart ([cask](https://formulae.brew.sh/cask/blackhole-2ch)).
- A **microphone TCC prompt** can hang an unattended capture on macos-14; pre-grant by inserting a `kTCCServiceMicrophone` row into `TCC.db` ([#9330](https://github.com/actions/runner-images/issues/9330)).
- **Kexts cannot be installed unattended on any macOS runner** (need approval + reboot; SIP not togglable in-job) — proven by macFUSE ([#4731](https://github.com/actions/runner-images/issues/4731)). This is *why* a HAL plugin is the only option, and it applies equally to Blacksmith macos-15.

### macOS CI snippet

```yaml
- name: Install BlackHole virtual audio device
  if: matrix.os == 'macos'
  run: |
    brew install blackhole-2ch        # cask; notarized .pkg via passwordless sudo
    sudo killall coreaudiod || true   # restart daemon (NOT kickstart -k on 14.4+)
    sleep 5
    system_profiler SPAudioDataType | grep -i blackhole   # confirm registered
- name: Live rsac smoke (macOS)
  if: matrix.os == 'macos'
  working-directory: audio-graph/src-tauri
  run: cargo test -p audio-graph --no-default-features --features cloud,live-audio-smoke rsac_live -- --nocapture --test-threads=1
```
Then in the test: CPAL plays out "BlackHole 2ch", rsac captures from "BlackHole 2ch".

### Build vs adopt (macOS)
**Adopt BlackHole.** Building an AudioServerPlugin is real driver work (weeks), and BlackHole *is* that driver, GPLv3/free, headless-installable, no reboot. Building your own buys nothing.

---

## 4. Windows

### Options

| Tool | License | Signed? | Silent install? | Reboot? | Capture endpoint? | CI verdict |
|---|---|---|---|---|---|---|
| **VB-CABLE** ✅ | Donationware (free single cable) | ✅ Authenticode | ✅ cert-pretrust + `-i -h` / `pnputil` | **No** | ✅ "CABLE Output" | **Adopt** |
| Scream | MS-PL | cert lapsed → self-sign in CI | ✅ (self-sign + devcon) | No (with cert trick) | ❌ render-only | Backup; weaker than VB-CABLE |
| Virtual Audio Cable (VAC, Muzychenko) | Paid ($30+); trial injects voice reminder | ✅ | no advantage | — | ✅ | Skip (poisons captured audio in trial) |
| WASAPI loopback (no driver) | n/a | n/a | n/a | n/a | requires an existing render endpoint | **Not sufficient alone** |
| SYSVAD / WDK build-your-own | sample | needs EV cert or testsigning+reboot | no | **reboot (testsigning)** | generates a tone, not real audio | Don't |

**WASAPI loopback is NOT a substitute on a bare runner.** `AUDCLNT_STREAMFLAGS_LOOPBACK` captures the mix rendered to a render endpoint — but with no audio hardware, `GetDefaultAudioEndpoint(eRender,…)` returns `ERROR_NOT_FOUND (0x80070490)` and the chain dies before init ([MS loopback doc](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording), [MS GetDefaultAudioEndpoint](https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-immdeviceenumerator-getdefaultaudioendpoint)). So a virtual render endpoint must be installed regardless — and VB-CABLE provides both render ("CABLE Input") and capture ("CABLE Output").

**VB-CABLE silent install IS possible on GitHub-hosted runners** (no reboot, no prompt) via the proven pattern: download → extract the publisher cert from the `.sys` → `Import-Certificate … Cert:\LocalMachine\TrustedPublisher` (suppresses the device-software dialog) → install via `-i -h` or `pnputil /add-driver … /install` → **`net start audiosrv`** → poll for the endpoint. Live workflows: [drowe67/freedv-gui](https://github.com/drowe67/freedv-gui/blob/master/.github/workflows/cmake-windows.yml), [AlekseyMartynov/action-vbcable-win](https://github.com/AlekseyMartynov/action-vbcable-win), LABSN. Loopback topology and signing confirmed: [VB-Audio Cable](https://vb-audio.com/Cable/VirtualCables.htm), [winget-pkgs PR#115361](https://github.com/microsoft/winget-pkgs/pull/115361).

> **Licensing flag:** the single VB-CABLE is donationware/free, but VB-Audio's [licensing page](https://vb-audio.com/Services/licensing.htm) expects a purchased license for professional/automated/server use. CI arguably qualifies — review before relying on it long-term, or fall back to Scream (MS-PL) if a strictly-FOSS path is required.

### Windows CI snippet

```yaml
- name: Install VB-CABLE virtual audio device
  if: matrix.os == 'windows'
  shell: pwsh
  run: |
    Invoke-WebRequest https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack45.zip -OutFile vbcable.zip
    Expand-Archive vbcable.zip -DestinationPath vbcable
    $cert = (Get-AuthenticodeSignature "vbcable\vbaudio_cable64_win10.sys").SignerCertificate
    Export-Certificate -Cert $cert -FilePath vbcable.cer | Out-Null
    Import-Certificate -FilePath vbcable.cer -CertStoreLocation Cert:\LocalMachine\TrustedPublisher | Out-Null
    Start-Process -Wait "vbcable\VBCABLE_Setup_x64.exe" -ArgumentList "-i","-h"
    Set-Service -Name audiosrv -StartupType Automatic; Restart-Service -Name audiosrv -Force
    $deadline=(Get-Date).AddSeconds(60)
    do { Start-Sleep 2; $d=(Get-CimInstance Win32_SoundDevice).Name } `
      until (($d -match "VB-Audio Virtual Cable") -or ((Get-Date) -gt $deadline))
- name: Live rsac smoke (Windows)
  if: matrix.os == 'windows'
  working-directory: audio-graph/src-tauri
  run: cargo test -p audio-graph --no-default-features --features cloud,live-audio-smoke rsac_live -- --nocapture --test-threads=1
```
Test: CPAL plays to "CABLE Input"; rsac captures "CABLE Output" (or system loopback of CABLE Input).

### Build vs adopt (Windows)
**Adopt VB-CABLE.** A WDK/SYSVAD driver costs weeks + an EV cert (~hundreds/yr) or `bcdedit /set testsigning on` + **reboot** (kills ephemeral runners), and SYSVAD only emits a tone, not your app's real audio. VB-CABLE sidesteps all of it.

---

## 5. Blacksmith vs GitHub-hosted summary

| OS | GitHub-hosted | Blacksmith (control the image) |
|---|---|---|
| Linux | PipeWire/Pulse userspace ✅ (kernel modules ❌) | same userspace path; PPA libs already installed; could pre-bake daemons |
| macOS | BlackHole ✅ (and preinstalled, but flaky) | BlackHole ✅ (install explicitly; can't pre-bake kexts but HAL plugin is fine) |
| Windows | VB-CABLE silent ✅ (proven) | VB-CABLE ✅; can **pre-bake cert + driver into image** so job step is just `pnputil` + `net start audiosrv` (optimization, not required) |

**Nothing is blocked on GitHub-hosted that needs self-hosting.** The only things that *require* a custom image are kernel `snd-aloop` (Linux — and unnecessary for rsac) and RDP-Remote-Audio / test-signed drivers on Windows (avoided by VB-CABLE). Blacksmith's advantage is pre-baking to reduce per-job time/flakiness.

---

## 6. Build-vs-adopt verdict (all OSes)

**Adopt on every OS.** No homegrown driver is warranted:
- **Linux:** virtual device = one command (`pactl load-module` / `modprobe`); the in-tree kernel module already exists. Build cost ≈ zero, so "adopt" is trivially correct.
- **macOS:** BlackHole *is* the AudioServerPlugin you'd build, free under GPLv3.
- **Windows:** a virtual driver is weeks of WDK work + signing + reboot; VB-CABLE is signed and silent-installable today.

---

## 7. How this plugs into seed 0d66

Seed 0d66 acceptance: *workflow_dispatch + scheduled jobs run live smokes; normal PRs keep lightweight checks; failures attach source/device enumeration logs.* Concretely:

1. **Gate it like the existing optional-feature smoke.** ci.yml already uses `if: github.event_name != 'pull_request'` for the Blacksmith optional-feature matrix (lines 280–282). Add a `live-audio-smoke` job with the same gate plus `workflow_dispatch`/`schedule` triggers so PRs stay lightweight (PRs keep only the current synthetic `playback` resampling test under `xvfb-run`).
2. **Feature-gate the test.** Add a `live-audio-smoke` Cargo feature so the round-trip test (`rsac_live`) compiles only when explicitly enabled — keeps default/cloud CI free of device assumptions.
3. **Per-OS device setup** = the three snippets above (Linux PipeWire null-sink, macOS BlackHole, Windows VB-CABLE). This satisfies 0d66's "Linux PipeWire dummy sink/source, Windows VB-CABLE or loopback fixture, macOS BlackHole/native loopback."
4. **The round-trip test** should: enumerate sources via rsac (`get_device_enumerator()`), feed known PCM into the virtual device (play via CPAL), capture it back via rsac (default/device/system targets), and assert the captured PCM matches (correlation / RMS / known-tone FFT bin). This also exercises the `CaptureTarget` parse/build/start path that f166 and f3ff (the Windows MMDevice ID round-trip bug) need regression coverage for.
5. **Attach enumeration logs on failure.** Each snippet already dumps device enumeration (`pactl list short`, `system_profiler SPAudioDataType`, `Get-CimInstance Win32_SoundDevice`); pipe these to a file and `actions/upload-artifact` on failure, satisfying "failures attach source/device enumeration logs."
6. **Optional fast-start:** front the whole thing with `LABSN/sound-ci-helpers@v1` to establish "a device exists," then layer the Linux null-sink `.monitor` shim for guaranteed capture-back. This reduces maintenance surface (LABSN owns the Windows cert/driver dance and macOS coreaudiod restart).

This unblocks 0d66 → which unblocks f166 (capture source round-trip tests), 09a7 (release usability runbooks), and the c395 release-readiness epic.

---

## Sources

**Project/runner baseline:** [rsac README](https://github.com/Codeseys-Labs/rust-crossplat-audio-capture/blob/master/README.md) · [runner-images#13610](https://github.com/actions/runner-images/issues/13610) · [#8295](https://github.com/actions/runner-images/issues/8295) · [#1114](https://github.com/actions/runner-images/issues/1114) · [#6983](https://github.com/actions/runner-images/issues/6983) · [#2528](https://github.com/actions/runner-images/issues/2528) · [#3526](https://github.com/actions/runner-images/issues/3526) · [#3542](https://github.com/actions/runner-images/pull/3542/files) · [#13668](https://github.com/actions/runner-images/issues/13668) · [#9330](https://github.com/actions/runner-images/issues/9330) · [#4731](https://github.com/actions/runner-images/issues/4731) · [Azure/AKS#2335](https://github.com/Azure/AKS/issues/2335)
**LABSN/community:** [LABSN/sound-ci-helpers](https://github.com/LABSN/sound-ci-helpers) ([action.yml](https://github.com/LABSN/sound-ci-helpers/blob/main/action.yml), [linux](https://github.com/LABSN/sound-ci-helpers/blob/main/linux/setup_sound.sh), [macos](https://github.com/LABSN/sound-ci-helpers/blob/main/macos/setup_sound.sh), [windows](https://github.com/LABSN/sound-ci-helpers/blob/main/windows/setup_sound.ps1), [test.yml](https://github.com/LABSN/sound-ci-helpers/blob/main/.github/workflows/test.yml)) · [AlekseyMartynov/action-vbcable-win](https://github.com/AlekseyMartynov/action-vbcable-win) · [nircoe/soundcoe](https://github.com/nircoe/soundcoe/blob/main/.github/workflows/ci-linux.yml) · [FurryGoods/pvspeaker-esm](https://github.com/FurryGoods/pvspeaker-esm/blob/main/.github/workflows/nodejs.yml) · [RustAudio/cpal quality.yml](https://github.com/RustAudio/cpal/blob/master/.github/workflows/quality.yml) · [taskcluster loopback_audio_linux.go](https://github.com/taskcluster/taskcluster/blob/v64.3.0/workers/generic-worker/loopback_audio_linux.go)
**Linux:** [PulseAudio modules](https://www.freedesktop.org/wiki/Software/PulseAudio/Documentation/User/Modules/) · [PipeWire pulse module](https://pipewire.pages.freedesktop.org/pipewire/page_module_protocol_pulse.html) · [WirePlumber running](https://pipewire.pages.freedesktop.org/wireplumber/daemon/running.html) · [Void PipeWire](https://docs.voidlinux.org/config/media/pipewire.html) · [cpal README](https://github.com/RustAudio/cpal/blob/master/README.md)
**macOS:** [BlackHole](https://github.com/ExistentialAudio/BlackHole) · [BlackHole#450 (HAL not kext)](https://github.com/ExistentialAudio/BlackHole/issues/450) · [brew cask](https://formulae.brew.sh/cask/blackhole-2ch) · [cask coreaudiod issue](https://github.com/Homebrew/homebrew-cask/issues/171570) · [macOS 14.4 Core Audio notes](https://developer.apple.com/documentation/macos-release-notes/macos-14_4-release-notes#Core-Audio) · [Apple kext deprecation](https://developer.apple.com/support/kernel-extensions/)
**Windows:** [VB-Audio Cable](https://vb-audio.com/Cable/) · [VB-Audio licensing](https://vb-audio.com/Services/licensing.htm) · [VirtualCables topology](https://vb-audio.com/Cable/VirtualCables.htm) · [winget-pkgs#115361 (signing)](https://github.com/microsoft/winget-pkgs/pull/115361) · [drowe67/freedv-gui](https://github.com/drowe67/freedv-gui/blob/master/.github/workflows/cmake-windows.yml) · [MS WASAPI loopback](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording) · [MS GetDefaultAudioEndpoint](https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-immdeviceenumerator-getdefaultaudioendpoint) · [Scream](https://github.com/duncanthrax/scream) · [MS SYSVAD sample](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/sample-audio-drivers)
