import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const disableNodeWebStorage = "--no-experimental-webstorage";
const existingNodeOptions = process.env.NODE_OPTIONS?.trim();
const nodeOptions = existingNodeOptions
  ? `${existingNodeOptions} ${disableNodeWebStorage}`
  : disableNodeWebStorage;
const vitestCli = fileURLToPath(
  new URL("../node_modules/vitest/vitest.mjs", import.meta.url),
);

const result = spawnSync(
  process.execPath,
  [vitestCli, "run", "--maxWorkers=1", ...process.argv.slice(2)],
  {
    env: { ...process.env, NODE_OPTIONS: nodeOptions },
    stdio: "inherit",
  },
);

if (result.error) {
  console.error(result.error.message);
}

process.exitCode = result.status ?? 1;
