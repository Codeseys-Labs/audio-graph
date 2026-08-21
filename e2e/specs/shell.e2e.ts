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

// `e2e/tsconfig.json` deliberately omits the `DOM` lib (this file runs under
// Node, not a browser) so `window`/`document` are otherwise unresolvable
// identifiers to tsc. Several `browser.execute()` callbacks below reference
// them anyway -- those callback bodies are serialized and actually run
// INSIDE the webview, where both genuinely exist -- so they need a minimal
// ambient declaration rather than pulling in the full (and here, unwanted)
// `DOM` lib.
declare const window: unknown;
declare const document: {
  getElementById(id: string): { focus(): void } | null;
};

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

/**
 * Bridges `@wdio/tauri-plugin`'s mock registry (`window.__wdio_mocks__`,
 * populated by `browser.tauri.mock()`/`.mockReturnValue()`/etc. — that queue
 * machinery itself works fine) into the IPC transport this app's Tauri build
 * ACTUALLY uses.
 *
 * `@wdio/tauri-plugin` intercepts by `Object.defineProperty`-patching
 * `window.__TAURI__.core.invoke`. That never fires here: real app code never
 * calls `window.__TAURI__.core.invoke` — it imports `invoke` from
 * `@tauri-apps/api/core`, whose bundled implementation calls
 * `window.__TAURI_INTERNALS__.invoke` directly (see `@tauri-apps/api/core`'s
 * `core.js`). Tauri's own init script defines every
 * `window.__TAURI_INTERNALS__` member (`invoke`, `postMessage`, `ipc`, ...)
 * via bare `Object.defineProperty(..., { value })` calls with no
 * `configurable`/`writable` flags, which default to `false` — i.e. Tauri
 * itself permanently freezes that whole chain, by design, specifically so
 * page content can't hijack the native IPC bridge. Re-`defineProperty`-ing or
 * reassigning `window.__TAURI__.core.invoke` (what the plugin attempts) or
 * `window.__TAURI_INTERNALS__.invoke` throws/no-ops either way — confirmed
 * locally: the plugin logs "Invoke interception via defineProperty failed"
 * on every run, and a direct probe against `__TAURI_INTERNALS__.invoke`
 * throws "Attempting to change configurable attribute of unconfigurable
 * property." Mocks registered via `browser.tauri.mock()` are therefore
 * silently inert for every real command the frontend invokes — this is a
 * genuine `@wdio/tauri-plugin@1.3.0` limitation against Tauri v2's Brownfield
 * IPC pattern, not a payload-shape or ordering bug.
 *
 * The one link in that chain that ISN'T frozen: on non-Android platforms
 * (this suite only runs on Linux/Windows), Tauri's real invoke path first
 * tries a `fetch()` to a synthetic `ipc://localhost/<cmd>` URL
 * (`convertFileSrc(cmd, 'ipc')` in `@tauri-apps/api`'s bundled `core.js`)
 * before ever falling back to the frozen `postMessage` binding — and
 * `window.fetch` is an ordinary, fully mutable global. Patching `fetch` to
 * serve a synthetic `Response` for URLs matching a currently-registered mock
 * (falling through to the real `fetch` for everything else) reaches the
 * exact point in the transport this build actually uses, while still
 * reusing the plugin's own (correctly implemented) mock queue/once
 * semantics unchanged.
 */
async function installIpcMockBridge(): Promise<void> {
  await browser.execute(() => {
    const w = window as {
      __wdio_mocks__?: Record<string, (args: unknown) => unknown>;
      __wdio_e2e_fetch_bridged__?: boolean;
      fetch: typeof fetch;
    };
    if (w.__wdio_e2e_fetch_bridged__) return;
    w.__wdio_e2e_fetch_bridged__ = true;

    const originalFetch = w.fetch.bind(w);
    w.fetch = async (input, init) => {
      const url =
        typeof input === "string"
          ? input
          : input instanceof URL
            ? input.href
            : input.url;
      const match =
        /^(?:ipc|https?):\/\/(?:localhost|ipc\.localhost)\/([^/?#]+)/.exec(url);
      const mockFn = match && w.__wdio_mocks__?.[decodeURIComponent(match[1])];
      if (!mockFn) return originalFetch(input, init);

      try {
        const value = await mockFn(undefined);
        return new Response(JSON.stringify(value ?? null), {
          status: 200,
          headers: {
            "Content-Type": "application/json",
            "Tauri-Response": "ok",
          },
        });
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        return new Response(JSON.stringify(message), {
          status: 200,
          headers: {
            "Content-Type": "application/json",
            "Tauri-Response": "error",
          },
        });
      }
    };
  });
}

describe("AudioGraph desktop shell (embedded WebdriverIO/Tauri E2E)", () => {
  before(async () => {
    await installIpcMockBridge();
  });

  // ── 1. Launch + title/selector ──────────────────────────────────────────
  // Proves React mounted, i18n resolved, and first paint completed inside the
  // real WebView — not jsdom. Selector rewritten SHELL-R4 (plan §R4,
  // ADR-0046): `#workspace-tab-during` → `#workspace-tab-capture` (the
  // during/after/analysis three-tab shell is deleted outright in favor of
  // the two Capture/Sessions destinations).
  it("mounts the real shell with the expected window title and first tab", async () => {
    await expect(browser).toHaveTitle(APP_TITLE);
    await expect($("#workspace-tab-capture")).toBeDisplayed();
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

  // ── 3. Navigation across the shell's main destinations ──────────────────
  // Mirrors handleWorkspaceViewKeyDown's arrow-key contract (via wraparound,
  // since this driver doesn't deliver Home/End -- see comment below) as well
  // as plain clicks. Rewritten SHELL-R4 (plan §R4, ADR-0046): the
  // during/after/analysis three-tab shell is deleted outright in favor of
  // the two Capture/Sessions destinations -- every occupant of the deleted
  // `analysis` tab already has a home (Graph/Route lenses on the Sessions
  // destination, R2; diagnostics in the NowStrip System drawer, R3).
  it("navigates capture -> sessions via click and keyboard, toggling aria-selected", async () => {
    const views = ["capture", "sessions"] as const;

    for (const view of views) {
      const tab = await $(`#workspace-tab-${view}`);
      await tab.click();
      await expect(tab).toHaveAttribute("aria-selected", "true");
      await expect($(`#workspace-panel-${view}`)).toBeDisplayed();
    }

    // WebKitGTK (unlike Chromium) does not move keyboard focus to a <button>
    // on click -- a longstanding, spec-legal WebKit quirk, not an app bug
    // (App.tsx's tabs already carry the correct roving tabindex: only the
    // selected tab has tabIndex 0). A real keyboard-only user reaches the
    // tablist via Tab, landing on whichever tab currently has tabIndex 0 (the
    // one just selected above, "sessions"); emulate that explicitly so the
    // arrow-key presses below land on `handleWorkspaceViewKeyDown` instead of
    // nothing.
    await browser.execute(() => {
      document.getElementById("workspace-tab-sessions")?.focus();
    });

    // `Home`/`End` are silently dropped by this embedded WebKitGTK provider
    // (tauri-plugin-wdio-webdriver's key injection) -- confirmed locally:
    // neither the named keys "Home"/"End" nor their raw W3C codepoints
    // (U+E010/U+E011) produce any DOM effect even once focus is verified to
    // be on the target tab, while ArrowLeft/ArrowRight reliably reach the
    // handler and move focus via its own `tabs?.[nextIndex]?.focus()` call.
    // Exercise the identical `handleWorkspaceViewKeyDown` switch (Home/End
    // and the arrows share one function; only the target index differs) via
    // arrow-key wraparound instead, which this driver actually delivers --
    // the 2-tab equivalent of the old 3-tab wraparound block: one ArrowLeft
    // from "sessions" (index 1) lands on "capture" (index 0) -- the same
    // destination Home would reach; a second ArrowLeft wraps back around to
    // "sessions" -- the same destination End would reach with only two tabs.
    await browser.keys(["ArrowLeft"]);
    await expect($("#workspace-tab-capture")).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await browser.keys(["ArrowLeft"]);
    await expect($("#workspace-tab-sessions")).toHaveAttribute(
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

  // ── 6. Session/app health through the end of the suite ─────────────────
  // The promotion-gate requirement this seed cites ("cleanup is verified; no
  // lingering process") is a property of `@wdio/tauri-service`'s own
  // launcher-level `onComplete()` hook, enforced by the
  // ".github/workflows/tauri-shell-e2e.yml" step that runs immediately after
  // `npx wdio run` exits -- see that workflow for the actual absence check.
  //
  // It is NOT something this (or any) `it()`/`after()` in this file can
  // observe: `onComplete()` runs in a separate launcher process, strictly
  // AFTER this entire spec file returns, specifically so the same app
  // process can be reused across sessions within a run -- confirmed locally
  // by reading its source and by probing `browser.deleteSession()` (a pure
  // in-memory session-table removal on the Rust side, no window close, no
  // process exit) from inside a test, which does not hasten that hook and
  // additionally collides with `@wdio/runner`'s own redundant end-of-file
  // `deleteSession()` call, turning an otherwise-clean run into "Failed
  // launching test session" from `@wdio/local-runner`.
  //
  // What this test verifies instead, honestly matching its title: the
  // embedded session and its app process are both still alive and healthy
  // at the end of the run (i.e. nothing crashed mid-suite) -- a real check,
  // just not the absence one.
  it("keeps the embedded session and app process alive and healthy through the end of the suite", async () => {
    const settings = await browser.tauri.execute(({ core }) =>
      core.invoke("load_settings_cmd"),
    );
    expect(settings).toBeDefined();

    const { execFileSync } = await import("node:child_process");
    const resolvedBinaryPath =
      process.env.APP_BINARY_PATH ?? "../src-tauri/target/release/audio-graph";
    if (process.platform === "win32") {
      const out = execFileSync(
        "tasklist",
        ["/FI", "IMAGENAME eq audio-graph.exe", "/FO", "CSV"],
        { encoding: "utf-8" },
      );
      expect(out).toContain("audio-graph.exe");
    } else {
      // Anchor on the exact resolved binary path, `$`-terminated: a bare
      // basename ("audio-graph") would also match this very test-runner
      // process's own cmdline, since the repo directory is itself named
      // "audio-graph" (e.g. `.../audio-graph/node_modules/.bin/wdio run
      // e2e/wdio.conf.ts`). The embedded provider spawns the app with no
      // extra args, so its cmdline is the resolved binary path verbatim.
      const escaped = resolvedBinaryPath.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      const out = execFileSync("pgrep", ["-f", `${escaped}$`], {
        encoding: "utf-8",
      });
      expect(out.trim()).not.toBe("");
    }
  });
});
