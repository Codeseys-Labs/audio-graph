import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import test from "node:test";

const repoRoot = fileURLToPath(new URL("../", import.meta.url));
const launcher = join(repoRoot, "scripts", "run-vitest-local.mjs");

test("local Vitest runs without Node Web Storage and preserves NODE_OPTIONS", (t) => {
  const fixtureDir = mkdtempSync(join(tmpdir(), "audio-graph-vitest-local-"));
  t.after(() => rmSync(fixtureDir, { force: true, recursive: true }));

  const fixture = join(fixtureDir, "launcher-probe.test.js");
  writeFileSync(
    fixture,
    `test("uses the JSDOM-compatible Node runtime", () => {
  expect("localStorage" in globalThis).toBe(false);
  expect(process.env.NODE_OPTIONS).toContain("--stack-trace-limit=37");
  expect(process.env.NODE_OPTIONS).toContain("--no-experimental-webstorage");
});\n`,
  );

  const result = spawnSync(
    process.execPath,
    [launcher, "--root", fixtureDir, "--globals", fixture],
    {
      cwd: repoRoot,
      encoding: "utf8",
      env: { ...process.env, NODE_OPTIONS: "--stack-trace-limit=37" },
    },
  );

  assert.equal(
    result.status,
    0,
    `launcher failed:\n${result.stdout}\n${result.stderr}`,
  );
  assert.match(result.stdout, /1 passed/);
});

test("local Vitest forwards assertion failures", (t) => {
  const fixtureDir = mkdtempSync(join(tmpdir(), "audio-graph-vitest-local-"));
  t.after(() => rmSync(fixtureDir, { force: true, recursive: true }));

  const fixture = join(fixtureDir, "launcher-failure.test.js");
  writeFileSync(
    fixture,
    `test("reports a real assertion failure", () => {
  expect("actual").toBe("expected");
});\n`,
  );

  const result = spawnSync(
    process.execPath,
    [launcher, "--root", fixtureDir, "--globals", fixture],
    { cwd: repoRoot, encoding: "utf8" },
  );

  assert.notEqual(result.status, 0);
  assert.match(`${result.stdout}\n${result.stderr}`, /1 failed/);
});
