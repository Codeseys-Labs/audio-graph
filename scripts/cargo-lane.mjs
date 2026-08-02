#!/usr/bin/env bun

import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import {
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  realpath,
  rename,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { availableParallelism, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptPath = fileURLToPath(import.meta.url);
const scriptRoot = resolve(dirname(scriptPath), "..");
const registeredChildMode = "__registered-cargo-child";
const registeredChildCommandKey = "AUDIO_GRAPH_CARGO_LANE_CHILD_COMMAND";
const registeredChildArgsKey = "AUDIO_GRAPH_CARGO_LANE_CHILD_ARGS_JSON";

function fail(message) {
  process.stderr.write(`[cargo-lane] error=${message}\n`);
  return 2;
}

function safeErrorLabel(error) {
  if (!(error instanceof Error)) return "unexpected_failure";
  if (/^[A-Za-z0-9_]+$/.test(error.message)) return error.message;
  if (typeof error.code === "string" && /^[A-Z0-9_]+$/.test(error.code)) {
    return `system_${error.code.toLowerCase()}`;
  }
  return "unexpected_failure";
}

function retainTokenLease(error) {
  const retainedError =
    error instanceof Error ? error : new Error("cargo_descendant_cleanup_failed");
  retainedError.retainCargoTokens = true;
  return retainedError;
}

function positiveInteger(name, value, fallback) {
  if (value === undefined || value === "") return fallback;
  if (!/^[1-9]\d*$/.test(value)) {
    throw new Error(`${name}_must_be_a_positive_integer`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed)) {
    throw new Error(`${name}_must_be_a_safe_integer`);
  }
  return parsed;
}

function cargoPrefix(env) {
  if (!env.AUDIO_GRAPH_CARGO_PREFIX_ARGS_JSON) return [];

  let parsed;
  try {
    parsed = JSON.parse(env.AUDIO_GRAPH_CARGO_PREFIX_ARGS_JSON);
  } catch {
    throw new Error("cargo_prefix_args_must_be_json");
  }
  if (
    !Array.isArray(parsed) ||
    parsed.some((value) => typeof value !== "string")
  ) {
    throw new Error("cargo_prefix_args_must_be_a_string_array");
  }
  return parsed;
}

function worktreeIdentity(worktreeRoot) {
  const normalized =
    process.platform === "win32" ? worktreeRoot.toLowerCase() : worktreeRoot;
  return createHash("sha256").update(normalized).digest("hex").slice(0, 12);
}

const sleep = (milliseconds) =>
  new Promise((resolveSleep) => setTimeout(resolveSleep, milliseconds));

async function fileExists(path) {
  try {
    await stat(path);
    return true;
  } catch {
    return false;
  }
}

function registeredChildArgs(env) {
  let args;
  try {
    args = JSON.parse(env[registeredChildArgsKey]);
  } catch {
    throw new Error("registered_child_args_invalid");
  }
  if (!Array.isArray(args) || args.some((arg) => typeof arg !== "string")) {
    throw new Error("registered_child_args_invalid");
  }
  return args;
}

async function sendToFacade(message) {
  if (typeof process.send !== "function" || !process.connected) return false;
  return await new Promise((resolveSend) => {
    try {
      process.send(message, (error) => resolveSend(!error));
    } catch {
      resolveSend(false);
    }
  });
}

async function registeredChildMain(env = process.env) {
  const command = env[registeredChildCommandKey];
  if (typeof command !== "string" || command.length === 0) {
    return fail("registered_child_command_invalid");
  }

  let args;
  try {
    args = registeredChildArgs(env);
  } catch (error) {
    return fail(safeErrorLabel(error));
  }

  let parentConnected = process.connected;
  let resolveDisconnected;
  const disconnected = new Promise((resolveDisconnect) => {
    resolveDisconnected = resolveDisconnect;
  });
  process.once("disconnect", () => {
    parentConnected = false;
    resolveDisconnected(false);
  });

  const barrierMarker = env.AUDIO_GRAPH_CARGO_LANE_TEST_BARRIER_MARKER;
  const barrierRelease = env.AUDIO_GRAPH_CARGO_LANE_TEST_BARRIER_RELEASE;
  if (
    env.AUDIO_GRAPH_CARGO_LANE_TEST_MODE === "1" &&
    barrierMarker &&
    barrierRelease
  ) {
    await writeFile(barrierMarker, "waiting\n", { mode: 0o600 });
    while (parentConnected && !(await fileExists(barrierRelease))) {
      await sleep(10);
    }
  }
  if (!parentConnected) return 0;

  let resolveStart;
  const startInstruction = new Promise((resolveInstruction) => {
    resolveStart = resolveInstruction;
  });
  process.on("message", (message) => {
    if (message?.type === "start") resolveStart(true);
  });
  if (!(await sendToFacade({ type: "ready" }))) return 0;
  if (!(await Promise.race([startInstruction, disconnected]))) return 0;

  const cargoEnv = { ...env };
  delete cargoEnv[registeredChildCommandKey];
  delete cargoEnv[registeredChildArgsKey];
  delete cargoEnv.AUDIO_GRAPH_CARGO_LANE_TEST_BARRIER_MARKER;
  delete cargoEnv.AUDIO_GRAPH_CARGO_LANE_TEST_BARRIER_RELEASE;

  const exitCode = await new Promise((resolveCargo) => {
    const cargo = spawn(command, args, {
      cwd: process.cwd(),
      env: cargoEnv,
      shell: false,
      stdio: "inherit",
    });
    let startupFailed = false;
    cargo.once("error", (error) => {
      startupFailed = true;
      void sendToFacade({
        type: "startup-error",
        label: safeErrorLabel(error),
      }).finally(() => resolveCargo(127));
    });
    cargo.once("close", (code, signal) => {
      if (startupFailed) return;
      if (typeof code === "number") {
        resolveCargo(code);
        return;
      }
      resolveCargo(signal === "SIGINT" ? 130 : signal === "SIGTERM" ? 143 : 1);
    });
  });
  if (process.connected) process.disconnect();
  return exitCode;
}

async function releaseTokens(tokens) {
  await Promise.all(
    tokens.map(async ({ lockDir, nonce }) => {
      try {
        const owner = JSON.parse(
          await readFile(join(lockDir, "owner.json"), "utf8"),
        );
        if (owner.nonce !== nonce) return;
        await rm(lockDir, { recursive: true, force: true });
      } catch {
        // A lost/reclaimed lease is no longer ours to remove.
      }
    }),
  );
}

async function writeTokenOwner(token) {
  const ownerPath = join(token.lockDir, "owner.json");
  const temporaryPath = join(
    token.lockDir,
    `owner-${token.nonce}-${randomUUID()}.tmp`,
  );
  try {
    await writeFile(temporaryPath, `${JSON.stringify(token.owner)}\n`, {
      mode: 0o600,
    });
    await rename(temporaryPath, ownerPath);
  } catch (error) {
    await rm(temporaryPath, { force: true });
    throw error;
  }
}

async function updateTokenOwners(tokens, childPid) {
  const heartbeatMs = Date.now();
  await Promise.all(
    tokens.map(async (token) => {
      token.owner.childPid = childPid;
      token.owner.processGroupId = childPid;
      token.owner.heartbeatMs = heartbeatMs;
      await writeTokenOwner(token);
    }),
  );
}

function pidIsAlive(pid) {
  if (!Number.isSafeInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return !(error instanceof Error) || error.code !== "ESRCH";
  }
}

function leaseOwnerIsWellFormed(owner) {
  return (
    owner !== null &&
    typeof owner === "object" &&
    owner.version === 1 &&
    typeof owner.nonce === "string" &&
    owner.nonce.length > 0 &&
    Number.isSafeInteger(owner.pid) &&
    owner.pid > 0 &&
    (owner.childPid === null ||
      (Number.isSafeInteger(owner.childPid) && owner.childPid > 0)) &&
    (owner.processGroupId === null ||
      (Number.isSafeInteger(owner.processGroupId) &&
        owner.processGroupId > 0)) &&
    Number.isFinite(owner.heartbeatMs) &&
    owner.heartbeatMs >= 0
  );
}

async function invalidLeaseState(lockDir, ownerPath) {
  try {
    return { source: "owner", mtimeMs: (await stat(ownerPath)).mtimeMs };
  } catch {
    return { source: "directory", mtimeMs: (await stat(lockDir)).mtimeMs };
  }
}

async function reclaimStaleToken(lockDir, staleMs) {
  const ownerPath = join(lockDir, "owner.json");
  let firstOwner = null;
  let firstInvalidState = null;

  try {
    const parsed = JSON.parse(await readFile(ownerPath, "utf8"));
    if (leaseOwnerIsWellFormed(parsed)) {
      firstOwner = parsed;
    } else {
      if (
        pidIsAlive(parsed?.pid) ||
        pidIsAlive(parsed?.childPid) ||
        processGroupIsAlive(parsed?.processGroupId)
      ) {
        return false;
      }
      firstInvalidState = await invalidLeaseState(lockDir, ownerPath);
    }
  } catch {
    try {
      firstInvalidState = await invalidLeaseState(lockDir, ownerPath);
    } catch {
      return true;
    }
  }

  if (firstOwner) {
    if (
      !Number.isFinite(firstOwner.heartbeatMs) ||
      Date.now() - firstOwner.heartbeatMs <= staleMs ||
      pidIsAlive(firstOwner.pid) ||
      pidIsAlive(firstOwner.childPid) ||
      processGroupIsAlive(firstOwner.processGroupId)
    ) {
      return false;
    }

    try {
      const currentOwner = JSON.parse(await readFile(ownerPath, "utf8"));
      if (
        currentOwner.nonce !== firstOwner.nonce ||
        currentOwner.heartbeatMs !== firstOwner.heartbeatMs
      ) {
        return false;
      }
    } catch {
      return false;
    }
  } else {
    if (Date.now() - firstInvalidState.mtimeMs <= staleMs) return false;
    try {
      const currentOwner = JSON.parse(await readFile(ownerPath, "utf8"));
      if (
        leaseOwnerIsWellFormed(currentOwner) ||
        pidIsAlive(currentOwner?.pid) ||
        pidIsAlive(currentOwner?.childPid) ||
        processGroupIsAlive(currentOwner?.processGroupId)
      ) {
        return false;
      }
    } catch {
      // A still-invalid owner is reclaimable only if its marker stayed put.
    }
    const currentInvalidState = await invalidLeaseState(lockDir, ownerPath);
    if (
      currentInvalidState.source !== firstInvalidState.source ||
      currentInvalidState.mtimeMs !== firstInvalidState.mtimeMs
    ) {
      return false;
    }
  }

  const reclaimedDir = `${lockDir}.reclaim-${randomUUID()}`;
  try {
    await rename(lockDir, reclaimedDir);
  } catch (error) {
    if (error instanceof Error && error.code === "ENOENT") return true;
    return false;
  }
  await rm(reclaimedDir, { recursive: true, force: true });
  process.stderr.write("[cargo-lane] state=reclaimed-stale-lease\n");
  return true;
}

async function tryAcquireLease(lockDir, staleMs) {
  try {
    await mkdir(lockDir, { mode: 0o700 });
  } catch (error) {
    if (error instanceof Error && error.code === "EEXIST") {
      if (await reclaimStaleToken(lockDir, staleMs)) {
        return await tryAcquireLease(lockDir, staleMs);
      }
      return null;
    }
    throw error;
  }

  const nonce = randomUUID();
  const owner = {
    version: 1,
    nonce,
    pid: process.pid,
    childPid: null,
    processGroupId: null,
    heartbeatMs: Date.now(),
  };
  try {
    await writeFile(join(lockDir, "owner.json"), `${JSON.stringify(owner)}\n`, {
      flag: "wx",
      mode: 0o600,
    });
    return { lockDir, nonce, owner };
  } catch (error) {
    await rm(lockDir, { recursive: true, force: true });
    throw error;
  }
}

async function ensureBudgetWhileAdmitted(coordinationRoot, budget, staleMs) {
  const configPath = join(coordinationRoot, "budget.json");
  let currentBudget = null;
  try {
    const current = JSON.parse(await readFile(configPath, "utf8"));
    if (
      current.version === 1 &&
      Number.isSafeInteger(current.budget) &&
      current.budget > 0
    ) {
      currentBudget = current.budget;
    }
  } catch {
    // Missing or interrupted configuration is repaired only while fully idle.
  }

  if (currentBudget === budget) return true;

  const entries = await readdir(coordinationRoot, { withFileTypes: true });
  for (const entry of entries) {
    if (!entry.isDirectory() || !/^token-\d+\.lock$/.test(entry.name)) continue;
    const lockDir = join(coordinationRoot, entry.name);
    if (!(await reclaimStaleToken(lockDir, staleMs))) return false;
  }

  await writeFile(configPath, `${JSON.stringify({ version: 1, budget })}\n`, {
    mode: 0o600,
  });
  if (currentBudget !== null) {
    process.stderr.write(
      `[cargo-lane] state=budget-reconfigured budget=${budget}\n`,
    );
  }
  return true;
}

function adaptiveRequestIsWellFormed(owner) {
  return (
    owner !== null &&
    typeof owner === "object" &&
    owner.version === 1 &&
    typeof owner.nonce === "string" &&
    owner.nonce.length > 0 &&
    Number.isSafeInteger(owner.pid) &&
    owner.pid > 0 &&
    Number.isSafeInteger(owner.budget) &&
    owner.budget > 0 &&
    Number.isFinite(owner.createdMs) &&
    owner.createdMs >= 0 &&
    Number.isFinite(owner.heartbeatMs) &&
    owner.heartbeatMs >= 0 &&
    (owner.assignedJobs === null ||
      (Number.isSafeInteger(owner.assignedJobs) &&
        owner.assignedJobs > 0 &&
        owner.assignedJobs <= owner.budget))
  );
}

async function writeAdaptiveRequest(request) {
  const ownerPath = join(request.lockDir, "owner.json");
  const temporaryPath = join(
    request.lockDir,
    `owner-${request.owner.nonce}-${randomUUID()}.tmp`,
  );
  try {
    await writeFile(temporaryPath, `${JSON.stringify(request.owner)}\n`, {
      mode: 0o600,
    });
    await rename(temporaryPath, ownerPath);
  } catch (error) {
    await rm(temporaryPath, { force: true });
    throw error;
  }
}

async function createAdaptiveRequest(coordinationRoot, budget) {
  await mkdir(coordinationRoot, { recursive: true, mode: 0o700 });
  const nonce = randomUUID();
  const lockDir = join(coordinationRoot, `request-${nonce}.lock`);
  const createdMs = Date.now();
  const request = {
    lockDir,
    owner: {
      version: 1,
      nonce,
      pid: process.pid,
      budget,
      createdMs,
      heartbeatMs: createdMs,
      assignedJobs: null,
    },
  };
  await mkdir(lockDir, { mode: 0o700 });
  try {
    await writeFile(
      join(lockDir, "owner.json"),
      `${JSON.stringify(request.owner)}\n`,
      { flag: "wx", mode: 0o600 },
    );
    return request;
  } catch (error) {
    await rm(lockDir, { recursive: true, force: true });
    throw error;
  }
}

async function readAdaptiveRequest(lockDir) {
  try {
    const owner = JSON.parse(
      await readFile(join(lockDir, "owner.json"), "utf8"),
    );
    return adaptiveRequestIsWellFormed(owner) ? owner : null;
  } catch {
    return null;
  }
}

async function removeStaleAdaptiveRequest(lockDir, owner, staleMs) {
  if (owner && pidIsAlive(owner.pid)) return;
  let markerMs;
  try {
    markerMs = owner?.heartbeatMs ?? (await stat(lockDir)).mtimeMs;
  } catch {
    return;
  }
  if (Date.now() - markerMs <= staleMs) return;

  if (owner) {
    const current = await readAdaptiveRequest(lockDir);
    if (
      !current ||
      current.nonce !== owner.nonce ||
      current.heartbeatMs !== owner.heartbeatMs ||
      pidIsAlive(current.pid)
    ) {
      return;
    }
  }
  const reclaimedDir = `${lockDir}.reclaim-${randomUUID()}`;
  try {
    await rename(lockDir, reclaimedDir);
  } catch {
    return;
  }
  await rm(reclaimedDir, { recursive: true, force: true });
}

async function unassignedAdaptiveRequests(
  coordinationRoot,
  budget,
  staleMs,
) {
  const entries = await readdir(coordinationRoot, { withFileTypes: true });
  const requests = [];
  for (const entry of entries) {
    if (
      !entry.isDirectory() ||
      !/^request-[a-f0-9-]+\.lock$/.test(entry.name)
    ) {
      continue;
    }
    const lockDir = join(coordinationRoot, entry.name);
    const owner = await readAdaptiveRequest(lockDir);
    if (!owner || !pidIsAlive(owner.pid)) {
      await removeStaleAdaptiveRequest(lockDir, owner, staleMs);
      continue;
    }
    if (owner.budget === budget && owner.assignedJobs === null) {
      requests.push({ lockDir, owner });
    }
  }
  return requests;
}

async function adaptiveJobs({
  coordinationRoot,
  budget,
  adaptiveWindowMs,
  pollMs,
  staleMs,
  waitMs,
}) {
  const request = await createAdaptiveRequest(coordinationRoot, budget);
  const deadline = Date.now() + waitMs;
  try {
    await sleep(adaptiveWindowMs);
    for (;;) {
      const current = await readAdaptiveRequest(request.lockDir);
      if (
        current?.nonce === request.owner.nonce &&
        Number.isSafeInteger(current.assignedJobs) &&
        current.assignedJobs > 0
      ) {
        return current.assignedJobs;
      }

      const admission = await tryAcquireLease(
        join(coordinationRoot, "admission.lock"),
        staleMs,
      );
      if (admission) {
        try {
          const cohort = await unassignedAdaptiveRequests(
            coordinationRoot,
            budget,
            staleMs,
          );
          if (
            cohort.some(
              ({ owner }) => owner.nonce === request.owner.nonce,
            )
          ) {
            const concurrentBuilds = Math.min(cohort.length, budget);
            const assignedJobs = Math.max(
              1,
              Math.floor(budget / concurrentBuilds),
            );
            await Promise.all(
              cohort.map(async (cohortRequest) => {
                cohortRequest.owner.assignedJobs = assignedJobs;
                cohortRequest.owner.heartbeatMs = Date.now();
                await writeAdaptiveRequest(cohortRequest);
              }),
            );
          }
        } finally {
          await releaseTokens([admission]);
        }
      }

      if (Date.now() >= deadline) {
        throw new Error("cargo_adaptive_batch_wait_timed_out");
      }
      await sleep(pollMs);
    }
  } finally {
    await rm(request.lockDir, { recursive: true, force: true });
  }
}

async function acquireTokens({
  coordinationRoot,
  budget,
  required,
  pollMs,
  staleMs,
  waitMs,
}) {
  await mkdir(coordinationRoot, { recursive: true, mode: 0o700 });
  const deadline = Date.now() + waitMs;
  let announcedWait = false;

  for (;;) {
    const admission = await tryAcquireLease(
      join(coordinationRoot, "admission.lock"),
      staleMs,
    );
    if (admission) {
      const acquired = [];
      let admittedTokens = null;
      let acquisitionError = null;
      try {
        if (
          await ensureBudgetWhileAdmitted(coordinationRoot, budget, staleMs)
        ) {
          for (let index = 0; index < budget; index += 1) {
            const token = await tryAcquireLease(
              join(coordinationRoot, `token-${index}.lock`),
              staleMs,
            );
            if (token) acquired.push(token);
            if (acquired.length === required) {
              admittedTokens = acquired;
              break;
            }
          }
        }
      } catch (error) {
        acquisitionError = error;
      } finally {
        await releaseTokens([admission]);
      }
      if (admittedTokens) return admittedTokens;
      await releaseTokens(acquired);
      if (acquisitionError) throw acquisitionError;
    }

    if (Date.now() >= deadline) {
      throw new Error("cargo_host_budget_wait_timed_out");
    }
    if (!announcedWait) {
      process.stderr.write(
        `[cargo-lane] state=waiting tokens=${required} budget=${budget}\n`,
      );
      announcedWait = true;
    }
    await sleep(pollMs);
  }
}

function processGroupIsAlive(processGroupId) {
  if (process.platform === "win32") {
    throw new Error("windows_descendant_ownership_unavailable");
  }
  if (!Number.isSafeInteger(processGroupId) || processGroupId <= 0) {
    return false;
  }
  if (
    process.env.AUDIO_GRAPH_CARGO_LANE_TEST_MODE === "1" &&
    process.env.AUDIO_GRAPH_CARGO_LANE_TEST_FORCE_GROUP_ALIVE === "1"
  ) {
    return true;
  }
  try {
    process.kill(-processGroupId, 0);
    return true;
  } catch (error) {
    return !(error instanceof Error) || error.code !== "ESRCH";
  }
}

function signalChildTree(child, signal) {
  if (!child.pid) return;
  if (process.platform === "win32") {
    throw new Error("windows_descendant_ownership_unavailable");
  }

  try {
    process.kill(-child.pid, signal);
  } catch (error) {
    if (!(error instanceof Error) || error.code !== "ESRCH") throw error;
  }
}

async function waitForProcessGroupExit(processGroupId, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (!processGroupIsAlive(processGroupId)) return true;
    await sleep(10);
  }
  return !processGroupIsAlive(processGroupId);
}

async function stopSurvivingChildTree(child) {
  if (process.platform === "win32") {
    throw new Error("windows_descendant_ownership_unavailable");
  }
  if (!child.pid) {
    return { stopped: true, survivorsFound: false };
  }
  if (!processGroupIsAlive(child.pid)) {
    return { stopped: true, survivorsFound: false };
  }

  signalChildTree(child, "SIGTERM");
  if (await waitForProcessGroupExit(child.pid, 500)) {
    return { stopped: true, survivorsFound: true };
  }
  signalChildTree(child, "SIGKILL");
  return {
    stopped: await waitForProcessGroupExit(child.pid, 1000),
    survivorsFound: true,
  };
}

async function runChild(command, args, options, { tokens, staleMs }) {
  return await new Promise((resolveChild, rejectChild) => {
    const child = spawn(process.execPath, [scriptPath, registeredChildMode], {
      ...options,
      env: {
        ...options.env,
        [registeredChildCommandKey]: command,
        [registeredChildArgsKey]: JSON.stringify(args),
      },
      detached: process.platform !== "win32",
      shell: false,
      stdio: ["inherit", "inherit", "inherit", "ipc"],
    });
    let interruptedSignal = null;
    let leaseFailure = null;
    let startupFailure = null;
    let registered = false;
    let forceTimer = null;
    let heartbeatPending = false;
    let heartbeatWork = Promise.resolve();
    const heartbeatIntervalMs = Math.max(
      10,
      Math.min(30_000, Math.floor(staleMs / 3)),
    );
    const heartbeatTimer = setInterval(() => {
      if (!registered || heartbeatPending || child.exitCode !== null) return;
      heartbeatPending = true;
      heartbeatWork = updateTokenOwners(tokens, child.pid)
        .catch((error) => {
          stopForLeaseFailure(error);
        })
        .finally(() => {
          heartbeatPending = false;
        });
    }, heartbeatIntervalMs);
    heartbeatTimer.unref();

    const signalNumbers = { SIGINT: 2, SIGTERM: 15, SIGHUP: 1 };
    const signalHandlers = new Map();
    const scheduleForcedStop = () => {
      if (forceTimer) return;
      forceTimer = setTimeout(() => {
        try {
          signalChildTree(child, "SIGKILL");
        } catch {
          // The child tree already exited.
        }
      }, 5000);
      forceTimer.unref();
    };
    const stopForLeaseFailure = (error) => {
      if (leaseFailure) return;
      leaseFailure = error;
      try {
        signalChildTree(child, "SIGTERM");
      } catch {
        // The close/error path still audits descendants before release.
      }
      scheduleForcedStop();
    };
    for (const signal of Object.keys(signalNumbers)) {
      const handler = () => {
        if (interruptedSignal !== null) {
          try {
            signalChildTree(child, "SIGKILL");
          } catch {
            // The child may already have exited between signals.
          }
          return;
        }
        interruptedSignal = signal;
        try {
          signalChildTree(child, signal === "SIGHUP" ? "SIGTERM" : signal);
        } catch {
          // The close/error path below still releases the lease.
        }
        scheduleForcedStop();
      };
      signalHandlers.set(signal, handler);
      process.on(signal, handler);
    }

    const cleanup = () => {
      clearInterval(heartbeatTimer);
      if (forceTimer) clearTimeout(forceTimer);
      for (const [signal, handler] of signalHandlers) {
        process.removeListener(signal, handler);
      }
    };

    let registrationReadySettled = false;
    let resolveRegistrationReady;
    const registrationReady = new Promise((resolveReady) => {
      resolveRegistrationReady = resolveReady;
    });
    const settleRegistrationReady = (ready) => {
      if (registrationReadySettled) return;
      registrationReadySettled = true;
      resolveRegistrationReady(ready);
    };
    child.on("message", (message) => {
      if (message?.type === "ready") {
        settleRegistrationReady(true);
      } else if (
        message?.type === "startup-error" &&
        typeof message.label === "string" &&
        /^[A-Za-z0-9_]+$/.test(message.label)
      ) {
        startupFailure = new Error(message.label);
      }
    });
    const registrationWork = registrationReady.then(async (ready) => {
      if (!ready) {
        if (interruptedSignal === null && !leaseFailure) {
          leaseFailure = new Error("cargo_registration_barrier_closed");
        }
        return;
      }
      try {
        await updateTokenOwners(tokens, child.pid);
        if (!child.connected) {
          throw new Error("cargo_registration_barrier_closed");
        }
        await new Promise((resolveSend, rejectSend) => {
          child.send({ type: "start" }, (error) => {
            if (error) {
              rejectSend(error);
              return;
            }
            resolveSend();
          });
        });
        registered = true;
      } catch (error) {
        stopForLeaseFailure(error);
      }
    });

    child.once("error", (error) => {
      settleRegistrationReady(false);
      if (!child.pid) {
        cleanup();
        rejectChild(error);
        return;
      }
      stopForLeaseFailure(error);
    });
    child.once("close", (code, signal) => {
      settleRegistrationReady(false);
      cleanup();
      void Promise.all([registrationWork, heartbeatWork])
        .then(() => stopSurvivingChildTree(child))
        .then(({ stopped, survivorsFound }) => {
          if (!stopped) {
            rejectChild(
              retainTokenLease(new Error("cargo_descendant_cleanup_failed")),
            );
            return;
          }
          if (survivorsFound && interruptedSignal === null) {
            rejectChild(new Error("cargo_descendants_survived_parent"));
            return;
          }
          if (startupFailure) {
            rejectChild(startupFailure);
            return;
          }
          if (leaseFailure) {
            rejectChild(leaseFailure);
            return;
          }
          if (interruptedSignal !== null) {
            resolveChild(128 + signalNumbers[interruptedSignal]);
            return;
          }
          if (typeof code === "number") {
            resolveChild(code);
            return;
          }
          resolveChild(signal === "SIGINT" ? 130 : 1);
        })
        .catch((error) => rejectChild(retainTokenLease(error)));
    });
  });
}

async function main(argv = process.argv.slice(2), env = process.env) {
  const [mode, ...extraArgs] = argv;
  const supportedModes = new Set([
    "cloud-check",
    "cloud-test",
    "full-check",
    "full-test",
    "clean-room-check",
  ]);
  if (
    !supportedModes.has(mode) ||
    ((mode === "cloud-check" ||
      mode === "full-check" ||
      mode === "clean-room-check") &&
      extraArgs.length !== 0) ||
    ((mode === "cloud-test" || mode === "full-test") && extraArgs.length > 1)
  ) {
    return fail("usage_cloud-check_cloud-test_or_full-check");
  }
  if (
    mode.endsWith("-test") &&
    extraArgs.length === 1 &&
    (extraArgs[0].length === 0 ||
      extraArgs[0].startsWith("-") ||
      extraArgs[0].includes("\0"))
  ) {
    return fail("test_filter_must_be_a_literal_non_flag_argument");
  }

  try {
    if (
      process.platform === "win32" ||
      (env.AUDIO_GRAPH_CARGO_LANE_TEST_MODE === "1" &&
        env.AUDIO_GRAPH_CARGO_LANE_TEST_PLATFORM === "win32")
    ) {
      throw new Error("windows_descendant_ownership_unavailable");
    }
    const detectedCpus =
      env.AUDIO_GRAPH_CARGO_LANE_TEST_MODE === "1" &&
      env.AUDIO_GRAPH_CARGO_LANE_TEST_DETECTED_CPUS
        ? positiveInteger(
            "AUDIO_GRAPH_CARGO_LANE_TEST_DETECTED_CPUS",
            env.AUDIO_GRAPH_CARGO_LANE_TEST_DETECTED_CPUS,
            null,
          )
        : Math.max(1, availableParallelism());
    const budget = positiveInteger(
      "AUDIO_GRAPH_CARGO_BUDGET",
      env.AUDIO_GRAPH_CARGO_BUDGET,
      Math.min(6, detectedCpus),
    );
    if (budget > detectedCpus) {
      throw new Error("cargo_budget_exceeds_detected_cpus");
    }
    const configuredJobs = positiveInteger(
      "AUDIO_GRAPH_CARGO_JOBS",
      env.AUDIO_GRAPH_CARGO_JOBS,
      null,
    );
    if (configuredJobs !== null && configuredJobs > budget) {
      throw new Error("cargo_jobs_exceed_host_budget");
    }
    const pollMs = positiveInteger(
      "AUDIO_GRAPH_CARGO_POLL_MS",
      env.AUDIO_GRAPH_CARGO_POLL_MS,
      250,
    );
    const waitMs = positiveInteger(
      "AUDIO_GRAPH_CARGO_WAIT_MS",
      env.AUDIO_GRAPH_CARGO_WAIT_MS,
      30 * 60 * 1000,
    );
    const staleMs = positiveInteger(
      "AUDIO_GRAPH_CARGO_STALE_MS",
      env.AUDIO_GRAPH_CARGO_STALE_MS,
      2 * 60 * 1000,
    );
    const adaptiveWindowMs = positiveInteger(
      "AUDIO_GRAPH_CARGO_ADAPTIVE_WINDOW_MS",
      env.AUDIO_GRAPH_CARGO_ADAPTIVE_WINDOW_MS,
      100,
    );

    const configuredRoot = env.AUDIO_GRAPH_CARGO_WORKTREE_ROOT ?? scriptRoot;
    const worktreeRoot = await realpath(configuredRoot);
    const targetRoot = resolve(
      env.AUDIO_GRAPH_CARGO_TARGET_ROOT ??
        join(worktreeRoot, "src-tauri", "target", "cargo-lanes"),
    );
    const featureLane = mode.startsWith("cloud-")
      ? "features-cloud"
      : "features-default";
    const cleanRoomMode = mode === "clean-room-check";
    const exclusiveMode = mode.startsWith("full-") || cleanRoomMode;
    if (
      configuredJobs === null &&
      !exclusiveMode &&
      adaptiveWindowMs > waitMs
    ) {
      throw new Error("cargo_adaptive_window_exceeds_wait");
    }
    const coordinationRoot = resolve(
      env.AUDIO_GRAPH_CARGO_COORDINATION_DIR ??
        join(tmpdir(), "audio-graph-cargo-budget-v1"),
    );
    const jobs =
      configuredJobs ??
      (exclusiveMode
        ? budget
        : await adaptiveJobs({
            coordinationRoot,
            budget,
            adaptiveWindowMs,
            pollMs,
            staleMs,
            waitMs,
          }));
    let targetDir = null;
    if (!cleanRoomMode) {
      targetDir = join(
        targetRoot,
        `worktree-${worktreeIdentity(worktreeRoot)}`,
        featureLane,
        "profile-debug",
      );
      await mkdir(targetDir, { recursive: true });
    }

    const cargoBin = env.AUDIO_GRAPH_CARGO_BIN ?? "cargo";
    const cargoOperation = mode.endsWith("-test") ? "test" : "check";
    const args = [...cargoPrefix(env), "+1.95.0", cargoOperation, "--locked"];
    if (featureLane === "features-cloud") {
      args.push(
        "-p",
        "audio-graph",
        "--lib",
        "--no-default-features",
        "--features",
        "cloud",
      );
    } else if (cargoOperation === "check") {
      args.push("--all-targets");
    }
    args.push("--jobs", String(jobs));
    if (mode.endsWith("-test")) {
      if (extraArgs.length === 1) args.push(extraArgs[0]);
      args.push("--", "--test-threads=1");
    }

    process.stderr.write(
      `[cargo-lane] mode=${exclusiveMode ? "exclusive" : "shared"} lane=${featureLane}/profile-debug jobs=${jobs} budget=${budget}\n`,
    );
    const tokens = await acquireTokens({
      coordinationRoot,
      budget,
      required: exclusiveMode ? budget : jobs,
      pollMs,
      staleMs,
      waitMs,
    });
    let tokensAreSafeToRelease = true;
    try {
      if (cleanRoomMode) {
        const cleanRoomRoot = resolve(
          env.AUDIO_GRAPH_CARGO_TEMP_ROOT ?? tmpdir(),
        );
        await mkdir(cleanRoomRoot, { recursive: true, mode: 0o700 });
        targetDir = await mkdtemp(
          join(cleanRoomRoot, "audio-graph-cargo-clean-room-default-debug-"),
        );
        process.stderr.write(`[cargo-lane] clean_room_target=${targetDir}\n`);
      }
      return await runChild(
        cargoBin,
        args,
        {
          cwd: join(worktreeRoot, "src-tauri"),
          env: { ...env, CARGO_TARGET_DIR: targetDir },
        },
        { tokens, staleMs },
      );
    } catch (error) {
      if (error?.retainCargoTokens === true) {
        tokensAreSafeToRelease = false;
      }
      throw error;
    } finally {
      if (tokensAreSafeToRelease) {
        await releaseTokens(tokens);
      }
    }
  } catch (error) {
    return fail(safeErrorLabel(error));
  }
}

process.exitCode =
  process.argv[2] === registeredChildMode
    ? await registeredChildMain()
    : await main();
