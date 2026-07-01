/**
 * Centralized localStorage key constants.
 *
 * Single source of truth for browser-storage keys shared across components,
 * so the literal strings never drift out of sync (B34). Before this module,
 * `ag.onboardingHandoffSeen` was duplicated in `App.tsx`, `ShortcutsHelpModal.tsx`,
 * and their tests, kept aligned only by comment convention.
 */

/**
 * Set to `"1"` once the post-Express onboarding hand-off nudge has been seen.
 * App.tsx reads it to suppress the nudge; ShortcutsHelpModal clears it to
 * re-arm "show getting-started again". (B20 / B34)
 */
export const ONBOARDING_HANDOFF_SEEN_KEY = "ag.onboardingHandoffSeen";
