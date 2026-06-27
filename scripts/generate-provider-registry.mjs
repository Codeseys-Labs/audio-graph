import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(scriptDir, "..");
const tauriDir = join(repoRoot, "src-tauri");
const outputPath = join(repoRoot, "src", "generated", "providerRegistry.ts");
const cargo = process.platform === "win32" ? "cargo.cmd" : "cargo";
const args = process.argv.slice(2);
const check = args.includes("--check");

const unknownArgs = args.filter((arg) => arg !== "--check");
if (unknownArgs.length > 0) {
  console.error(`Unknown argument(s): ${unknownArgs.join(", ")}`);
  process.exit(2);
}

const result = spawnSync(
  cargo,
  [
    "run",
    "-p",
    "audio-graph-provider-registry",
    "--bin",
    "export-provider-registry",
    "--",
    ...(check ? ["--check"] : []),
    outputPath,
  ],
  {
    cwd: tauriDir,
    stdio: "inherit",
    shell: false,
  },
);

if (result.error) {
  throw result.error;
}

process.exit(result.status ?? 1);
