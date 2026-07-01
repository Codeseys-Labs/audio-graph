#!/usr/bin/env node
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { basename, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
const args = process.argv.slice(2);
const fixtureSelfTest = args.includes("--fixture-self-test");
const help = args.includes("--help") || args.includes("-h");
const unknownArgs = args.filter((arg) => !["--fixture-self-test", "--help", "-h"].includes(arg));

if (help) {
  console.log(`Usage: bun scripts/check-docs-secret-hygiene.mjs [--fixture-self-test]

Scans docs and Seeds text for provider-shaped API keys and live credential
presence claims. Findings are redacted and cause a nonzero exit.

The fixture self-test runs against in-memory samples and exits 0 only if
detection and redaction behavior are working.`);
  process.exit(0);
}

if (unknownArgs.length > 0) {
  console.error(`Unknown argument(s): ${unknownArgs.join(", ")}`);
  process.exit(2);
}

const scanRoots = ["docs", ".seeds", "README.md", "AGENTS.md"];
const textExtensions = new Set([
  ".jsonl",
  ".md",
  ".mdx",
  ".txt",
  ".yaml",
  ".yml",
]);

const keyRules = [
  {
    id: "openai-key",
    label: "OpenAI/OpenRouter-style API key",
    pattern: /\bsk-(?:proj-|or-|ant-)?[A-Za-z0-9_-]{3,}\b/g,
  },
  {
    id: "google-ai-key",
    label: "Google AI Studio API key",
    pattern: /\bAIza[0-9A-Za-z_-]{8,}\b/g,
  },
  {
    id: "aws-access-key",
    label: "AWS access key id",
    pattern: /\b(?:AKIA|ASIA)[0-9A-Z]{12,}\b/g,
  },
  {
    id: "deepgram-key",
    label: "Deepgram-style API key",
    pattern: /\bdg[_-][A-Za-z0-9_-]{6,}\b/g,
  },
  {
    id: "anthropic-key",
    label: "Anthropic API key",
    pattern: /\bsk-ant-[A-Za-z0-9_-]{6,}\b/g,
  },
  {
    id: "tavily-key",
    label: "Tavily API key",
    pattern: /\btvly-[A-Za-z0-9_-]{6,}\b/g,
  },
  {
    id: "slack-token",
    label: "Slack token",
    pattern: /\bxox[baprs]-[A-Za-z0-9-]{8,}\b/g,
  },
  {
    id: "provider-key-example-shape",
    label: "provider-shaped example key",
    pattern:
      /\b(?:sk-(?:proj-|or-|ant-)?\.\.\.|dg[_-]\.\.\.|AIza\.\.\.|(?:AKIA|ASIA)\.\.\.|tvly-\.\.\.|xox[baprs]-\.\.\.)\b/g,
  },
];

const credentialClaimRules = [
  {
    id: "live-credential-presence",
    label: "live credential presence claim",
    pattern:
      /\b(?:live|real|actual|temporary)\s+(?:[A-Za-z0-9_-]+\s+){0,3}(?:api[_ -]?key|credential|openai_api_key|openrouter_api_key|deepgram_api_key|gemini_api_key|assemblyai_api_key|aws_access_key_id)\b.{0,80}\b(?:(?:is|are|was|were)\s+)?(?:present|provided|available|configured|loaded|used|valid)\b/i,
  },
  {
    id: "present-credential-claim",
    label: "present credential claim",
    pattern:
      /\b(?:api[_ -]?key|openai_api_key|openrouter_api_key|deepgram_api_key|gemini_api_key|assemblyai_api_key|aws_access_key_id)\b.{0,80}\b(?:(?:is|are|was|were|shows?|shown|marked)\s+(?:present|provided|available|configured|filled(?: in)?)|(?:present|provided|available|configured)\s+in|empty slot|fill(?:ed)?(?: in)?)\b/i,
  },
  {
    id: "credential-file-presence",
    label: "credential file presence claim",
    pattern:
      /\bcredentials\.yaml\b.{0,120}\b(?:already exists|contains|with live|full schema|empty slot|exists with|configured in|present in|dev box already has|fill(?:ed)?(?: in)?)\b/i,
  },
];

const secretRedactions = [
  ...keyRules.map((rule) => rule.pattern),
  /\b(?:openai|openrouter|deepgram|gemini|assemblyai|anthropic|soniox|revai|gladia|speechmatics|aws|google)[_-]api[_-]?key\s*[:=]\s*["']?[^"',\s)]+/gi,
  /\baws_secret_access_key\s*[:=]\s*["']?[^"',\s)]+/gi,
];

// Allowlist policy:
// - Keep the scanner self-contained so future CI use does not rely on hidden state.
// - Allow only explicit redaction/test fixtures, not prose docs that claim a real
//   local credentials file exists or show provider-shaped example tokens.
// - Path allowlists should stay narrow and named; broad docs/ or .seeds/
//   exemptions would defeat audio-graph-de28.
const allowlist = [
  {
    reason: "scanner's own in-memory fixture proves redaction and detection",
    path: "scripts/check-docs-secret-hygiene.mjs",
  },
];

function isAllowlisted(filePath) {
  const normalized = normalizePath(filePath);
  return allowlist.some((entry) => normalized === entry.path);
}

function normalizePath(filePath) {
  return filePath.split(sep).join("/");
}

function extensionOf(filePath) {
  const name = basename(filePath);
  const dot = name.lastIndexOf(".");
  return dot >= 0 ? name.slice(dot) : "";
}

function collectFiles(target) {
  const absolute = resolve(repoRoot, target);
  if (!existsSync(absolute)) {
    return [];
  }

  const stats = statSync(absolute);
  if (stats.isFile()) {
    return shouldScanFile(absolute) ? [absolute] : [];
  }

  const files = [];
  const entries = readdirSync(absolute, { withFileTypes: true });
  for (const entry of entries) {
    const child = join(absolute, entry.name);
    if (entry.isDirectory()) {
      if ([".git", "node_modules", "target", "dist", "build", "coverage"].includes(entry.name)) {
        continue;
      }
      files.push(...collectFiles(relative(repoRoot, child)));
    } else if (entry.isFile() && shouldScanFile(child)) {
      files.push(child);
    }
  }

  return files;
}

function shouldScanFile(filePath) {
  return textExtensions.has(extensionOf(filePath));
}

function maskSecretLikeValues(value) {
  let redacted = value;
  for (const pattern of secretRedactions) {
    pattern.lastIndex = 0;
    redacted = redacted.replace(pattern, (match) => {
      const prefix = match.includes("=")
        ? match.slice(0, match.indexOf("=") + 1)
        : match.includes(":")
          ? match.slice(0, match.indexOf(":") + 1)
          : "";
      return `${prefix}[REDACTED:${match.length}]`;
    });
  }
  return redacted;
}

function compactSnippet(line) {
  const trimmed = line.trim().replace(/\s+/g, " ");
  return maskSecretLikeValues(trimmed.length > 180 ? `${trimmed.slice(0, 177)}...` : trimmed);
}

function isApprovedTestOrRedactionValue(match) {
  return /^sk-test$/i.test(match) || match.includes("[REDACTED");
}

function isNegatedCredentialClaim(line) {
  const credentialName =
    "(?:api[_ -]?key|credential|openai_api_key|openrouter_api_key|deepgram_api_key|gemini_api_key|assemblyai_api_key|aws_access_key_id)";
  return (
    new RegExp(
      `\\bno\\s+(?:[A-Za-z0-9_-]+\\s+){0,4}${credentialName}\\s+(?:is|are|was|were)?\\s*provided\\b`,
      "i",
    ).test(line) ||
    new RegExp(`\\b${credentialName}\\s+(?:is|are|was|were)?\\s*(?:not\\s+available|unavailable|missing)\\b`, "i").test(
      line,
    )
  );
}

function scanText(filePath, text) {
  if (isAllowlisted(filePath)) {
    return [];
  }

  const findings = [];
  const lines = text.split(/\r?\n/);
  for (const [index, line] of lines.entries()) {
    for (const rule of keyRules) {
      rule.pattern.lastIndex = 0;
      const matches = [...line.matchAll(rule.pattern)].map((match) => match[0]);
      if (matches.some((match) => !isApprovedTestOrRedactionValue(match))) {
        findings.push({
          file: filePath,
          line: index + 1,
          rule: rule.id,
          label: rule.label,
          snippet: compactSnippet(line),
        });
      }
    }

    for (const rule of credentialClaimRules) {
      if (rule.pattern.test(line) && !isNegatedCredentialClaim(line)) {
        findings.push({
          file: filePath,
          line: index + 1,
          rule: rule.id,
          label: rule.label,
          snippet: compactSnippet(line),
        });
      }
    }
  }

  return findings;
}

function scanRepo() {
  const files = scanRoots.flatMap(collectFiles);
  const findings = [];
  for (const absolute of files) {
    const relativePath = normalizePath(relative(repoRoot, absolute));
    const text = readFileSync(absolute, "utf8");
    findings.push(...scanText(relativePath, text));
  }
  return findings.sort((left, right) => {
    if (left.file !== right.file) {
      return left.file.localeCompare(right.file);
    }
    return left.line - right.line || left.rule.localeCompare(right.rule);
  });
}

function printFindings(findings) {
  if (findings.length === 0) {
    console.log("docs/Seeds secret hygiene scan passed: 0 findings");
    return;
  }

  console.error(`docs/Seeds secret hygiene scan failed: ${findings.length} finding(s)`);
  for (const finding of findings) {
    console.error(
      `${finding.file}:${finding.line} ${finding.rule} (${finding.label}) :: ${finding.snippet}`,
    );
  }
}

function runFixtureSelfTest() {
  const fixturePath = "docs/fixture.md";
  const liveKey = "sk-or-liveSecretValueThatMustNotPrint";
  const findings = scanText(
    fixturePath,
    [
      `Do not leak ${liveKey} in docs.`,
      "credentials.yaml already exists with live openrouter_api_key is present.",
    ].join("\n"),
  );

  const output = findings.map((finding) => finding.snippet).join("\n");
  if (findings.length !== 4) {
    console.error(`fixture self-test failed: expected 4 findings, got ${findings.length}`);
    process.exit(1);
  }

  if (output.includes(liveKey) || !output.includes("[REDACTED:")) {
    console.error("fixture self-test failed: secret-shaped value was not redacted");
    process.exit(1);
  }

  console.log("fixture self-test passed: detected key and credential presence claims with redaction");
}

if (fixtureSelfTest) {
  runFixtureSelfTest();
} else {
  const findings = scanRepo();
  printFindings(findings);
  process.exit(findings.length === 0 ? 0 : 1);
}
