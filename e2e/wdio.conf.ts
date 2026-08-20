/**
 * Portable desktop shell E2E (seed audio-graph-f9e0, Wave 3).
 *
 * Drives the REAL compiled Tauri binary via `@wdio/tauri-service`'s embedded
 * WebDriver provider — no jsdom, no external `tauri-driver`/`webkit2gtk-driver`
 * process. The provider needs `tauri-plugin-wdio` + `tauri-plugin-wdio-webdriver`
 * registered inside the binary, which only happens when the app was built with
 * the `wdio-e2e` Cargo feature (see `src-tauri/Cargo.toml` and `src-tauri/src/lib.rs`)
 * and the `src-tauri/tauri.e2e.conf.json` capability overlay.
 *
 * `tauri-shell-e2e.yml` builds that exact artifact once per OS and passes its
 * path via `APP_BINARY_PATH`. Locally, running `bun run tauri build --no-bundle
 * --no-default-features --features cloud,wdio-e2e --config
 * src-tauri/tauri.e2e.conf.json` (with `VITE_WDIO_E2E=1` at frontend-build
 * time) produces the same binary at the fallback path below, so `bun run
 * test:e2e` works without any env wiring.
 */
import type { TauriCapabilities } from "@wdio/tauri-service";

const appBinaryPath =
  process.env.APP_BINARY_PATH ?? "../src-tauri/target/release/audio-graph";

// `WebdriverIO.Capabilities` (the globally-merged interface) only picks up
// `'wdio:tauriServiceOptions'`, not `'tauri:options'` — that field lives on
// the separately-exported `TauriCapabilities` type, so this needs its own
// explicit annotation rather than an inline literal in `config.capabilities`.
const tauriCapabilities: TauriCapabilities = {
  browserName: "tauri",
  "tauri:options": { application: appBinaryPath },
};

export const config: WebdriverIO.Config = {
  runner: "local",
  // Resolved relative to THIS config file's own directory (`e2e/`), not the
  // process cwd -- @wdio/config's ConfigParser sets `rootDir =
  // path.dirname(configFilePath)` and globs specs with that as `cwd`. A
  // leading `./e2e/...` here would glob `e2e/e2e/specs/**`, match nothing,
  // and make the whole suite exit 1 with zero tests run.
  specs: ["./specs/**/*.e2e.ts"],
  maxInstances: 1,

  services: [
    [
      "@wdio/tauri-service",
      {
        appBinaryPath,
        // Explicit rather than relying on auto-detection (which only
        // defaults to 'embedded' on macOS / when TAURI_WEBDRIVER_PORT is
        // set) — a future @wdio/tauri-service major bump must not silently
        // change which provider Linux/Windows resolve to.
        driverProvider: "embedded",
        // The embedded provider's own default is already 60s; stated
        // explicitly for the same reason.
        startTimeout: 60000,
        captureBackendLogs: true,
        captureFrontendLogs: true,
        backendLogLevel: "warn",
        frontendLogLevel: "warn",
        clearMocks: true,
        // Only honoured by the service's STANDALONE `init()` entry point,
        // never by the WDIO testrunner path (this config). Kept for anyone
        // who runs the service standalone; `outputDir` below is what
        // actually controls the log directory under `wdio run`/`bun run
        // test:e2e` -- @wdio/tauri-service's launcher does
        // `_config.outputDir || join(process.cwd(), 'logs')`, ignoring this
        // field entirely.
        logDir: "./e2e/logs",
      },
    ],
  ],

  capabilities: [tauriCapabilities],

  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: {
    ui: "bdd",
    timeout: 60000,
  },

  // Backend + frontend log capture (captureBackendLogs/captureFrontendLogs
  // above) writes here via @wdio/tauri-service's launcher, which reads
  // `outputDir` from the WDIO config, NOT `serviceOptions.logDir`. Without
  // this, logs silently land in `<cwd>/logs` and both the secret-shape scan
  // in shell.e2e.ts and the workflow's `e2e/logs/**` artifact upload see an
  // empty directory.
  outputDir: "./e2e/logs",

  logLevel: "warn",
  bail: 0,
  waitforTimeout: 10000,
  connectionRetryTimeout: 90000,
  connectionRetryCount: 3,
};
