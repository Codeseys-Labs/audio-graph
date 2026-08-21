// Node types are ambient here only because the E2E devDependency chain
// (seed audio-graph-f9e0) transitively pulls in @types/node — the browser
// bundle itself still never imports "node:fs" outside test-only code (see
// src/styles.a11y.test.ts for the same convention).
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * Repo-wide gate generalizing the `audio-graph-d19f` SessionsBrowser
 * regression test (`src/components/SessionsBrowser.test.tsx` — "no ghost
 * var(--border,#333) fallbacks").
 *
 * A `var(--custom-property, fallback)` whose `--custom-property` is never
 * defined anywhere in `src/styles.css` (the single source of truth per its
 * own header comment) is a latent theme bug: the fallback silently masks the
 * missing token in EVERY theme, so the rule never actually resolves through
 * the design system and instead renders a hardcoded literal that ignores
 * dark/light theming. The `audio-graph-8d89` fix pointed the four known
 * offenders in `src/styles/settings.css` (five ghost var() call sites — one
 * line nests a second ghost fallback inside the first) at real tokens; this
 * test makes sure the class of bug can't come back anywhere under `src/`.
 *
 * Test files are excluded from the sweep: they legitimately reference the
 * OLD ghost-token syntax in prose (e.g. the d19f test's own title/comment
 * describe the `var(--border,#333)` bug it fixed) without ever emitting real
 * CSS, so scanning them would flag documentation, not defects.
 */

const SRC_DIR = "src";
const SCAN_EXTENSIONS = [".css", ".ts", ".tsx"];
const EXCLUDED_NAME_FRAGMENTS = [".test.", ".spec."];

function isScannableFile(name: string): boolean {
  if (!SCAN_EXTENSIONS.some((ext) => name.endsWith(ext))) return false;
  return !EXCLUDED_NAME_FRAGMENTS.some((fragment) => name.includes(fragment));
}

/** Recursively collects scannable source files under `dir`. */
function collectFiles(dir: string): string[] {
  const files: string[] = [];
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    const stats = statSync(full);
    if (stats.isDirectory()) {
      files.push(...collectFiles(full));
    } else if (stats.isFile() && isScannableFile(entry)) {
      files.push(full);
    }
  }
  return files;
}

/** Every custom property `styles.css` declares, across every theme block. */
function definedTokens(css: string): Set<string> {
  return new Set(
    Array.from(
      css.matchAll(/^\s*--([a-zA-Z0-9_-]+)\s*:/gm),
      (match) => match[1],
    ),
  );
}

interface GhostFallback {
  file: string;
  line: number;
  token: string;
}

/** Every `var(--token, fallback)` call site whose token isn't in `known`.
 *
 * Runs the regex over the WHOLE file content (not line-by-line): a
 * `var(--token,` split across a line break — the `--token,` on a different
 * line than the `var(` — never matches a per-line scan, since neither line
 * alone contains the full call-site pattern. Line numbers for reporting are
 * derived from `match.index` instead. */
function findGhostFallbacks(
  files: string[],
  known: Set<string>,
): GhostFallback[] {
  const ghosts: GhostFallback[] = [];
  const callSite = /var\(\s*--([a-zA-Z0-9_-]+)\s*,/g;
  for (const file of files) {
    const content = readFileSync(file, "utf8");
    for (const match of content.matchAll(callSite)) {
      const token = match[1];
      if (!known.has(token)) {
        const line = content.slice(0, match.index ?? 0).split("\n").length;
        ghosts.push({ file, line, token });
      }
    }
  }
  return ghosts;
}

describe("no undefined-token var() fallbacks anywhere in src/ (d19f generalization)", () => {
  const css = readFileSync("src/styles.css", "utf8");
  const known = definedTokens(css);

  it("parses a real, non-trivial token set from styles.css", () => {
    // Guards against the extraction regex silently matching nothing, which
    // would make the ghost-fallback assertion below vacuously pass.
    expect(known.size).toBeGreaterThan(50);
    expect(known).toContain("bg-primary");
    expect(known).toContain("focus-ring-color");
    expect(known).toContain("hover-overlay");
    expect(known).toContain("bg-elevated");
  });

  it("has no var(--x, fallback) anywhere in src/ where --x is undefined in styles.css", () => {
    const files = collectFiles(SRC_DIR);
    const ghosts = findGhostFallbacks(files, known);
    expect(
      ghosts,
      ghosts
        .map(
          (ghost) =>
            `${ghost.file}:${ghost.line} references undefined token --${ghost.token} with a silent fallback`,
        )
        .join("\n"),
    ).toEqual([]);
  });
});
