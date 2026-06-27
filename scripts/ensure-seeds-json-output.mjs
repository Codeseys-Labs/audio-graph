import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(scriptDir, "..");
const args = process.argv.slice(2);
const check = args.includes("--check");
const fix = args.includes("--fix");
const stress = args.includes("--stress");
const supportedPatchVersions = new Set(["0.4.5"]);
const unknownArgs = args.filter(
  (arg) => !["--check", "--fix", "--stress"].includes(arg),
);

if (unknownArgs.length > 0) {
  console.error(`Unknown argument(s): ${unknownArgs.join(", ")}`);
  process.exit(2);
}

if (check && fix) {
  console.error("Use either --check or --fix, not both.");
  process.exit(2);
}

function run(command, commandArgs, options = {}) {
  const result = spawnSync(command, commandArgs, {
    cwd: options.cwd ?? repoRoot,
    encoding: "utf8",
    shell: false,
    stdio: options.stdio ?? "pipe",
    maxBuffer: options.maxBuffer ?? 16 * 1024 * 1024,
  });

  if (result.error) {
    throw result.error;
  }

  return result;
}

function readPackageJson(candidate) {
  const packagePath = join(candidate, "package.json");
  return JSON.parse(readFileSync(packagePath, "utf8"));
}

function resolveSdBin() {
  const lookupCommand = process.platform === "win32" ? "where" : "which";
  const result = run(lookupCommand, ["sd"], { cwd: repoRoot });

  if (result.status !== 0 || !result.stdout.trim()) {
    throw new Error("Could not find `sd` on PATH.");
  }

  return result.stdout.trim().split(/\r?\n/)[0];
}

function isSeedsCliRoot(candidate) {
  if (!candidate) {
    return false;
  }

  const packagePath = join(candidate, "package.json");
  if (!existsSync(packagePath) || !existsSync(join(candidate, "src", "output.ts"))) {
    return false;
  }

  try {
    const packageJson = readPackageJson(candidate);
    return packageJson.name === "@os-eco/seeds-cli";
  } catch {
    return false;
  }
}

function resolveSeedsCliRoots() {
  const candidates = [
    resolve(repoRoot, "node_modules", "@os-eco", "seeds-cli"),
    process.env.SEEDS_CLI_ROOT ? resolve(process.env.SEEDS_CLI_ROOT) : null,
  ];

  let binPath = null;
  try {
    const sdBin = resolveSdBin();
    binPath = realpathSync(sdBin);
    const binDir = dirname(binPath);
    if (binDir.endsWith(`${join("@os-eco", "seeds-cli", "src")}`)) {
      candidates.push(dirname(binDir));
    }

    candidates.push(resolve(binDir, ".."));
    candidates.push(
      resolve(dirname(sdBin), "..", "install", "global", "node_modules", "@os-eco", "seeds-cli"),
    );
  } catch {
    // A repo-local devDependency is enough; global sd is only a fallback.
  }

  if (process.env.BUN_INSTALL) {
    candidates.push(
      resolve(
        process.env.BUN_INSTALL,
        "install",
        "global",
        "node_modules",
        "@os-eco",
        "seeds-cli",
      ),
    );
  }

  const bunBinResult = run("bun", ["pm", "bin", "-g"], { cwd: repoRoot });
  if (bunBinResult.status === 0 && bunBinResult.stdout.trim()) {
    candidates.push(
      resolve(
        bunBinResult.stdout.trim(),
        "..",
        "install",
        "global",
        "node_modules",
        "@os-eco",
        "seeds-cli",
      ),
    );
  }

  const roots = [];
  const seen = new Set();

  for (const candidate of candidates) {
    if (!isSeedsCliRoot(candidate)) {
      continue;
    }

    const realpath = realpathSync(candidate);
    if (seen.has(realpath)) {
      continue;
    }

    seen.add(realpath);
    roots.push(realpath);
  }

  if (roots.length > 0) {
    return roots;
  }

  throw new Error(
    `Could not find @os-eco/seeds-cli in node_modules or infer it from ${
      binPath ?? "PATH"
    }. Run \`bun install\` or set SEEDS_CLI_ROOT.`,
  );
}

function assertCanPatch(seedsRoot) {
  const packageJson = readPackageJson(seedsRoot);
  if (!supportedPatchVersions.has(packageJson.version)) {
    throw new Error(
      `Refusing to patch @os-eco/seeds-cli ${packageJson.version}. ` +
        `Supported versions: ${[...supportedPatchVersions].join(", ")}. ` +
        "Update scripts/ensure-seeds-json-output.mjs for this version.",
    );
  }
}

function findMatchingBrace(source, openBraceIndex) {
  let depth = 0;
  for (let index = openBraceIndex; index < source.length; index += 1) {
    const char = source[index];
    if (char === "{") {
      depth += 1;
    } else if (char === "}") {
      depth -= 1;
      if (depth === 0) {
        return index;
      }
    }
  }

  return -1;
}

function outputJsonIsPatched(source) {
  return (
    source.includes('import { writeSync } from "node:fs";') &&
    source.includes("const payload = Buffer.from") &&
    source.includes("writeSync(1, payload") &&
    source.includes('code !== "EAGAIN"')
  );
}

function patchedOutputJsonFunction() {
  return `export function outputJson(data: unknown): void {
\tconst payload = Buffer.from(\`\${JSON.stringify(data, null, 2)}\\n\`);
\tconst retry = new Int32Array(new SharedArrayBuffer(4));
\tlet offset = 0;
\twhile (offset < payload.length) {
\t\ttry {
\t\t\tconst written = writeSync(1, payload, offset, payload.length - offset);
\t\t\tif (written === 0) {
\t\t\t\tAtomics.wait(retry, 0, 0, 1);
\t\t\t\tcontinue;
\t\t\t}
\t\t\toffset += written;
\t\t} catch (error) {
\t\t\tif ((error as { code?: string }).code !== "EAGAIN") throw error;
\t\t\tAtomics.wait(retry, 0, 0, 1);
\t\t}
\t}
}`;
}

function patchOutputJson(source) {
  let next = source;
  if (!next.includes('import { writeSync } from "node:fs";')) {
    const importMatches = [...next.matchAll(/^import .*;$/gm)];
    if (importMatches.length === 0) {
      throw new Error("Could not find import block in Seeds output.ts.");
    }

    const lastImport = importMatches[importMatches.length - 1];
    const insertAt = (lastImport.index ?? 0) + lastImport[0].length;
    next = `${next.slice(0, insertAt)}\nimport { writeSync } from "node:fs";${next.slice(insertAt)}`;
  }

  const functionStart = next.indexOf("export function outputJson(data: unknown): void ");
  if (functionStart < 0) {
    throw new Error("Could not find outputJson function in Seeds output.ts.");
  }

  const openBrace = next.indexOf("{", functionStart);
  const closeBrace = findMatchingBrace(next, openBrace);
  if (openBrace < 0 || closeBrace < 0) {
    throw new Error("Could not parse outputJson function body in Seeds output.ts.");
  }

  return `${next.slice(0, functionStart)}${patchedOutputJsonFunction()}${next.slice(
    closeBrace + 1,
  )}`;
}

function assertSeedsEnvelope(parsed, label) {
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${label} did not produce a JSON object envelope.`);
  }

  if (parsed.success !== true) {
    throw new Error(`${label} did not report success: true.`);
  }

  if (typeof parsed.command !== "string") {
    throw new Error(`${label} did not include a string command field.`);
  }

  if (!Array.isArray(parsed.issues)) {
    throw new Error(`${label} did not include an issues array.`);
  }

  if (Object.hasOwn(parsed, "items")) {
    throw new Error(`${label} unexpectedly included items; parse issue rows from issues.`);
  }

  if (typeof parsed.count !== "number") {
    throw new Error(`${label} did not include a numeric count.`);
  }

  if (parsed.count !== parsed.issues.length) {
    throw new Error(
      `${label} count ${parsed.count} did not match issues length ${parsed.issues.length}.`,
    );
  }
}

function stressParseSeedsJson(seedsRoot) {
  const commands = [
    ["ready", "--format", "json"],
    ["blocked", "--format", "json"],
    ["list", "--format", "json"],
  ];
  const seedsEntry = join(seedsRoot, "src", "index.ts");

  for (const commandArgs of commands) {
    const result = run("bun", [seedsEntry, ...commandArgs], { cwd: repoRoot });
    const label = `sd ${commandArgs.join(" ")}`;
    if (result.status !== 0) {
      throw new Error(`${label} failed:\n${result.stderr}`);
    }

    try {
      const parsed = JSON.parse(result.stdout);
      assertSeedsEnvelope(parsed, label);
      const count = parsed.count;
      console.log(`${label}: parsed (${count})`);
    } catch (error) {
      throw new Error(`${label} produced invalid JSON: ${error.message}`);
    }
  }
}

for (const seedsRoot of resolveSeedsCliRoots()) {
  const outputPath = join(seedsRoot, "src", "output.ts");

  if (!existsSync(outputPath)) {
    throw new Error(`Seeds output.ts not found at ${outputPath}`);
  }

  const source = readFileSync(outputPath, "utf8");
  const isPatched = outputJsonIsPatched(source);

  if (!isPatched && check) {
    console.error(
      `Seeds CLI outputJson does not have the pipe-safe stdout retry patch: ${outputPath}`,
    );
    process.exit(1);
  }

  if (!isPatched && fix) {
    assertCanPatch(seedsRoot);
    writeFileSync(outputPath, patchOutputJson(source));
    console.log(`Patched Seeds CLI outputJson for pipe-safe JSON output: ${outputPath}`);
  } else if (isPatched) {
    console.log(`Seeds CLI outputJson patch present: ${outputPath}`);
  } else {
    console.error(
      `Seeds CLI outputJson patch is missing. Run \`bun run prepare:seeds-json-output\`.`,
    );
    process.exit(1);
  }

  if (stress) {
    stressParseSeedsJson(seedsRoot);
  }
}
