#!/usr/bin/env node
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const args = process.argv.slice(2);
const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const issuesPath = path.resolve(scriptDir, "..", ".seeds", "issues.jsonl");

function usage() {
  console.error(`Usage: bun scripts/sd-issues.mjs <sd-command> [sd-options]

Runs \`sd <command> ... --format json\`, validates the Seeds JSON envelope,
and prints only the issue rows from \`.issues\` as a JSON array.

Special modes:
  ready-all              Read .seeds/issues.jsonl directly and emit the full
                         open/unblocked planning queue without the sd ready cap.

Examples:
  bun scripts/sd-issues.mjs ready
  bun scripts/sd-issues.mjs ready-all
  bun scripts/sd-issues.mjs blocked
  bun scripts/sd-issues.mjs list --all --priority 1`);
}

if (
  args.length === 0 ||
  args.includes("-h") ||
  args.includes("--help")
) {
  usage();
  process.exit(args.length === 0 ? 2 : 0);
}

function fail(message) {
  console.error(message);
  process.exit(1);
}

function isBrokenPipe(error) {
  return (
    error !== null &&
    typeof error === "object" &&
    (error.code === "EPIPE" ||
      error.errno === -32 ||
      /EPIPE|broken pipe/i.test(error.message ?? ""))
  );
}

function writeStdout(text) {
  return new Promise((resolve, reject) => {
    try {
      process.stdout.write(text, (error) => {
        if (isBrokenPipe(error)) {
          resolve();
          return;
        }

        if (error) {
          reject(error);
          return;
        }

        resolve();
      });
    } catch (error) {
      if (isBrokenPipe(error)) {
        resolve();
        return;
      }

      reject(error);
    }
  });
}

function readValidatedIssuesJsonl() {
  let source;
  try {
    source = readFileSync(issuesPath, "utf8");
  } catch (error) {
    fail(`Failed to read ${issuesPath}: ${error.message}`);
  }

  const rawLines = source.split(/\r?\n/);
  const issues = [];

  for (const [index, rawLine] of rawLines.entries()) {
    const lineNumber = index + 1;
    const line = rawLine.trim();

    if (line.length === 0) {
      continue;
    }

    let parsed;
    try {
      parsed = JSON.parse(line);
    } catch (error) {
      fail(`${issuesPath}:${lineNumber} did not contain valid JSON: ${error.message}`);
    }

    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
      fail(`${issuesPath}:${lineNumber} must contain a JSON object.`);
    }

    if (typeof parsed.id !== "string" || parsed.id.length === 0) {
      fail(`${issuesPath}:${lineNumber} is missing a non-empty string id.`);
    }

    if (typeof parsed.status !== "string" || parsed.status.length === 0) {
      fail(`${issuesPath}:${lineNumber} is missing a non-empty string status.`);
    }

    issues.push(parsed);
  }

  return issues;
}

function isResolvedDependency(dependency, issuesById) {
  if (typeof dependency === "string") {
    const blocker = issuesById.get(dependency);
    return blocker !== undefined && blocker.status === "closed";
  }

  if (dependency && typeof dependency === "object" && !Array.isArray(dependency)) {
    if (dependency.status === "closed") {
      return true;
    }

    if (typeof dependency.id === "string" && dependency.id.length > 0) {
      const blocker = issuesById.get(dependency.id);
      return blocker !== undefined && blocker.status === "closed";
    }
  }

  return false;
}

function readyAllIssues() {
  const issues = readValidatedIssuesJsonl();
  const issuesById = new Map(issues.map((issue) => [issue.id, issue]));

  return issues.filter((issue) => {
    if (issue.status !== "open") {
      return false;
    }

    const blockers = issue.blockedBy;
    if (blockers === undefined) {
      return true;
    }

    if (!Array.isArray(blockers)) {
      fail(`${issuesPath} issue ${issue.id} has a non-array blockedBy field.`);
    }

    return blockers.every((blocker) => isResolvedDependency(blocker, issuesById));
  });
}

if (args[0] === "ready-all") {
  if (args.length !== 1) {
    console.error("ready-all does not accept additional arguments.");
    process.exit(2);
  }

  await writeStdout(`${JSON.stringify(readyAllIssues(), null, 2)}\n`);
  process.exit(0);
}

if (
  args.includes("--json") ||
  args.includes("--format") ||
  args.some((arg) => arg.startsWith("--format="))
) {
  console.error(
    "Do not pass --json or --format to sd-issues; it always requests the JSON envelope.",
  );
  process.exit(2);
}

const result = spawnSync("sd", [...args, "--format", "json"], {
  encoding: "utf8",
  shell: false,
  stdio: ["ignore", "pipe", "pipe"],
  maxBuffer: 32 * 1024 * 1024,
});

if (result.error) {
  fail(result.error.message);
}

if (result.status !== 0) {
  if (result.stderr.trim()) {
    process.stderr.write(result.stderr);
  }
  if (result.stdout.trim()) {
    process.stderr.write(result.stdout);
  }
  process.exit(result.status ?? 1);
}

let parsed;
try {
  parsed = JSON.parse(result.stdout);
} catch (error) {
  fail(`sd ${args.join(" ")} did not produce valid JSON: ${error.message}`);
}

const label = `sd ${args.join(" ")}`;
if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
  fail(`${label} did not produce a JSON object envelope.`);
}

if (parsed.success !== true) {
  fail(`${label} did not report success: true.`);
}

if (!Array.isArray(parsed.issues)) {
  fail(`${label} did not include an issues array.`);
}

if (typeof parsed.count !== "number") {
  fail(`${label} did not include a numeric count.`);
}

if (parsed.count !== parsed.issues.length) {
  fail(
    `${label} count ${parsed.count} did not match issues length ${parsed.issues.length}.`,
  );
}

await writeStdout(`${JSON.stringify(parsed.issues, null, 2)}\n`);
