/**
 * Motion preferences helper.
 *
 * CSS `prefers-reduced-motion` is handled globally in `styles.css`, but it
 * does NOT affect JavaScript-driven scrolling such as
 * `element.scrollIntoView({ behavior: "smooth" })`. Use {@link scrollBehavior}
 * to pick the OS-appropriate behaviour for those calls.
 */

/** True when the OS requests reduced motion. */
export function prefersReducedMotion(): boolean {
    if (typeof window === "undefined" || !window.matchMedia) return false;
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * Resolve a `ScrollBehavior` honouring the OS reduced-motion setting:
 * `"auto"` (instant) when reduced motion is requested, otherwise `"smooth"`.
 */
export function scrollBehavior(): ScrollBehavior {
    return prefersReducedMotion() ? "auto" : "smooth";
}
