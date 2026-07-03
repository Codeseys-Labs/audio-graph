import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { visualizer } from "rollup-plugin-visualizer";
import { defineConfig } from "vite";

// https://v2.tauri.app/start/frontend/vite/
const host = process.env.TAURI_DEV_HOST;

// Opt-in bundle-size analyzer: set ANALYZE=1 to emit dist/stats.html.
// Has no effect on the default build when ANALYZE is unset.
const analyze = Boolean(process.env.ANALYZE);

export default defineConfig(async () => ({
  plugins: [
    react(),
    tailwindcss(),
    ...(analyze
      ? [
          visualizer({
            filename: "dist/stats.html",
            gzipSize: true,
            brotliSize: true,
            template: "treemap",
          }),
        ]
      : []),
  ],

  // Chunk strategy (audio-graph-932b). Heavy/conditional UI (graph viewer +
  // force-graph, settings/sessions/express-setup modals) is already code-split
  // via React.lazy in App.tsx. On top of that we carve the long-lived
  // node_modules dependencies out of the app-code entry chunk into a handful of
  // stable, separately-cacheable vendor chunks. This keeps the app-code entry
  // chunk under Rollup's 500 kB warning threshold and lets browsers keep vendor
  // code cached across app-only redeploys — none of these deps are on a
  // first-paint critical path that eager inlining would otherwise help.
  //
  // NOTE: this reduces the *entry* chunk size and improves cacheability; it
  // does NOT materially speed up the production build. The ~9m39s cold build is
  // dominated by WSL2/NTFS filesystem I/O on /mnt/e (processes sit in
  // uninterruptible disk-wait during emit), which is environmental and not
  // addressable from the bundler config. See the PR body for the analysis.
  build: {
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) return undefined;
          // React runtime + its scheduler: the largest, most stable vendor
          // group, shared by every route.
          if (
            /[\\/]node_modules[\\/](react|react-dom|scheduler)[\\/]/.test(id)
          ) {
            return "react-vendor";
          }
          // i18n stack (i18next + react-i18next + language detector). Needed at
          // startup but rarely changes, so it caches well on its own.
          if (/[\\/]node_modules[\\/](i18next|react-i18next)/.test(id)) {
            return "i18n-vendor";
          }
          // Everything else from node_modules (force-graph/d3 — reachable only
          // via the lazy KnowledgeGraphViewer — plus Radix, Tauri API, lucide
          // icons, zustand, …) shares one general vendor chunk rather than being
          // inlined into the app-code entry chunk. force-graph + d3 are kept in
          // this same group (not a separate graph-vendor chunk) because d3
          // sub-packages are also pulled by shared code, and splitting them out
          // creates a graph-vendor↔vendor circular chunk that Rollup warns on.
          return "vendor";
        },
      },
    },
  },

  // Vite options tailored for Tauri development
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // tell vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
