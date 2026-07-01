/**
 * Ambient type for Vite `?raw` imports of Rust sources.
 *
 * Vite's `vite/client` types only declare `?raw` for known asset extensions;
 * `.rs?raw` is not one of them. Contract tests import Rust source-of-truth
 * files as strings (e.g. `credentialSourceContract.test.ts` reads
 * `src-tauri/src/credentials/mod.rs`) to assert backend⇄frontend invariants,
 * so this declares the specifier as a string default export.
 */
declare module "*.rs?raw" {
  const content: string;
  export default content;
}
