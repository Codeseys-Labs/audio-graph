import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const launchers = readdirSync(scriptDir)
  .filter((name) => name.startsWith("generate-") && name.endsWith(".mjs"))
  .sort();

const failures = [];
for (const launcher of launchers) {
  const source = readFileSync(join(scriptDir, launcher), "utf8");
  for (const [label, invariant] of [
    ["CARGO override", "process.env.CARGO ??"],
    ["Windows executable", 'process.platform === "win32" ? "cargo.exe" : "cargo"'],
    ["locked dependency graph", '"--locked"'],
    ["non-shell spawn", "shell: false"],
  ]) {
    if (!source.includes(invariant)) {
      failures.push(`${launcher}: missing ${label}`);
    }
  }
}

if (failures.length > 0) {
  console.error(failures.join("\n"));
  process.exit(1);
}

console.log(`generator launcher invariants verified (${launchers.length} files)`);
