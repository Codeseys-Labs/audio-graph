#!/usr/bin/env node
/**
 * Frontend flake check: run the vitest suite N times, optionally with CPU load
 * and shuffled order, and report every test that failed at least once (#523,
 * #548).
 *
 *   bun run test:flake                         # 3 runs, default workers
 *   bun run test:flake -- --runs 5 --load --shuffle
 *   bun run test:flake -- --runs 5 --load 16 --max-workers 8 --shuffle
 *   bun run test:flake -- --runs 3 src/components/SessionsBrowser   # file filter
 *
 * Options (everything after the options is passed to vitest as a file filter):
 *   --runs N          number of full runs (default 3)
 *   --load [K]        run K CPU-burning processes alongside (default: one per
 *                     core), to reproduce the starvation other agents' builds
 *                     cause on a shared machine
 *   --shuffle         shuffle file and test order each run (seed printed, so a
 *                     failing order can be replayed with `--seed`)
 *   --seed S          base seed for --shuffle (default: time based); run i uses S+i
 *   --max-workers W   vitest `--maxWorkers`
 *   --slow MS         also list tests that passed but took at least MS (default 2000)
 *   --top N           how many slow tests to list (default 15)
 *   --exclude GLOB    forwarded to vitest `--exclude` (repeatable)
 *
 * Exit status is 1 when any test failed in any run. A test that failed in some
 * runs and passed in others is listed as FLAKY; one that failed in every run as
 * BROKEN.
 */
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { cpus, tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const vitestCli = fileURLToPath(
  new URL("../node_modules/vitest/vitest.mjs", import.meta.url),
);

function parseArgs(argv) {
  const options = {
    runs: 3,
    load: 0,
    shuffle: false,
    seed: Date.now() % 1_000_000,
    maxWorkers: null,
    slowMs: 2000,
    top: 15,
    exclude: [],
    filters: [],
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = argv[i + 1];
    if (arg === "--runs") options.runs = Number(argv[++i]);
    else if (arg === "--load") {
      options.load =
        next !== undefined && /^\d+$/.test(next)
          ? Number(argv[++i])
          : cpus().length;
    } else if (arg === "--shuffle") options.shuffle = true;
    else if (arg === "--seed") options.seed = Number(argv[++i]);
    else if (arg === "--max-workers") options.maxWorkers = Number(argv[++i]);
    else if (arg === "--slow") options.slowMs = Number(argv[++i]);
    else if (arg === "--top") options.top = Number(argv[++i]);
    else if (arg === "--exclude") options.exclude.push(argv[++i]);
    else if (arg === "--") continue;
    else options.filters.push(arg);
  }
  if (!Number.isInteger(options.runs) || options.runs < 1) {
    throw new Error("--runs must be a positive integer");
  }
  return options;
}

function startLoad(count) {
  const burners = [];
  for (let i = 0; i < count; i += 1) {
    burners.push(
      spawn(process.execPath, ["-e", "for(;;){}"], { stdio: "ignore" }),
    );
  }
  return () => {
    for (const burner of burners) burner.kill();
  };
}

/** First informative line of a failure: vitest leads some with a bare STACK_TRACE_ERROR. */
function firstMessageLine(messages) {
  const lines = (messages ?? []).flatMap((m) => m.split("\n"));
  return (
    lines.find((l) => l.trim() !== "" && !/STACK_TRACE_ERROR/.test(l)) ??
    lines[0] ??
    ""
  );
}

function runOnce(options, run, workdir) {
  const outputFile = join(workdir, `run-${run}.json`);
  const args = [
    vitestCli,
    "run",
    "--reporter=json",
    `--outputFile=${outputFile}`,
  ];
  if (options.maxWorkers !== null) args.push(`--maxWorkers=${options.maxWorkers}`);
  const seed = options.seed + run;
  if (options.shuffle) {
    args.push("--sequence.shuffle.files", "--sequence.shuffle.tests");
    args.push(`--sequence.seed=${seed}`);
  }
  for (const glob of options.exclude) args.push("--exclude", glob);
  args.push(...options.filters);
  const started = Date.now();
  const result = spawnSync(process.execPath, args, {
    stdio: ["ignore", "ignore", "inherit"],
    maxBuffer: 1 << 28,
  });
  const seconds = ((Date.now() - started) / 1000).toFixed(0);
  let report;
  try {
    report = JSON.parse(readFileSync(outputFile, "utf8"));
  } catch {
    throw new Error(
      `run ${run + 1}: vitest produced no JSON report (exit ${result.status})`,
    );
  }
  return { report, seconds, seed };
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  const workdir = mkdtempSync(join(tmpdir(), "vitest-flake-"));
  const stopLoad = options.load > 0 ? startLoad(options.load) : () => {};
  /** test id -> { failures, runs, messages, maxMs } */
  const tests = new Map();
  const perRun = [];
  console.log(
    `flake check: ${options.runs} run(s), load=${options.load}, shuffle=${options.shuffle}` +
      (options.shuffle ? ` (seeds ${options.seed}..${options.seed + options.runs - 1})` : "") +
      (options.maxWorkers ? `, maxWorkers=${options.maxWorkers}` : ""),
  );
  try {
    for (let run = 0; run < options.runs; run += 1) {
      const { report, seconds, seed } = runOnce(options, run, workdir);
      let failed = 0;
      let passed = 0;
      for (const file of report.testResults) {
        const relative = file.name.replace(/\\/g, "/").replace(/^.*\/src\//, "src/");
        if (file.assertionResults.length === 0 && file.status === "failed") {
          // A file that failed to load: count it as one failing "test".
          const id = `${relative} :: (file failed to run)`;
          const entry = tests.get(id) ?? { failures: 0, runs: 0, messages: [], maxMs: 0 };
          entry.runs += 1;
          entry.failures += 1;
          entry.lastFailedRun = run;
          entry.messages.push((file.message ?? "").split("\n")[0]);
          tests.set(id, entry);
          failed += 1;
          continue;
        }
        for (const test of file.assertionResults) {
          const id = `${relative} :: ${test.fullName}`;
          const entry = tests.get(id) ?? { failures: 0, runs: 0, messages: [], maxMs: 0 };
          entry.runs += 1;
          entry.maxMs = Math.max(entry.maxMs, test.duration ?? 0);
          if (test.status === "failed") {
            entry.lastFailedRun = run;
            entry.failures += 1;
            entry.messages.push(firstMessageLine(test.failureMessages));
            failed += 1;
          } else if (test.status === "passed") {
            passed += 1;
          }
          tests.set(id, entry);
        }
      }
      perRun.push({ run: run + 1, seconds, seed, passed, failed });
      console.log(
        `  run ${run + 1}: ${passed} passed, ${failed} failed, ${seconds}s` +
          (options.shuffle ? ` (seed ${seed})` : ""),
      );
      // Incremental: name this run's failures now, so a long session that is
      // interrupted still leaves a usable record.
      for (const [id, entry] of tests) {
        if (entry.lastFailedRun === run) console.log(`      failed: ${id}`);
      }
    }
  } finally {
    stopLoad();
    rmSync(workdir, { recursive: true, force: true });
  }

  const failing = [...tests.entries()].filter(([, e]) => e.failures > 0);
  const slow = [...tests.entries()]
    .filter(([, e]) => e.failures === 0 && e.maxMs >= options.slowMs)
    .sort((a, b) => b[1].maxMs - a[1].maxMs);

  console.log("");
  if (failing.length === 0) {
    console.log(`no test failed in ${options.runs} run(s)`);
  } else {
    console.log(`${failing.length} test(s) failed at least once:`);
    for (const [id, entry] of failing.sort((a, b) => b[1].failures - a[1].failures)) {
      const kind = entry.failures === entry.runs ? "BROKEN" : "FLAKY ";
      console.log(`  ${kind} ${entry.failures}/${entry.runs}  ${id}`);
      console.log(`           ${[...new Set(entry.messages)][0] ?? ""}`);
    }
  }
  if (slow.length > 0) {
    console.log(`\npassed but took >= ${options.slowMs} ms at least once (top ${options.top}):`);
    for (const [id, entry] of slow.slice(0, options.top)) {
      console.log(`  ${String(Math.round(entry.maxMs)).padStart(6)} ms  ${id}`);
    }
  }
  process.exitCode = failing.length > 0 ? 1 : 0;
}

main();
