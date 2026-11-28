import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// GitHub Actions runners are not shared, so the default 5 s per-test budget
// stays there: a hang is caught fast. (Not `CI`: agent harnesses set that too,
// on the very machines this is about.) Dev machines (several agents each running a full
// suite) are routinely oversubscribed 10x or more: tests whose unloaded cost
// is 0.1-2 s (measured, #523/#548) were observed taking 5-15 s of wall clock,
// and no test change can fix that. The wait is only ever spent by a test that
// is failing or hung, so a passing test is no slower; a real hang is still
// caught, at 30 s. `src/test/setup.ts` scales RTL's `waitFor` budget the same way.
const loadedMachineBudgetMs = process.env.GITHUB_ACTIONS ? 5_000 : 30_000;

export default defineConfig({
  plugins: [react()],
  test: {
    testTimeout: loadedMachineBudgetMs,
    hookTimeout: loadedMachineBudgetMs * 2,
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    css: false,
    // Only run the canonical suite under src/. Worktree checkouts
    // (.claude/worktrees/**, .codex/**) contain stale duplicate test
    // files that otherwise get collected and run twice.
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    exclude: [
      "node_modules/**",
      "dist/**",
      ".claude/**",
      ".codex/**",
      ".agents/**",
    ],
    coverage: {
      provider: "v8",
      reporter: ["text", "json-summary", "html"],
      reportsDirectory: "./coverage",
      include: ["src/**/*.{ts,tsx}"],
      exclude: [
        "src/**/*.test.{ts,tsx}",
        "src/test/**",
        "src/vite-env.d.ts",
        "src/main.tsx",
      ],
      thresholds: {
        statements: 60,
        branches: 50,
        functions: 55,
        lines: 60,
      },
    },
  },
});
