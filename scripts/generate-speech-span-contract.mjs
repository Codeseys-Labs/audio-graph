import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(scriptDir, "..");
const tauriDir = join(repoRoot, "src-tauri");
const outputPath = join(repoRoot, "src", "generated", "speechSpanRevision.ts");
const cargo = process.env.CARGO ?? (process.platform === "win32" ? "cargo.exe" : "cargo");
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
    "+1.95.0",
    "run",
    "--locked",
    "-p",
    "audio-graph-ipc-contract",
    "--bin",
    "export_speech_span_revision",
    "--",
    ...(check ? ["--check"] : []),
    outputPath,
  ],
  {
    cwd: tauriDir,
    stdio: "inherit",
    shell: false,
    env: {
      ...process.env,
      CARGO_TARGET_DIR:
        process.env.CARGO_TARGET_DIR ?? join(tauriDir, "target", "speech-span-contract"),
    },
  },
);

if (result.error) {
  throw result.error;
}

process.exit(result.status ?? 1);
