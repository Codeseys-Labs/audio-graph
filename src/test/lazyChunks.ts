/**
 * `SessionsBrowser` loads the Graph lens with `React.lazy`, so the first test
 * that opens it pays for transforming and evaluating the whole
 * `KnowledgeGraphViewer` module graph inside its own time budget. That cost is
 * what made the Graph lens wait in the resident and lens tests fail on a busy
 * machine (#548), and which test paid it depended on file and test order.
 * Awaiting this in `beforeAll` moves the cost outside every test; `lazy()` then
 * resolves from the module cache. Call it after the file's own
 * `vi.mock("react-force-graph-2d", ...)` so the stub is what gets cached.
 */
export function preloadGraphLens(): Promise<unknown> {
  return import("../components/KnowledgeGraphViewer");
}
