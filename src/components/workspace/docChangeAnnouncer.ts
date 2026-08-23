/**
 * `docChangeAnnouncer` — the living-document's batched sr-only announcement
 * (ticket W10, synthesis audio-graph-a6b5, design-a §1.6: "one debounced
 * `role="status" aria-live="polite"` per tile"). No debounce utility exists
 * anywhere else in this repo (verified — the W7 review's own finding, cited
 * in this ticket), so this is the minimal local implementation the ticket
 * calls for: pure, framework-free, and independently unit-testable with fake
 * timers before `LiveDocument.tsx` ever wires it to a real fold.
 *
 * Accumulates DISTINCT node ids (a `Set`, not a running sum of per-fold
 * counts) across the debounce window, so a node that pulses twice inside one
 * window is announced once, not twice — "N passages refined" must count
 * PASSAGES, not upsert events, the same distinction `liveDocumentModel.ts`'s
 * own `revisions` field draws for the gutter badge.
 *
 * Classic TRAILING-edge debounce: each `push()` call resets the timer, so a
 * burst of rapid folds collapses to exactly one `onFlush` call, fired
 * `waitMs` after the LAST push in the burst — never once per push. Once a
 * window flushes, the accumulator resets to empty, so the NEXT push starts a
 * fresh window and (after its own `waitMs`) produces its own, separate
 * announcement — "3 rapid folds -> 1 announcement; window passes -> next
 * announcement" (this ticket's own acceptance wording) is exactly this
 * shape.
 *
 * DISCLOSED GAP: a pure trailing-edge debounce has no MAX WAIT — a fold
 * arriving faster than every `waitMs` (this ticket: 2000ms) keeps resetting
 * the timer indefinitely, so a sustained sub-2s-cadence patch storm produces
 * ZERO announcements for as long as the storm continues (verified: folds
 * every 100ms for 5s flushed nothing until the storm stopped). This is still
 * within the ticket's own spec — "debounced" was the mandated shape, and at
 * most one announcement per window holds regardless — but it means
 * announcement LATENCY is unbounded under sustained activity. A
 * debounce/throttle hybrid (force a flush at least once every N windows
 * during continuous pushes) would bound that latency; left as a follow-up
 * rather than folded into this pass, since it changes this function's
 * contract (today: "flushes `waitMs` after the last push", full stop) and
 * deserves its own acceptance criteria rather than an incidental tweak here.
 */

export interface BatchedChangeAnnouncer {
  /** Merge `ids` into the pending window and (re)start the debounce timer.
   * A no-op call with an empty array never starts/extends a window — this is
   * what lets `LiveDocument.tsx` unconditionally call `push(vm.changedNodeIds)`
   * on every fold without a separate empty-array guard at the call site. */
  push: (ids: readonly string[]) => void;
  /** Cancel any pending timer and discard the accumulated ids without
   * flushing — called on unmount so a debounce started just before a
   * teardown never fires `onFlush` against an unmounted component. */
  cancel: () => void;
}

/**
 * @param onFlush Called with the number of DISTINCT ids accumulated since
 *   the last flush. Never called with `0` (a window that received `push()`
 *   calls but only ever with ids already inside it — impossible in practice
 *   since `Set.add` is idempotent and a window only starts on a non-empty
 *   push — cannot happen, but the guard costs nothing and keeps this
 *   function's contract "onFlush's count is always > 0" load-bearing).
 * @param waitMs The debounce window, in ms.
 */
export function createBatchedChangeAnnouncer(
  onFlush: (count: number) => void,
  waitMs: number,
): BatchedChangeAnnouncer {
  let pending = new Set<string>();
  let timer: ReturnType<typeof setTimeout> | undefined;

  const flush = () => {
    const count = pending.size;
    pending = new Set();
    timer = undefined;
    if (count > 0) onFlush(count);
  };

  return {
    push(ids) {
      if (ids.length === 0) return;
      for (const id of ids) pending.add(id);
      if (timer !== undefined) clearTimeout(timer);
      timer = setTimeout(flush, waitMs);
    },
    cancel() {
      if (timer !== undefined) clearTimeout(timer);
      timer = undefined;
      pending = new Set();
    },
  };
}
