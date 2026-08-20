/**
 * Portable desktop shell E2E (seed audio-graph-f9e0, Wave 3).
 *
 * Drives the REAL compiled AudioGraph binary through `@wdio/tauri-service`'s
 * embedded WebDriver provider — real WebKitGTK/WebView2/WKWebView, real Tauri
 * IPC bridge, real React mount. Native audio commands are mocked via
 * `browser.tauri.mock()` so this suite never opens a real audio device or
 * depends on CI runner audio hardware.
 *
 * CI-only: this spec only runs against a binary built with the `wdio-e2e`
 * Cargo feature + the `tauri.e2e.conf.json` capability overlay (see
 * `e2e/wdio.conf.ts`'s doc comment). It is never part of `bun run test`
 * (Vitest) or a shipped build.
 */

const APP_TITLE = "AudioGraph";

// Mirrors the provider-key SHAPES in scripts/check-docs-secret-hygiene.mjs's
// `keyRules` (kept in sync by hand — this is a runtime log scan, not a repo
// text scan, so it can't `import` that CLI script directly).
const SECRET_SHAPE_PATTERNS: RegExp[] = [
  /\bsk-(?:proj-|or-|ant-)?[A-Za-z0-9_-]{3,}\b/,
  /\bAIza[0-9A-Za-z_-]{8,}\b/,
  /\b(?:AKIA|ASIA)[0-9A-Z]{12,}\b/,
  /\bdg[_-][A-Za-z0-9_-]{6,}\b/,
  /\btvly-[A-Za-z0-9_-]{6,}\b/,
  /\bxox[baprs]-[A-Za-z0-9-]{8,}\b/,
];

function assertNoSecretShapes(haystack: string, label: string): void {
  for (const pattern of SECRET_SHAPE_PATTERNS) {
    const match = pattern.exec(haystack);
    if (match) {
      throw new Error(
        `${label} contained a provider-key-shaped string matching ${pattern}: "${match[0].slice(0, 6)}…"`,
      );
    }
  }
}

// Matches `e2e/wdio.conf.ts`'s top-level `outputDir` (NOT the tauri-service
// option `logDir`, which the WDIO testrunner path ignores).
const LOG_DIR = "./e2e/logs";

/**
 * Reads every `.log` file the log-capture writer produced this run. Backend
 * and frontend lines share the SAME file (there is no per-source split), so
 * one read covers both the secret-shape scan and the frontend-error check
 * below.
 */
async function readCapturedLogLines(): Promise<string[]> {
  const { readdirSync, readFileSync } = await import("node:fs");
  const { join } = await import("node:path");
  let files: string[] = [];
  try {
    files = readdirSync(LOG_DIR).filter((name) => name.endsWith(".log"));
  } catch {
    files = [];
  }
  const lines: string[] = [];
  for (const file of files) {
    const content = readFileSync(join(LOG_DIR, file), "utf-8");
    lines.push(...content.split("\n").filter((line) => line.trim().length > 0));
  }
  return lines;
}

/**
 * `@wdio/tauri-service`'s log line parser tags frontend-sourced lines with
 * one of these markers (`extractPrefixAndSource` in its bundled dist) before
 * writing them to the shared log file. Mirrored here rather than imported
 * since they are internal, unexported constants of that package.
 */
function isFrontendLogLine(line: string): boolean {
  return (
    line.includes("[Tauri:Frontend]") ||
    /\[frontend\]/i.test(line) ||
    line.includes("[WDIO-FRONTEND]")
  );
}

/**
 * Mirrors `@wdio/native-core`'s own level-detection heuristic
 * (`LEVEL_SCAN_WINDOW = 60`, pattern `\b(ERROR|Error|error)\b`): the level
 * token sits in the line's prefix, before message-body text could
 * false-match the word.
 */
function hasErrorLevelToken(line: string): boolean {
  return /\bERROR\b/i.test(line.slice(0, 60));
}

// Minimal valid AudioSourceInfo (src/generated/audioSource.ts): only `id`,
// `name`, `source_type`, and `is_active` are required: everything else here
// is filled in so the real AudioSourceSelector row (which reads
// `capabilities`/`device_kind` to decide enabled/disabled state) renders as a
// normal, selectable row instead of falling back to an "unsupported" one.
const MOCK_SOURCE = {
  id: "e2e-mock-mic",
  name: "E2E Mock Microphone",
  source_type: { type: "SystemDefault" as const },
  is_active: false,
  device_kind: "Input" as const,
  capabilities: {
    backend_name: "e2e-mock",
    capture_supported: true,
    supports_system_capture: true,
    supports_application_capture: false,
    supports_process_tree_capture: false,
    supports_device_selection: false,
    supports_device_change_notifications: false,
  },
};

describe("AudioGraph desktop shell (embedded WebdriverIO/Tauri E2E)", () => {
  // ── 1. Launch + title/selector ──────────────────────────────────────────
  // Proves React mounted, i18n resolved, and first paint completed inside the
  // real WebView — not jsdom.
  it("mounts the real shell with the expected window title and first tab", async () => {
    await expect(browser).toHaveTitle(APP_TITLE);
    await expect($("#workspace-tab-during")).toBeDisplayed();
  });

  // ── 2. Renderer-to-Rust IPC round trip (settings) ───────────────────────
  // `load_settings_cmd` is disk-only, synchronous, and zero native-audio
  // dependency — the same command App.tsx's mount-time `runCredentialProbe()`
  // already calls first. Proves the real Tauri IPC bridge round-trips a
  // command end-to-end.
  it("round-trips load_settings_cmd through the real Tauri IPC bridge", async () => {
    const settings = await browser.tauri.execute(({ core }) =>
      core.invoke("load_settings_cmd"),
    );

    expect(settings).toBeDefined();
    expect(settings).toHaveProperty("asr_provider");
    expect(settings).toHaveProperty("llm_provider");

    // redacted_settings() must have stripped any live credential material
    // before this ever reached the renderer.
    assertNoSecretShapes(
      JSON.stringify(settings),
      "load_settings_cmd response",
    );
  });

  // ── 3. Navigation across the shell's main views ─────────────────────────
  // Mirrors handleWorkspaceViewKeyDown's Home/End/arrow contract as well as
  // plain clicks.
  it("navigates during -> after -> analysis via click and keyboard, toggling aria-selected", async () => {
    const views = ["during", "after", "analysis"] as const;

    for (const view of views) {
      const tab = await $(`#workspace-tab-${view}`);
      await tab.click();
      await expect(tab).toHaveAttribute("aria-selected", "true");
      await expect($(`#workspace-panel-${view}`)).toBeDisplayed();
    }

    // End -> last tab (analysis), Home -> first tab (during).
    await browser.keys(["End"]);
    await expect($("#workspace-tab-analysis")).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await browser.keys(["Home"]);
    await expect($("#workspace-tab-during")).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  // ── 4. Capture ready/error/stop UX with mocked native audio ─────────────
  // The suite's only claim about "capture" — entirely hardware-free.
  it("flips live state on a mocked start_capture success and surfaces a toast (not a hang) on a mocked rejection", async () => {
    const listSourcesMock = await browser.tauri.mock("list_audio_sources");
    await listSourcesMock.mockReturnValue([MOCK_SOURCE]);

    const startMock = await browser.tauri.mock("start_capture");
    await startMock.mockResolvedValueOnce(null);
    await startMock.mockRejectedValueOnce(
      new Error("e2e-mocked-capture-failure"),
    );

    const stopMock = await browser.tauri.mock("stop_capture");
    await stopMock.mockResolvedValue(null);

    // `AudioSourceSelector` lives in the always-mounted `<aside>` outside the
    // `workspaceView`-switched `<main>` subtree (App.tsx), so hopping tabs
    // does NOT remount it and its mount-only `fetchSources()` effect never
    // re-runs. Drive its own Refresh control instead, which must run AFTER
    // the mocks above are in place, not reuse whatever it fetched before this
    // test registered them.
    await (await $('button[aria-label="Refresh sources"]')).click();

    let sourceRow: WebdriverIO.Element | undefined;
    await browser.waitUntil(
      async () => {
        const rows = await $$('[role="checkbox"]');
        for (const row of rows) {
          if ((await row.getText()).includes(MOCK_SOURCE.name)) {
            sourceRow = row;
            return true;
          }
        }
        return false;
      },
      {
        timeout: 15000,
        timeoutMsg: `expected a source row for "${MOCK_SOURCE.name}" after mocking list_audio_sources`,
      },
    );
    await sourceRow?.click();

    const startButton = await $('button[aria-label="Start"]');
    await startButton.click();

    // First start_capture call resolves -> isCapturing flips true ->
    // `.workspace-switcher__state` narrates the live session (en.json:
    // workspace.stateLive = "Live session").
    await expect($(".workspace-switcher__state")).toHaveText(
      expect.stringContaining("Live session"),
    );

    const stopButton = await $('button[aria-label="Stop"]');
    await stopButton.click();
    await expect($(".workspace-switcher__state")).not.toHaveText(
      expect.stringContaining("Live session"),
    );

    // Second start_capture call (mocked rejection) must surface the error via
    // the Notifications host, not hang the renderer. This rejection flows
    // through the legacy `error` string bridge (store/index.ts's
    // `startCapture` catch -> `set({ error })`), which Notifications.tsx
    // renders as `.notification--error` > `.notification__body` >
    // `HumanizedError` -- NOT into the `notifications[]` queue, so
    // `.notification__message` (only used by queued items) never appears
    // here and would either time out or false-pass on an unrelated queued
    // notification.
    await startButton.click();
    await expect($(".notifications .notification--error")).toBeDisplayed();
  });

  // ── 5. Error-free console + no secret-shaped strings in captured logs ──
  it("captured zero error-level frontend logs and no secret-shaped strings in captured logs", async () => {
    // `browser.getLogs("browser")` is a Chromium-only WebDriver protocol
    // command (`@wdio/protocols`' `chromium_default` -> `POST
    // /session/:sessionId/se/log`). The embedded provider's WebDriver server
    // implements no such endpoint, and `webdriver` only attaches that
    // protocol at all when the session's browserName resolves to
    // chrome/msedge -- on Linux the embedded provider reports `WebKitGTK`, so
    // `browser.getLogs` isn't even a function there. The only source of
    // frontend console output this provider exposes is the shared
    // log-capture file that `captureFrontendLogs`/`captureBackendLogs`
    // (wdio.conf.ts) write to.
    const lines = await readCapturedLogLines();

    const frontendErrorLines = lines.filter(
      (line) => isFrontendLogLine(line) && hasErrorLevelToken(line),
    );
    expect(frontendErrorLines).toEqual([]);

    // Best-effort: scan whatever is on disk so far this run — this is a
    // log-hygiene check, not a completeness check of every line ever
    // emitted. Covers both backend and frontend lines since they share one
    // file.
    for (const line of lines) {
      assertNoSecretShapes(line, "captured log line");
    }
  });

  // ── 6. Clean teardown ───────────────────────────────────────────────────
  // Mirrors the promotion-gate requirement: "cleanup is verified; no
  // lingering process." Ends the embedded-provider session (which must tear
  // down the spawned binary) and then checks the OS process table.
  it("tears down the embedded session and leaves no lingering audio-graph process", async () => {
    try {
      await browser.deleteSession();
    } catch {
      // The service's own after-suite teardown may already be racing this;
      // either way the assertion below is what actually matters.
    }

    const { execFileSync } = await import("node:child_process");
    // Give the OS a brief moment to reap the process after the WebDriver
    // session (and the binary it spawned) end.
    await new Promise((resolve) => setTimeout(resolve, 2000));

    if (process.platform === "win32") {
      // `IMAGENAME eq` is an exact match on the process image name (not a
      // command-line substring search), so a bare basename is already safe
      // here — this branch was never the vacuous one.
      const out = execFileSync(
        "tasklist",
        ["/FI", "IMAGENAME eq audio-graph.exe", "/FO", "CSV"],
        { encoding: "utf-8" },
      );
      expect(out).not.toContain("audio-graph.exe");
    } else {
      // Match the exact binary this suite launched -- `target/(release|
      // debug)/audio-graph` is not a substring of the CI-downloaded artifact
      // path (`APP_BINARY_PATH=$GITHUB_WORKSPACE/dist-bin/audio-graph`, see
      // tauri-shell-e2e.yml), so that old pattern could never match the real
      // process and this assertion passed vacuously even with a genuine
      // leak. A bare basename ("audio-graph") isn't safe either: the repo
      // directory is itself named "audio-graph", so THIS test-runner
      // process's own cmdline (e.g. `.../audio-graph/node_modules/.bin/wdio
      // run e2e/wdio.conf.ts`) would also match it. The embedded provider
      // spawns the app binary with no extra args (`spawnTauriApp`'s `args`
      // defaults to `[]`), so its cmdline is the resolved binary path
      // verbatim -- anchor on that exact string instead.
      const resolvedBinaryPath =
        process.env.APP_BINARY_PATH ??
        "../src-tauri/target/release/audio-graph";
      const escaped = resolvedBinaryPath.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      let out = "";
      try {
        out = execFileSync("pgrep", ["-f", `${escaped}$`], {
          encoding: "utf-8",
        });
      } catch {
        // pgrep exits nonzero (and prints nothing) when no process matches —
        // that is the passing case.
        out = "";
      }
      expect(out.trim()).toBe("");
    }
  });
});
