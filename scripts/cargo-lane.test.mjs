import { afterEach, describe, expect, test } from "bun:test";
import { spawn } from "node:child_process";
import {
  access,
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  realpath,
  rm,
  utimes,
  writeFile,
} from "node:fs/promises";
import { availableParallelism, tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const facadePath = join(repositoryRoot, "scripts", "cargo-lane.mjs");
const fixtures = new Set();
const simulatedSixCpuHost = {
  AUDIO_GRAPH_CARGO_LANE_TEST_MODE: "1",
  AUDIO_GRAPH_CARGO_LANE_TEST_DETECTED_CPUS: "6",
};

afterEach(async () => {
  await Promise.all(
    [...fixtures].map((fixture) =>
      rm(fixture, { recursive: true, force: true }),
    ),
  );
  fixtures.clear();
});

async function makeFixture() {
  const root = await mkdtemp(join(tmpdir(), "audio-graph-cargo-lane-test-"));
  fixtures.add(root);
  await mkdir(join(root, "src-tauri"), { recursive: true });
  await mkdir(join(root, "temp"), { recursive: true });
  await writeFile(join(root, "src-tauri", "Cargo.toml"), "[workspace]\n");

  const fakeCargo = join(root, "fake-cargo.mjs");
  await writeFile(
    fakeCargo,
    `import { spawn } from "node:child_process";
import { access, readFile, writeFile } from "node:fs/promises";
const sleep = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
const record = {
  argv: process.argv.slice(2),
  cwd: process.cwd(),
  targetDir: process.env.CARGO_TARGET_DIR,
};
if (process.env.AUDIO_GRAPH_FAKE_CARGO_OWNER_PATH) {
  record.ownerAtStart = JSON.parse(
    await readFile(process.env.AUDIO_GRAPH_FAKE_CARGO_OWNER_PATH, "utf8"),
  );
}
await writeFile(process.env.AUDIO_GRAPH_FAKE_CARGO_CAPTURE, JSON.stringify(record));
if (process.env.AUDIO_GRAPH_FAKE_CARGO_DESCENDANT_PID) {
  const descendant = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
    stdio: "ignore",
    shell: false,
  });
  await writeFile(process.env.AUDIO_GRAPH_FAKE_CARGO_DESCENDANT_PID, String(descendant.pid));
}
if (process.env.AUDIO_GRAPH_FAKE_CARGO_STARTED) {
  await writeFile(process.env.AUDIO_GRAPH_FAKE_CARGO_STARTED, "started\\n");
}
if (process.env.AUDIO_GRAPH_FAKE_CARGO_RELEASE) {
  for (;;) {
    try {
      await access(process.env.AUDIO_GRAPH_FAKE_CARGO_RELEASE);
      break;
    } catch {
      await sleep(10);
    }
  }
}
`,
  );

  return {
    root: await realpath(root),
    capture: join(root, "cargo-invocation.json"),
    coordination: join(root, "coordination"),
    targetRoot: join(root, "targets"),
    tempRoot: join(root, "temp"),
    fakeCargo,
  };
}

async function pathExists(path) {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

async function waitForPath(path, timeoutMs = 2000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await pathExists(path)) return;
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 10));
  }
  throw new Error("timed out waiting for deterministic fixture marker");
}

function pidIsAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error?.code !== "ESRCH";
  }
}

async function waitForPidExit(pid, timeoutMs = 2000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!pidIsAlive(pid)) return true;
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 10));
  }
  return !pidIsAlive(pid);
}

function runFacade(fixture, mode, extraArgs = [], extraEnv = {}) {
  const budget = Math.min(2, availableParallelism());
  const child = spawn(process.execPath, [facadePath, mode, ...extraArgs], {
    cwd: fixture.root,
    env: {
      ...process.env,
      AUDIO_GRAPH_CARGO_BIN: process.execPath,
      AUDIO_GRAPH_CARGO_PREFIX_ARGS_JSON: JSON.stringify([fixture.fakeCargo]),
      AUDIO_GRAPH_CARGO_WORKTREE_ROOT: fixture.root,
      AUDIO_GRAPH_CARGO_TARGET_ROOT: fixture.targetRoot,
      AUDIO_GRAPH_CARGO_COORDINATION_DIR: fixture.coordination,
      AUDIO_GRAPH_CARGO_TEMP_ROOT: fixture.tempRoot,
      AUDIO_GRAPH_CARGO_BUDGET: String(budget),
      AUDIO_GRAPH_CARGO_POLL_MS: "10",
      AUDIO_GRAPH_CARGO_STALE_MS: "50",
      AUDIO_GRAPH_CARGO_WAIT_MS: "2000",
      AUDIO_GRAPH_FAKE_CARGO_CAPTURE: fixture.capture,
      ...extraEnv,
    },
    stdio: ["ignore", "pipe", "pipe"],
    shell: false,
  });

  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk) => {
    stdout += chunk;
  });
  child.stderr.on("data", (chunk) => {
    stderr += chunk;
  });

  const completed = new Promise((resolveCompletion, rejectCompletion) => {
    child.once("error", rejectCompletion);
    child.once("close", (code, signal) => {
      resolveCompletion({ code, signal, stdout, stderr });
    });
  });

  return { child, completed };
}

describe("Cargo lane facade", () => {
  test("package tasks expose the supported stable and clean-room modes", async () => {
    const packageJson = JSON.parse(
      await readFile(join(repositoryRoot, "package.json"), "utf8"),
    );

    expect(packageJson.scripts).toMatchObject({
      "rust:check:cloud": "bun scripts/cargo-lane.mjs cloud-check",
      "rust:test:cloud": "bun scripts/cargo-lane.mjs cloud-test",
      "rust:check:full": "bun scripts/cargo-lane.mjs full-check",
      "rust:test:full": "bun scripts/cargo-lane.mjs full-test",
      "rust:check:clean-room": "bun scripts/cargo-lane.mjs clean-room-check",
      "test:cargo-lane": "bun test scripts/cargo-lane.test.mjs",
    });
  });

  test("cloud check uses the pinned locked feature lane and a stable target", async () => {
    const fixture = await makeFixture();

    const first = await runFacade(fixture, "cloud-check").completed;
    expect(first.code).toBe(0);
    const firstInvocation = JSON.parse(await readFile(fixture.capture, "utf8"));

    expect(firstInvocation.argv).toEqual([
      "+1.95.0",
      "check",
      "--locked",
      "-p",
      "audio-graph",
      "--lib",
      "--no-default-features",
      "--features",
      "cloud",
      "--jobs",
      String(Math.min(2, availableParallelism())),
    ]);
    expect(firstInvocation.cwd).toBe(join(fixture.root, "src-tauri"));
    expect(relative(fixture.targetRoot, firstInvocation.targetDir)).toMatch(
      /^worktree-[a-f0-9]{12}[\\/]features-cloud[\\/]profile-debug$/,
    );
    expect(`${first.stdout}\n${first.stderr}`).not.toContain(fixture.root);
    expect(`${first.stdout}\n${first.stderr}`).not.toContain(
      firstInvocation.targetDir,
    );

    const second = await runFacade(fixture, "cloud-check").completed;
    expect(second.code).toBe(0);
    const secondInvocation = JSON.parse(
      await readFile(fixture.capture, "utf8"),
    );
    expect(secondInvocation.targetDir).toBe(firstInvocation.targetDir);
  });

  test("a shared override root still separates distinct worktrees", async () => {
    const firstFixture = await makeFixture();
    const secondFixture = await makeFixture();

    expect((await runFacade(firstFixture, "cloud-check").completed).code).toBe(
      0,
    );
    const firstInvocation = JSON.parse(
      await readFile(firstFixture.capture, "utf8"),
    );
    expect(
      (
        await runFacade(secondFixture, "cloud-check", [], {
          AUDIO_GRAPH_CARGO_TARGET_ROOT: firstFixture.targetRoot,
        }).completed
      ).code,
    ).toBe(0);
    const secondInvocation = JSON.parse(
      await readFile(secondFixture.capture, "utf8"),
    );

    expect(secondInvocation.targetDir).not.toBe(firstInvocation.targetDir);
    expect(
      relative(firstFixture.targetRoot, secondInvocation.targetDir),
    ).toMatch(/^worktree-[a-f0-9]{12}[\\/]features-cloud[\\/]profile-debug$/);
  });

  test("cloud test keeps its filter as one argument in the same feature lane", async () => {
    const fixture = await makeFixture();

    const result = await runFacade(fixture, "cloud-test", [
      "projection::tests::focused case",
    ]).completed;
    expect(result.code).toBe(0);
    const invocation = JSON.parse(await readFile(fixture.capture, "utf8"));

    expect(invocation.argv).toEqual([
      "+1.95.0",
      "test",
      "--locked",
      "-p",
      "audio-graph",
      "--lib",
      "--no-default-features",
      "--features",
      "cloud",
      "--jobs",
      String(Math.min(2, availableParallelism())),
      "projection::tests::focused case",
      "--",
      "--test-threads=1",
    ]);
    expect(relative(fixture.targetRoot, invocation.targetDir)).toMatch(
      /^worktree-[a-f0-9]{12}[\\/]features-cloud[\\/]profile-debug$/,
    );
  });

  test("full check uses a distinct default-feature target and pinned locked Cargo", async () => {
    const fixture = await makeFixture();

    expect((await runFacade(fixture, "cloud-check").completed).code).toBe(0);
    const cloudInvocation = JSON.parse(await readFile(fixture.capture, "utf8"));

    const result = await runFacade(fixture, "full-check").completed;
    expect(result.code).toBe(0);
    const fullInvocation = JSON.parse(await readFile(fixture.capture, "utf8"));

    expect(fullInvocation.argv).toEqual([
      "+1.95.0",
      "check",
      "--locked",
      "--all-targets",
      "--jobs",
      String(Math.min(2, availableParallelism())),
    ]);
    expect(relative(fixture.targetRoot, fullInvocation.targetDir)).toMatch(
      /^worktree-[a-f0-9]{12}[\\/]features-default[\\/]profile-debug$/,
    );
    expect(fullInvocation.targetDir).not.toBe(cloudInvocation.targetDir);
    expect(`${result.stdout}\n${result.stderr}`).toContain("mode=exclusive");
  });

  test("full test preserves the locked default-feature test convention", async () => {
    const fixture = await makeFixture();

    const result = await runFacade(fixture, "full-test", ["credential_service"])
      .completed;
    expect(result.code).toBe(0);
    const invocation = JSON.parse(await readFile(fixture.capture, "utf8"));

    expect(invocation.argv).toEqual([
      "+1.95.0",
      "test",
      "--locked",
      "--jobs",
      String(Math.min(2, availableParallelism())),
      "credential_service",
      "--",
      "--test-threads=1",
    ]);
    expect(relative(fixture.targetRoot, invocation.targetDir)).toMatch(
      /^worktree-[a-f0-9]{12}[\\/]features-default[\\/]profile-debug$/,
    );
    expect(`${result.stdout}\n${result.stderr}`).toContain("mode=exclusive");
  });

  test("one default shared build can use the full six-token host budget", async () => {
    const fixture = await makeFixture();
    const result = await runFacade(fixture, "cloud-check", [], {
      ...simulatedSixCpuHost,
      AUDIO_GRAPH_CARGO_BUDGET: "6",
    }).completed;

    expect(result.code).toBe(0);
    const invocation = JSON.parse(await readFile(fixture.capture, "utf8"));
    expect(invocation.argv.slice(-2)).toEqual(["--jobs", "6"]);
  });

  test("two concurrent default builds split a six-token budget three each", async () => {
    const fixture = await makeFixture();
    const firstStarted = join(fixture.root, "adaptive-two-first-started");
    const secondStarted = join(fixture.root, "adaptive-two-second-started");
    const firstRelease = join(fixture.root, "adaptive-two-first-release");
    const secondRelease = join(fixture.root, "adaptive-two-second-release");
    const firstCapture = join(fixture.root, "adaptive-two-first.json");
    const secondCapture = join(fixture.root, "adaptive-two-second.json");
    const commonEnv = {
      ...simulatedSixCpuHost,
      AUDIO_GRAPH_CARGO_BUDGET: "6",
      AUDIO_GRAPH_CARGO_ADAPTIVE_WINDOW_MS: "200",
    };
    const first = runFacade(fixture, "cloud-check", [], {
      ...commonEnv,
      AUDIO_GRAPH_FAKE_CARGO_CAPTURE: firstCapture,
      AUDIO_GRAPH_FAKE_CARGO_STARTED: firstStarted,
      AUDIO_GRAPH_FAKE_CARGO_RELEASE: firstRelease,
    });
    const second = runFacade(fixture, "cloud-check", [], {
      ...commonEnv,
      AUDIO_GRAPH_FAKE_CARGO_CAPTURE: secondCapture,
      AUDIO_GRAPH_FAKE_CARGO_STARTED: secondStarted,
      AUDIO_GRAPH_FAKE_CARGO_RELEASE: secondRelease,
    });

    try {
      await Promise.all([
        waitForPath(firstStarted, 1000),
        waitForPath(secondStarted, 1000),
      ]);
      const invocations = await Promise.all(
        [firstCapture, secondCapture].map(async (capture) =>
          JSON.parse(await readFile(capture, "utf8")),
        ),
      );
      expect(invocations.map(({ argv }) => argv.slice(-2))).toEqual([
        ["--jobs", "3"],
        ["--jobs", "3"],
      ]);
    } finally {
      await Promise.all([
        writeFile(firstRelease, "release\n"),
        writeFile(secondRelease, "release\n"),
      ]);
      await Promise.all([first.completed, second.completed]);
    }
  });

  test("three concurrent default builds split a six-token budget two each", async () => {
    const fixture = await makeFixture();
    const runs = Array.from({ length: 3 }, (_, index) => {
      const ordinal = index + 1;
      return {
        capture: join(fixture.root, `adaptive-three-${ordinal}.json`),
        started: join(fixture.root, `adaptive-three-${ordinal}-started`),
        release: join(fixture.root, `adaptive-three-${ordinal}-release`),
      };
    });
    const facades = runs.map((run) =>
      runFacade(fixture, "cloud-check", [], {
        ...simulatedSixCpuHost,
        AUDIO_GRAPH_CARGO_BUDGET: "6",
        AUDIO_GRAPH_CARGO_ADAPTIVE_WINDOW_MS: "200",
        AUDIO_GRAPH_FAKE_CARGO_CAPTURE: run.capture,
        AUDIO_GRAPH_FAKE_CARGO_STARTED: run.started,
        AUDIO_GRAPH_FAKE_CARGO_RELEASE: run.release,
      }),
    );

    try {
      await Promise.all(runs.map(({ started }) => waitForPath(started, 1000)));
      const invocations = await Promise.all(
        runs.map(async ({ capture }) =>
          JSON.parse(await readFile(capture, "utf8")),
        ),
      );
      expect(invocations.map(({ argv }) => argv.slice(-2))).toEqual([
        ["--jobs", "2"],
        ["--jobs", "2"],
        ["--jobs", "2"],
      ]);
    } finally {
      await Promise.all(
        runs.map(({ release }) => writeFile(release, "release\n")),
      );
      await Promise.all(facades.map(({ completed }) => completed));
    }
  });

  test("shared builds cannot reserve more jobs than the host token budget", async () => {
    const fixture = await makeFixture();
    const firstStarted = join(fixture.root, "first-started");
    const firstRelease = join(fixture.root, "first-release");
    const secondStarted = join(fixture.root, "second-started");
    const firstCapture = join(fixture.root, "first-capture.json");
    const secondCapture = join(fixture.root, "second-capture.json");

    const first = runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_FAKE_CARGO_CAPTURE: firstCapture,
      AUDIO_GRAPH_FAKE_CARGO_STARTED: firstStarted,
      AUDIO_GRAPH_FAKE_CARGO_RELEASE: firstRelease,
    });
    await waitForPath(firstStarted);

    const second = runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_FAKE_CARGO_CAPTURE: secondCapture,
      AUDIO_GRAPH_FAKE_CARGO_STARTED: secondStarted,
    });

    try {
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
      expect(await pathExists(secondStarted)).toBe(false);
      await writeFile(firstRelease, "release\n");
      expect((await first.completed).code).toBe(0);
      expect((await second.completed).code).toBe(0);
      expect(await pathExists(secondStarted)).toBe(true);
    } finally {
      await writeFile(firstRelease, "release\n");
    }
  });

  test("a dead stale lease is reclaimed before Cargo starts", async () => {
    const fixture = await makeFixture();
    const lockDir = join(fixture.coordination, "token-0.lock");
    await mkdir(lockDir, { recursive: true });
    await writeFile(
      join(fixture.coordination, "budget.json"),
      `${JSON.stringify({ version: 1, budget: 1 })}\n`,
    );
    await writeFile(
      join(lockDir, "owner.json"),
      `${JSON.stringify({
        version: 1,
        nonce: "dead-owner",
        pid: 99999999,
        childPid: null,
        processGroupId: null,
        heartbeatMs: 0,
      })}\n`,
    );

    const result = await runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_CARGO_BUDGET: "1",
      AUDIO_GRAPH_CARGO_JOBS: "1",
      AUDIO_GRAPH_CARGO_STALE_MS: "10",
      AUDIO_GRAPH_CARGO_WAIT_MS: "500",
    }).completed;

    expect(result.code).toBe(0);
    expect(await pathExists(fixture.capture)).toBe(true);
    expect(await pathExists(lockDir)).toBe(false);
    expect(`${result.stdout}\n${result.stderr}`).not.toContain(fixture.root);
  });

  test("an interrupted owner-file write cannot wedge the token pool", async () => {
    const fixture = await makeFixture();
    const lockDir = join(fixture.coordination, "token-0.lock");
    const ownerPath = join(lockDir, "owner.json");
    await mkdir(lockDir, { recursive: true });
    await writeFile(
      join(fixture.coordination, "budget.json"),
      `${JSON.stringify({ version: 1, budget: 1 })}\n`,
    );
    await writeFile(ownerPath, "{");
    await utimes(ownerPath, new Date(0), new Date(0));

    const result = await runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_CARGO_BUDGET: "1",
      AUDIO_GRAPH_CARGO_JOBS: "1",
      AUDIO_GRAPH_CARGO_STALE_MS: "10",
    }).completed;

    expect(result.code).toBe(0);
    expect(await pathExists(fixture.capture)).toBe(true);
    expect(await pathExists(lockDir)).toBe(false);
  });

  test("interruption stops Cargo and releases path-free leases", async () => {
    // Windows `.kill("SIGTERM")` uses TerminateProcess and cannot exercise a
    // process-level signal handler. Windows hard-stop recovery is covered by
    // the dead/stale and orphan-child lease tests instead.
    if (process.platform === "win32") return;

    const fixture = await makeFixture();
    const started = join(fixture.root, "interrupt-started");
    const release = join(fixture.root, "interrupt-release");
    const descendantPidPath = join(fixture.root, "descendant-pid");
    let descendantPid = null;
    const running = runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_CARGO_BUDGET: "1",
      AUDIO_GRAPH_CARGO_JOBS: "1",
      AUDIO_GRAPH_FAKE_CARGO_STARTED: started,
      AUDIO_GRAPH_FAKE_CARGO_RELEASE: release,
      AUDIO_GRAPH_FAKE_CARGO_DESCENDANT_PID: descendantPidPath,
    });

    try {
      await waitForPath(started);
      await waitForPath(descendantPidPath);
      descendantPid = Number(await readFile(descendantPidPath, "utf8"));
      const ownerPath = join(
        fixture.coordination,
        "token-0.lock",
        "owner.json",
      );
      const owner = JSON.parse(await readFile(ownerPath, "utf8"));
      expect(Number.isSafeInteger(owner.childPid)).toBe(true);
      expect(JSON.stringify(owner)).not.toContain(fixture.root);
      expect(Object.keys(owner).sort()).toEqual([
        "childPid",
        "heartbeatMs",
        "nonce",
        "pid",
        "processGroupId",
        "version",
      ]);

      running.child.kill("SIGTERM");
      const result = await Promise.race([
        running.completed,
        new Promise((_, rejectTimeout) =>
          setTimeout(
            () => rejectTimeout(new Error("interrupted facade did not exit")),
            2000,
          ),
        ),
      ]);
      expect(result.code).toBe(143);
      expect(result.signal).toBeNull();
      expect(await pathExists(join(fixture.coordination, "token-0.lock"))).toBe(
        false,
      );
      expect(await waitForPidExit(descendantPid)).toBe(true);
    } finally {
      await writeFile(release, "release\n");
      if (descendantPid && pidIsAlive(descendantPid)) {
        process.kill(descendantPid, "SIGKILL");
      }
    }
  });

  test("cleanup uncertainty retains the lease until the recorded group is dead", async () => {
    if (process.platform === "win32") return;

    const fixture = await makeFixture();
    const lockDir = join(fixture.coordination, "token-0.lock");
    const firstCapture = join(fixture.root, "uncertain-first.json");
    const blockedCapture = join(fixture.root, "uncertain-blocked.json");
    const recoveredCapture = join(fixture.root, "uncertain-recovered.json");
    const commonEnv = {
      AUDIO_GRAPH_CARGO_BUDGET: "1",
      AUDIO_GRAPH_CARGO_JOBS: "1",
      AUDIO_GRAPH_CARGO_STALE_MS: "20",
      AUDIO_GRAPH_CARGO_WAIT_MS: "80",
      AUDIO_GRAPH_CARGO_LANE_TEST_MODE: "1",
      AUDIO_GRAPH_CARGO_LANE_TEST_FORCE_GROUP_ALIVE: "1",
    };

    const uncertain = await runFacade(fixture, "cloud-check", [], {
      ...commonEnv,
      AUDIO_GRAPH_FAKE_CARGO_CAPTURE: firstCapture,
    }).completed;
    expect(uncertain.code).toBe(2);
    expect(uncertain.stderr).toContain("cargo_descendant_cleanup_failed");
    expect(await pathExists(firstCapture)).toBe(true);
    expect(await pathExists(lockDir)).toBe(true);

    const blocked = await runFacade(fixture, "cloud-check", [], {
      ...commonEnv,
      AUDIO_GRAPH_FAKE_CARGO_CAPTURE: blockedCapture,
    }).completed;
    expect(blocked.code).toBe(2);
    expect(blocked.stderr).toContain("cargo_host_budget_wait_timed_out");
    expect(await pathExists(blockedCapture)).toBe(false);
    expect(await pathExists(lockDir)).toBe(true);

    await new Promise((resolveDelay) => setTimeout(resolveDelay, 30));
    const recovered = await runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_CARGO_BUDGET: "1",
      AUDIO_GRAPH_CARGO_JOBS: "1",
      AUDIO_GRAPH_CARGO_STALE_MS: "20",
      AUDIO_GRAPH_CARGO_WAIT_MS: "500",
      AUDIO_GRAPH_FAKE_CARGO_CAPTURE: recoveredCapture,
    }).completed;
    expect(recovered.code).toBe(0);
    expect(recovered.stderr).toContain("state=reclaimed-stale-lease");
    expect(await pathExists(recoveredCapture)).toBe(true);
    expect(await pathExists(lockDir)).toBe(false);
  });

  test("Cargo cannot execute before its detached owner is durably registered", async () => {
    if (process.platform === "win32") return;

    const fixture = await makeFixture();
    const barrierMarker = join(fixture.root, "registration-barrier");
    const barrierRelease = join(fixture.root, "registration-release");
    const cargoStarted = join(fixture.root, "barrier-cargo-started");
    const cargoRelease = join(fixture.root, "barrier-cargo-release");
    const firstCapture = join(fixture.root, "barrier-first.json");
    const recoveredCapture = join(fixture.root, "barrier-recovered.json");
    const ownerPath = join(
      fixture.coordination,
      "token-0.lock",
      "owner.json",
    );
    const running = runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_CARGO_BUDGET: "1",
      AUDIO_GRAPH_CARGO_JOBS: "1",
      AUDIO_GRAPH_CARGO_STALE_MS: "30",
      AUDIO_GRAPH_CARGO_LANE_TEST_MODE: "1",
      AUDIO_GRAPH_CARGO_LANE_TEST_BARRIER_MARKER: barrierMarker,
      AUDIO_GRAPH_CARGO_LANE_TEST_BARRIER_RELEASE: barrierRelease,
      AUDIO_GRAPH_FAKE_CARGO_CAPTURE: firstCapture,
      AUDIO_GRAPH_FAKE_CARGO_STARTED: cargoStarted,
      AUDIO_GRAPH_FAKE_CARGO_RELEASE: cargoRelease,
    });

    try {
      await Promise.race([
        waitForPath(barrierMarker, 500),
        waitForPath(cargoStarted, 500),
      ]);
      expect(await pathExists(barrierMarker)).toBe(true);
      expect(await pathExists(cargoStarted)).toBe(false);

      running.child.kill("SIGKILL");
      const hardStop = await running.completed;
      expect(hardStop.code).toBeNull();
      expect(hardStop.signal).toBe("SIGKILL");
      await writeFile(barrierRelease, "release\n");
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 80));
      expect(await pathExists(cargoStarted)).toBe(false);
      expect(await pathExists(firstCapture)).toBe(false);

      const recovered = await runFacade(fixture, "cloud-check", [], {
        AUDIO_GRAPH_CARGO_BUDGET: "1",
        AUDIO_GRAPH_CARGO_JOBS: "1",
        AUDIO_GRAPH_CARGO_STALE_MS: "30",
        AUDIO_GRAPH_CARGO_WAIT_MS: "500",
        AUDIO_GRAPH_FAKE_CARGO_CAPTURE: recoveredCapture,
        AUDIO_GRAPH_FAKE_CARGO_OWNER_PATH: ownerPath,
      }).completed;
      expect(recovered.code).toBe(0);
      expect(recovered.stderr).toContain("state=reclaimed-stale-lease");
      const invocation = JSON.parse(await readFile(recoveredCapture, "utf8"));
      expect(invocation.ownerAtStart.childPid).toBeGreaterThan(0);
      expect(invocation.ownerAtStart.processGroupId).toBe(
        invocation.ownerAtStart.childPid,
      );
    } finally {
      await writeFile(barrierRelease, "release\n");
      await writeFile(cargoRelease, "release\n");
      if (running.child.exitCode === null && running.child.signalCode === null) {
        running.child.kill("SIGTERM");
        await Promise.race([
          running.completed,
          new Promise((resolveDelay) => setTimeout(resolveDelay, 500)),
        ]);
      }
      if (running.child.exitCode === null && running.child.signalCode === null) {
        running.child.kill("SIGKILL");
        await running.completed;
      }
    }
  });

  test("losing an active lease stops Cargo before reporting failure", async () => {
    const fixture = await makeFixture();
    const started = join(fixture.root, "lost-lease-started");
    const release = join(fixture.root, "lost-lease-release");
    const running = runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_CARGO_BUDGET: "1",
      AUDIO_GRAPH_CARGO_JOBS: "1",
      AUDIO_GRAPH_CARGO_STALE_MS: "30",
      AUDIO_GRAPH_FAKE_CARGO_STARTED: started,
      AUDIO_GRAPH_FAKE_CARGO_RELEASE: release,
    });

    try {
      await waitForPath(started);
      await rm(join(fixture.coordination, "token-0.lock"), {
        recursive: true,
        force: true,
      });
      const result = await Promise.race([
        running.completed,
        new Promise((_, rejectTimeout) =>
          setTimeout(
            () => rejectTimeout(new Error("lease loss did not stop Cargo")),
            2000,
          ),
        ),
      ]);
      expect(result.code).toBe(2);
      expect(`${result.stdout}\n${result.stderr}`).not.toContain(fixture.root);
    } finally {
      await writeFile(release, "release\n");
    }
  });

  test("an exclusive full gate waits for every token, including idle capacity", async () => {
    if (availableParallelism() < 2) return;

    const fixture = await makeFixture();
    const sharedStarted = join(fixture.root, "shared-started");
    const sharedRelease = join(fixture.root, "shared-release");
    const fullStarted = join(fixture.root, "full-started");

    const shared = runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_CARGO_BUDGET: "2",
      AUDIO_GRAPH_CARGO_JOBS: "1",
      AUDIO_GRAPH_FAKE_CARGO_CAPTURE: join(fixture.root, "shared.json"),
      AUDIO_GRAPH_FAKE_CARGO_STARTED: sharedStarted,
      AUDIO_GRAPH_FAKE_CARGO_RELEASE: sharedRelease,
    });
    await waitForPath(sharedStarted);

    const full = runFacade(fixture, "full-check", [], {
      AUDIO_GRAPH_CARGO_BUDGET: "2",
      AUDIO_GRAPH_CARGO_JOBS: "1",
      AUDIO_GRAPH_FAKE_CARGO_CAPTURE: join(fixture.root, "full.json"),
      AUDIO_GRAPH_FAKE_CARGO_STARTED: fullStarted,
    });

    try {
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
      expect(await pathExists(fullStarted)).toBe(false);
      await writeFile(sharedRelease, "release\n");
      expect((await shared.completed).code).toBe(0);
      expect((await full.completed).code).toBe(0);
      expect(await pathExists(fullStarted)).toBe(true);
    } finally {
      await writeFile(sharedRelease, "release\n");
    }
  });

  test("a budget change waits for idle admission, then reconfigures in place", async () => {
    if (availableParallelism() < 2) return;

    const fixture = await makeFixture();
    const firstStarted = join(fixture.root, "old-budget-started");
    const firstRelease = join(fixture.root, "old-budget-release");
    const secondStarted = join(fixture.root, "new-budget-started");

    const first = runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_CARGO_BUDGET: "1",
      AUDIO_GRAPH_CARGO_JOBS: "1",
      AUDIO_GRAPH_FAKE_CARGO_CAPTURE: join(fixture.root, "old-budget.json"),
      AUDIO_GRAPH_FAKE_CARGO_STARTED: firstStarted,
      AUDIO_GRAPH_FAKE_CARGO_RELEASE: firstRelease,
    });
    await waitForPath(firstStarted);

    const second = runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_CARGO_BUDGET: "2",
      AUDIO_GRAPH_CARGO_JOBS: "1",
      AUDIO_GRAPH_FAKE_CARGO_CAPTURE: join(fixture.root, "new-budget.json"),
      AUDIO_GRAPH_FAKE_CARGO_STARTED: secondStarted,
    });

    try {
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 100));
      expect(await pathExists(secondStarted)).toBe(false);
      await writeFile(firstRelease, "release\n");
      expect((await first.completed).code).toBe(0);
      expect((await second.completed).code).toBe(0);
      expect(await pathExists(secondStarted)).toBe(true);
      expect(
        JSON.parse(
          await readFile(join(fixture.coordination, "budget.json"), "utf8"),
        ).budget,
      ).toBe(2);
    } finally {
      await writeFile(firstRelease, "release\n");
    }
  });

  test("the explicit clean-room mode creates, reports, and preserves one fresh target", async () => {
    const fixture = await makeFixture();

    expect(await readdir(fixture.tempRoot)).toEqual([]);
    const result = await runFacade(fixture, "clean-room-check").completed;
    expect(result.code).toBe(0);
    const invocation = JSON.parse(await readFile(fixture.capture, "utf8"));
    const cleanRooms = (await readdir(fixture.tempRoot)).filter((entry) =>
      entry.startsWith("audio-graph-cargo-clean-room-default-debug-"),
    );

    expect(cleanRooms).toHaveLength(1);
    expect(invocation.targetDir).toBe(join(fixture.tempRoot, cleanRooms[0]));
    expect(await pathExists(invocation.targetDir)).toBe(true);
    expect(`${result.stdout}\n${result.stderr}`).toContain(
      `clean_room_target=${invocation.targetDir}`,
    );
    expect(invocation.argv).toEqual([
      "+1.95.0",
      "check",
      "--locked",
      "--all-targets",
      "--jobs",
      String(Math.min(2, availableParallelism())),
    ]);
  });

  test("test filters cannot smuggle Cargo flags or shell execution", async () => {
    const fixture = await makeFixture();

    const flagResult = await runFacade(fixture, "cloud-test", ["--release"])
      .completed;
    expect(flagResult.code).toBe(2);
    expect(await pathExists(fixture.capture)).toBe(false);

    const marker = join(fixture.root, "must-not-exist");
    const literalFilter = `focused;touch ${marker}`;
    const literalResult = await runFacade(fixture, "cloud-test", [
      literalFilter,
    ]).completed;
    expect(literalResult.code).toBe(0);
    const invocation = JSON.parse(await readFile(fixture.capture, "utf8"));
    expect(invocation.argv).toContain(literalFilter);
    expect(await pathExists(marker)).toBe(false);
  });

  test("the configured host budget cannot exceed detected parallelism", async () => {
    const fixture = await makeFixture();
    const result = await runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_CARGO_BUDGET: String(availableParallelism() + 1),
      AUDIO_GRAPH_CARGO_JOBS: "1",
    }).completed;

    expect(result.code).toBe(2);
    expect(result.stderr).toContain("cargo_budget_exceeds_detected_cpus");
    expect(await pathExists(fixture.capture)).toBe(false);
  });

  test("Windows refuses coordinated execution before acquiring or spawning", async () => {
    const fixture = await makeFixture();
    const result = await runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_CARGO_LANE_TEST_MODE: "1",
      AUDIO_GRAPH_CARGO_LANE_TEST_PLATFORM: "win32",
    }).completed;

    expect(result.code).toBe(2);
    expect(result.stderr).toContain(
      "windows_descendant_ownership_unavailable",
    );
    expect(await pathExists(fixture.capture)).toBe(false);
    expect(await pathExists(fixture.coordination)).toBe(false);
  });

  test("system failures are sanitized instead of logging filesystem paths", async () => {
    const fixture = await makeFixture();
    const missingExecutable = join(fixture.root, "private", "missing-cargo");
    const result = await runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_CARGO_BIN: missingExecutable,
      AUDIO_GRAPH_CARGO_PREFIX_ARGS_JSON: "[]",
    }).completed;

    expect(result.code).toBe(2);
    expect(`${result.stdout}\n${result.stderr}`).not.toContain(fixture.root);
    expect(result.stderr).toContain("system_enoent");
    expect(await pathExists(join(fixture.coordination, "token-0.lock"))).toBe(
      false,
    );
  });

  test("an old lease is retained while its orphaned Cargo child PID is alive", async () => {
    const fixture = await makeFixture();
    const lockDir = join(fixture.coordination, "token-0.lock");
    await mkdir(lockDir, { recursive: true });
    await writeFile(
      join(fixture.coordination, "budget.json"),
      `${JSON.stringify({ version: 1, budget: 1 })}\n`,
    );
    await writeFile(
      join(lockDir, "owner.json"),
      `${JSON.stringify({
        version: 1,
        nonce: "live-owner",
        pid: 99999999,
        childPid: process.pid,
        processGroupId: null,
        heartbeatMs: 0,
      })}\n`,
    );

    const result = await runFacade(fixture, "cloud-check", [], {
      AUDIO_GRAPH_CARGO_BUDGET: "1",
      AUDIO_GRAPH_CARGO_JOBS: "1",
      AUDIO_GRAPH_CARGO_STALE_MS: "10",
      AUDIO_GRAPH_CARGO_WAIT_MS: "80",
    }).completed;

    expect(result.code).toBe(2);
    expect(await pathExists(lockDir)).toBe(true);
    expect(await pathExists(fixture.capture)).toBe(false);
  });
});
