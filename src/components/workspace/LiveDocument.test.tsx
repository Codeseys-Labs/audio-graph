import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../../store";
import type {
  AsrSpanRevisionEvent,
  MaterializedNote,
  MaterializedNotes,
  ProjectionPatch,
} from "../../types";
import {
  DocRecencyChip,
  LiveDocument,
  LiveDocumentHeaderActions,
  useLiveDocumentModel,
} from "./LiveDocument";
import { WorkspaceTile } from "./WorkspaceTile";

function note(
  overrides: Partial<MaterializedNote> & { id: string },
): MaterializedNote {
  return {
    id: overrides.id,
    title: overrides.title ?? "Untitled",
    body: overrides.body ?? "",
    tags: overrides.tags ?? [],
    heading_level: overrides.heading_level ?? null,
    updated_by_sequence: overrides.updated_by_sequence ?? 1,
    updated_at_ms: overrides.updated_at_ms ?? 0,
    basis: null,
    provenance: null,
  };
}

function materializedNotes(
  notes: MaterializedNote[],
  lastSequence: number,
): MaterializedNotes {
  return {
    schema_version: 1,
    session_id: "live",
    last_sequence: lastSequence,
    notes,
  };
}

function notesPatch(overrides: Partial<ProjectionPatch>): ProjectionPatch {
  return {
    sequence: 1,
    kind: "notes",
    llm_request_id: "r1",
    basis: null,
    operations: [],
    confidence: 1,
    provenance: null,
    created_at_ms: 0,
    ...overrides,
  };
}

/** A finalized-turn ASR revision fixture (ticket W6) — `is_final: true` by
 * default so callers only need `received_at_ms`/`turn_id` to build a
 * `turnsBehind`-driving revision. Mirrors the real wire shape
 * `useTauriEvents.test.ts` builds for `asr-span-revision`. */
function finalizedRevision(
  overrides: Partial<AsrSpanRevisionEvent> & { received_at_ms: number },
): AsrSpanRevisionEvent {
  return {
    span_id: `span-${overrides.received_at_ms}`,
    provider: "deepgram",
    source_id: "system-default",
    text: "",
    start_time: 0,
    end_time: 0,
    confidence: 1,
    is_final: true,
    stability: "final",
    revision_number: 1,
    end_of_turn: true,
    ...overrides,
  };
}

// The composed shape App.tsx actually renders: one `useLiveDocumentModel()`
// call feeding BOTH `WorkspaceTile`'s `headerSlot` and `LiveDocument`'s
// body, exactly as ticket W5's point 8 requires (no second header inside
// the tile body).
function DocumentTileHarness() {
  const vm = useLiveDocumentModel();
  return (
    <WorkspaceTile
      id="document"
      title="Notes"
      headerSlot={<LiveDocumentHeaderActions vm={vm} />}
    >
      <LiveDocument vm={vm} />
    </WorkspaceTile>
  );
}

// Real store, unwrapped by `SessionViewProvider` — same fallback pattern
// `useActiveGraphSnapshot.test.tsx` uses: `useSessionView()` reads the live
// store directly with no provider present.
describe("LiveDocument (ticket W5, synthesis audio-graph-a6b5)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      materializedNotes: null,
      isCapturing: false,
    });
  });

  it("renders an empty state with no fabricated structure when there are no notes yet", () => {
    render(<DocumentTileHarness />);
    expect(screen.getByTestId("document-empty")).toBeInTheDocument();
    expect(screen.getByText("Your notes will appear here")).toBeInTheDocument();
    // The sample-session preview affordance (design-a §1.7 row 1: reuse
    // today's hero verbatim) survives the NotesPanel -> LiveDocument swap.
    expect(
      screen.getByRole("button", { name: "Preview sample session" }),
    ).toBeInTheDocument();
  });

  it("shows an aria-hidden skeleton (not the empty hero) while capturing with nothing landed yet", () => {
    useAudioGraphStore.setState({ isCapturing: true });
    render(<DocumentTileHarness />);
    expect(screen.getByTestId("document-skeleton")).toBeInTheDocument();
    expect(screen.queryByTestId("document-empty")).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Building notes");
  });

  it("renders legacy-mode notes as flat bullets, in order, with the title as a bold lead", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [
          note({
            id: "n1",
            title: "Quote 1",
            body: "We ship Friday.",
            updated_by_sequence: 1,
          }),
          note({
            id: "n2",
            title: "Quote 2",
            body: "Per-seat rejected.",
            updated_by_sequence: 2,
          }),
        ],
        2,
      ),
    });
    render(<DocumentTileHarness />);
    expect(screen.getByText("We ship Friday.")).toBeInTheDocument();
    expect(screen.getByText("Per-seat rejected.")).toBeInTheDocument();
    expect(screen.getByText("Quote 1")).toBeInTheDocument();
    const n1 = document.querySelector('[data-note-id="n1"]');
    const n2 = document.querySelector('[data-note-id="n2"]');
    expect(n1).toBeInTheDocument();
    expect(n2).toBeInTheDocument();
  });

  it("renders a heading_level note as a real heading tag, with following notes nested under it", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [
          note({
            id: "pricing",
            title: "Pricing model",
            heading_level: 2,
            body: "Tiered pricing agreed.",
            updated_by_sequence: 1,
          }),
          note({
            id: "followup",
            title: "Seat floor",
            body: "Finance to confirm.",
            updated_by_sequence: 2,
          }),
        ],
        2,
      ),
    });
    render(<DocumentTileHarness />);
    expect(
      screen.getByRole("heading", { level: 2, name: "Pricing model" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Tiered pricing agreed.")).toBeInTheDocument();
    expect(screen.getByText("Finance to confirm.")).toBeInTheDocument();
  });

  it("sticky-follows a tail append on a FRESH MOUNT, before the reader ever touches the scrollbar", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 1 })],
        1,
      ),
    });
    const { container } = render(<DocumentTileHarness />);
    const scrollEl = container.querySelector(
      ".overflow-y-auto",
    ) as HTMLDivElement;
    expect(scrollEl).toBeInTheDocument();
    // jsdom never computes real layout, so `scrollHeight`/`clientHeight`
    // default to 0 — stand in a real-looking geometry so a follow (or its
    // absence) is observable via `scrollTop`.
    Object.defineProperty(scrollEl, "scrollHeight", {
      value: 2000,
      configurable: true,
    });
    Object.defineProperty(scrollEl, "clientHeight", {
      value: 400,
      configurable: true,
    });

    // A pure tail append, with the reader having NEVER scrolled (no
    // `onScroll` was ever dispatched on `scrollEl`) — this is the default
    // case for every live session: nobody touches the transcript/document
    // scrollbar while just watching it grow.
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha",
              updated_by_sequence: 1,
            }),
            note({
              id: "n2",
              title: "B",
              body: "bravo",
              updated_by_sequence: 2,
            }),
          ],
          2,
        ),
      });
    });

    expect(scrollEl.scrollTop).toBe(2000);
  });

  it("does NOT chase the tail for a pure tail append once the reader has scrolled away — L1's negative half (mutation-provable: `if (vm.appendedAtTail)` alone, dropping the `wasNearBottomRef.current` guard, passes every OTHER test in this suite)", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 1 })],
        1,
      ),
    });
    const { container } = render(<DocumentTileHarness />);
    const scrollEl = container.querySelector(
      ".overflow-y-auto",
    ) as HTMLDivElement;
    Object.defineProperty(scrollEl, "scrollHeight", {
      value: 2000,
      configurable: true,
    });
    Object.defineProperty(scrollEl, "clientHeight", {
      value: 400,
      configurable: true,
    });
    Object.defineProperty(scrollEl, "scrollTop", {
      value: 1000,
      configurable: true,
      writable: true,
    });

    // The reader scrolls away from the bottom (2000 - 1000 - 400 = 600,
    // well past the 100px "near bottom" threshold) — flips
    // `wasNearBottomRef.current` to `false`.
    act(() => {
      fireEvent.scroll(scrollEl);
    });

    // A pure tail append — the exact case that DOES auto-follow when the
    // reader has never scrolled (the sibling test above).
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha",
              updated_by_sequence: 1,
            }),
            note({
              id: "n2",
              title: "B",
              body: "bravo",
              updated_by_sequence: 2,
            }),
          ],
          2,
        ),
      });
    });

    // Must NOT chase the tail — the reader is still exactly where they
    // scrolled to.
    expect(scrollEl.scrollTop).toBe(1000);
  });

  it("does not remount an unchanged node's DOM element when a SIBLING note changes (stable key contract)", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [
          note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 1 }),
          note({ id: "n2", title: "B", body: "bravo", updated_by_sequence: 2 }),
        ],
        2,
      ),
    });
    render(<DocumentTileHarness />);
    const n1Before = document.querySelector('[data-note-id="n1"]');
    expect(n1Before).toBeInTheDocument();

    // A NEW MaterializedNotes object (as every real accepted patch produces
    // — `applyProjectionNotesPatch` never mutates in place) where n1 is
    // byte-identical and only n2 changed. Wrapped in `act()` so the update
    // is flushed synchronously before the next assertion — without it,
    // this test would pass vacuously (nothing re-rendered yet).
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha",
              updated_by_sequence: 1,
            }),
            note({
              id: "n2",
              title: "B",
              body: "bravo-charlie",
              updated_by_sequence: 3,
            }),
          ],
          3,
        ),
      });
    });

    // Proves the update actually flushed (otherwise the identity check
    // below would be vacuously true).
    expect(screen.getByText("bravo-charlie")).toBeInTheDocument();
    expect(screen.queryByText("bravo")).not.toBeInTheDocument();

    const n1After = document.querySelector('[data-note-id="n1"]');
    expect(n1After).toBeInTheDocument();
    // Same DOM element instance — React patched around it rather than
    // tearing it down and recreating it.
    expect(n1After).toBe(n1Before);
  });

  it("a tags-only re-upsert produces no visible change in the rendered document", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [
          note({
            id: "n1",
            title: "A",
            body: "alpha",
            tags: ["x"],
            updated_by_sequence: 1,
          }),
        ],
        1,
      ),
    });
    const { container } = render(<DocumentTileHarness />);
    const before = container.innerHTML;

    // TWO successive tags-only churns, wrapped in `act()` so each update
    // actually flushes (see the sibling "does not remount" test's comment
    // for why this matters). Two, not one: the gutter glyph only becomes
    // visible once `revisions > 1` (design-a §1.5), so a single incorrectly
    // counted "revision" would still render invisibly — this makes the
    // assertion below sensitive to a contentHash-includes-tags regression
    // at the DOM level too, not just at the model level.
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha",
              tags: ["x", "y", "z"],
              updated_by_sequence: 2,
            }),
          ],
          2,
        ),
      });
    });
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha",
              tags: ["x"],
              updated_by_sequence: 3,
            }),
          ],
          3,
        ),
      });
    });

    expect(container.innerHTML).toBe(before);
  });

  it("only shows the gutter revision glyph once a node has been revised more than once", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [note({ id: "n1", title: "A", body: "v1", updated_by_sequence: 1 })],
        1,
      ),
    });
    render(<DocumentTileHarness />);

    // One real edit -> revisions === 1 -> the glyph must stay hidden (design-a
    // §1.5: the gutter shows a revision count only "when > 1").
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [note({ id: "n1", title: "A", body: "v2", updated_by_sequence: 2 })],
          2,
        ),
      });
    });
    const buttonAfterOneEdit = document.querySelector(
      '[data-note-id="n1"] button',
    );
    expect(buttonAfterOneEdit?.textContent).toBe("");

    // A second real edit -> revisions === 2 -> the glyph becomes visible.
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [note({ id: "n1", title: "A", body: "v3", updated_by_sequence: 3 })],
          3,
        ),
      });
    });
    const buttonAfterTwoEdits = document.querySelector(
      '[data-note-id="n1"] button',
    );
    expect(buttonAfterTwoEdits?.textContent).toBe("·2");
  });

  it("(control) a real body edit DOES change the rendered document — proves the tags-only test above isn't vacuous", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 1 })],
        1,
      ),
    });
    render(<DocumentTileHarness />);
    expect(screen.getByText("alpha")).toBeInTheDocument();

    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha-prime",
              updated_by_sequence: 2,
            }),
          ],
          2,
        ),
      });
    });

    expect(screen.queryByText("alpha")).not.toBeInTheDocument();
    expect(screen.getByText("alpha-prime")).toBeInTheDocument();
  });

  it("the document tile has exactly one accessibly-named region — LiveDocument renders no second header", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [
          note({
            id: "n1",
            title: "Quote",
            body: "Something was said.",
            updated_by_sequence: 1,
          }),
        ],
        1,
      ),
    });
    render(<DocumentTileHarness />);

    const regions = screen.getAllByRole("region", { name: "Notes" });
    expect(regions).toHaveLength(1);
    // The tile's own title text appears exactly once — a second internal
    // header repeating "Notes" would fail this.
    expect(screen.getAllByText("Notes")).toHaveLength(1);

    const region = regions[0];
    const body = within(region)
      .getByText("Something was said.")
      .closest(".workspace-tile__body");
    expect(body).not.toBeNull();
  });

  it("the header slot shows a note count and a working copy action", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });

    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [
          note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 1 }),
          note({ id: "n2", title: "B", body: "bravo", updated_by_sequence: 2 }),
        ],
        2,
      ),
    });
    render(<DocumentTileHarness />);

    expect(screen.getByText("Notes: 2")).toBeInTheDocument();

    const copyButton = screen.getByRole("button", {
      name: "Copy notes as text",
    });
    copyButton.click();

    expect(writeText).toHaveBeenCalledTimes(1);
    const [markdown] = writeText.mock.calls[0] as [string];
    expect(markdown).toContain("**A** alpha");
    expect(markdown).toContain("**B** bravo");
  });
});

describe("DocRecencyChip — the tone-routed freshness chip (ticket W6, synthesis audio-graph-a6b5 §2)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [],
      asrSpanRevisions: [],
      loadedSessionId: null,
    });
  });

  it("renders data-tone=neutral with the 'as of' text when fewer than 3 finalized turns have passed since the last notes patch", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        notesPatch({ sequence: 1, created_at_ms: 1_700_000_000_000 }),
      ],
      asrSpanRevisions: [
        finalizedRevision({ received_at_ms: 1_700_000_001_000, turn_id: "t1" }),
      ],
      loadedSessionId: null,
    });
    render(<DocRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toBeInTheDocument();
    expect(chip).toHaveAttribute("data-tone", "neutral");
    expect(chip?.textContent).toMatch(/as of/i);
    // Regression guard for the "toLocaleTimeString() with no args uses the
    // OS/runtime locale, not the app language" bug: this test's own runtime
    // (Node/vitest) resolves to en-US, which renders AM/PM unless the call
    // site pins an explicit locale + `hour12: false`. Asserting "no AM/PM"
    // here fails if that pin regresses, closing a gap the interpolating
    // i18n budget test (which never calls the real `Date` API) cannot.
    expect(chip?.textContent).not.toMatch(/AM|PM/i);
  });

  it("renders data-tone=warning with '-N turns' at the ratified >=3 threshold", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        notesPatch({ sequence: 1, created_at_ms: 1_700_000_000_000 }),
      ],
      asrSpanRevisions: [
        finalizedRevision({ received_at_ms: 1_700_000_001_000, turn_id: "t1" }),
        finalizedRevision({ received_at_ms: 1_700_000_002_000, turn_id: "t2" }),
        finalizedRevision({ received_at_ms: 1_700_000_003_000, turn_id: "t3" }),
      ],
      loadedSessionId: null,
    });
    render(<DocRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toHaveAttribute("data-tone", "warning");
    expect(chip?.textContent).toMatch(/3/);
    expect(chip?.textContent).not.toMatch(/as of/i);
  });

  it("2 finalized turns behind stays neutral (the warning threshold is exclusive below 3)", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        notesPatch({ sequence: 1, created_at_ms: 1_700_000_000_000 }),
      ],
      asrSpanRevisions: [
        finalizedRevision({ received_at_ms: 1_700_000_001_000, turn_id: "t1" }),
        finalizedRevision({ received_at_ms: 1_700_000_002_000, turn_id: "t2" }),
      ],
      loadedSessionId: null,
    });
    render(<DocRecencyChip />);
    expect(document.querySelector("[data-tone]")).toHaveAttribute(
      "data-tone",
      "neutral",
    );
  });

  it("renders nothing when the notes lane has never produced an accepted patch this session", () => {
    render(<DocRecencyChip />);
    expect(document.querySelector("[data-tone]")).not.toBeInTheDocument();
  });

  it("renders nothing for a loaded/reviewed session — no freshness claim about a finished session's own history", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        notesPatch({ sequence: 1, created_at_ms: 1_700_000_000_000 }),
      ],
      asrSpanRevisions: [
        finalizedRevision({ received_at_ms: 1_700_000_001_000, turn_id: "t1" }),
      ],
      loadedSessionId: "recorded-session-1",
    });
    render(<DocRecencyChip />);
    expect(document.querySelector("[data-tone]")).not.toBeInTheDocument();
  });

  it("without W3 evidence, never renders data-tone=success at this call site, even given a maximally-healthy-looking store state — absent-evidence pin held at the CALL SITE, not just liveWorkspaceTone.ts's own unit tests", () => {
    // `laneRecencyChipTone.test.ts` pins that `evidence: null` structurally
    // blocks success. That pin says nothing about whether THIS call site
    // maps a patch with no `basis_currency_at_apply` to `evidence: null` —
    // a future edit to the mapping step could regress that, and nothing
    // here would fail. Assert on the rendered output instead: 0 turns
    // behind, a just-now patch, no `basis_currency_at_apply` on it — the
    // most success-looking store state that still carries no W3 evidence.
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        notesPatch({ sequence: 1, created_at_ms: Date.now() }),
      ],
      asrSpanRevisions: [],
      loadedSessionId: null,
    });
    render(<DocRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toBeInTheDocument();
    expect(chip).not.toHaveAttribute("data-tone", "success");
    expect(chip).toHaveAttribute("data-tone", "neutral");
  });

  it("ticket W3 fix round: renders data-tone=success AND the 'Up to date' copy (not the color-only regression the fix-round review caught) when the latest notes patch carried {type: 'current'} — the ONLY input that can ever earn it", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        notesPatch({
          sequence: 1,
          created_at_ms: Date.now(),
          basis_currency_at_apply: { type: "current" },
        }),
      ],
      asrSpanRevisions: [],
      loadedSessionId: null,
    });
    render(<DocRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toBeInTheDocument();
    expect(chip).toHaveAttribute("data-tone", "success");
    // The earned tier must be legible without color: a distinct visible
    // string, never the byte-identical "as of HH:MM:SS" the neutral tier
    // renders — plus its OWN sr-only text (not shared with the neutral
    // tier's `asOfAria`).
    const visible = chip?.querySelector('[aria-hidden="true"]');
    expect(visible?.textContent).toBe("Up to date");
    const srOnly = chip?.querySelector(".sr-only");
    expect(srOnly?.textContent).toMatch(/up to date/i);
  });

  it("ticket W3: {type: 'appended_tail'} is present evidence of lag, not current-ness — stays neutral, never upgraded to success", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        notesPatch({
          sequence: 1,
          created_at_ms: Date.now(),
          basis_currency_at_apply: {
            type: "appended_tail",
            staleness: { type: "missing_current_span" },
          },
        }),
      ],
      asrSpanRevisions: [],
      loadedSessionId: null,
    });
    render(<DocRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toBeInTheDocument();
    expect(chip).not.toHaveAttribute("data-tone", "success");
    expect(chip).toHaveAttribute("data-tone", "neutral");
    // Copy, not just tone: a known-lagging apply must render the SAME
    // observed-fact "as of HH:MM:SS" text as "no evidence yet" — never the
    // "Up to date" claim the success tier alone may make.
    expect(chip?.textContent).toMatch(/as of/i);
    expect(chip?.textContent).not.toMatch(/up to date/i);
  });

  it("ticket W3: a malformed/unrecognized basis_currency_at_apply.type maps to no evidence, not a crash or a success", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        notesPatch({
          sequence: 1,
          created_at_ms: Date.now(),
          // @ts-expect-error — deliberately malformed wire value.
          basis_currency_at_apply: { type: "not_a_real_tag" },
        }),
      ],
      asrSpanRevisions: [],
      loadedSessionId: null,
    });
    render(<DocRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toBeInTheDocument();
    expect(chip).not.toHaveAttribute("data-tone", "success");
    expect(chip).toHaveAttribute("data-tone", "neutral");
  });

  it("only counts the NOTES lane's patches, not the graph lane's, when deriving lastAppliedAtMs", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        // A graph patch, much more recent than any notes patch — must be
        // ignored entirely by the notes chip (kind-scoped derivation, W6's
        // "one function, two call sites" contract).
        {
          sequence: 1,
          kind: "graph",
          llm_request_id: "r1",
          basis: null,
          operations: [],
          confidence: 1,
          provenance: null,
          created_at_ms: 9_000_000_000_000,
        },
        notesPatch({ sequence: 1, created_at_ms: 1_700_000_000_000 }),
      ],
      asrSpanRevisions: [
        finalizedRevision({ received_at_ms: 1_700_000_001_000, turn_id: "t1" }),
      ],
      loadedSessionId: null,
    });
    render(<DocRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toHaveAttribute("data-tone", "neutral");
  });
});

/** Real `getBoundingClientRect()` returns VIEWPORT-relative coordinates that
 * shift as a scroll container's `scrollTop` changes (content moves up
 * relative to the fixed viewport window). jsdom computes no real layout, so
 * this mock reproduces that relationship explicitly: `contentTop` is the
 * element's position in CONTENT space (scroll-invariant), and the returned
 * `top` is recomputed from the CURRENT `scrollEl.scrollTop` every call —
 * exactly what `measureNodeGeometry`'s `elRect.top - containerRect.top +
 * container.scrollTop` formula expects to cancel back out to `contentTop`
 * regardless of scroll position. */
function mockNodeRect(
  el: HTMLElement,
  contentTop: number,
  height: number,
  scrollEl: HTMLElement,
): void {
  el.getBoundingClientRect = () =>
    ({
      top: contentTop - scrollEl.scrollTop,
      bottom: contentTop - scrollEl.scrollTop + height,
      height,
      left: 0,
      right: 0,
      width: 0,
      x: 0,
      y: contentTop - scrollEl.scrollTop,
      toJSON() {},
    }) as DOMRect;
}

/** The scroll container's OWN rect never moves relative to the page when
 * only its internal content scrolls — a constant mock is correct here
 * (unlike `mockNodeRect` above, which must track `scrollTop`). */
function mockContainerRect(el: HTMLElement, height: number): void {
  el.getBoundingClientRect = () =>
    ({
      top: 0,
      bottom: height,
      height,
      left: 0,
      right: 0,
      width: 0,
      x: 0,
      y: 0,
      toJSON() {},
    }) as DOMRect;
}

function mockScrollGeometry(
  el: HTMLElement,
  {
    scrollHeight,
    clientHeight,
  }: { scrollHeight: number; clientHeight: number },
): void {
  Object.defineProperty(el, "scrollHeight", {
    value: scrollHeight,
    configurable: true,
  });
  Object.defineProperty(el, "clientHeight", {
    value: clientHeight,
    configurable: true,
  });
  Object.defineProperty(el, "scrollTop", {
    value: 0,
    configurable: true,
    writable: true,
  });
  mockContainerRect(el, clientHeight);
}

describe("refinement pulse — .ag-doc-refined (ticket W10, synthesis audio-graph-a6b5)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      materializedNotes: null,
      isCapturing: false,
    });
  });

  function pulseWrapper(id: string): Element | null {
    return document.querySelector(`[data-note-id="${id}"] > div`);
  }

  it("pulses ONLY the node whose contentHash changed this fold — a sibling's real edit never pulses this node (reuses the DOM-identity test pattern)", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [
          note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 1 }),
          note({ id: "n2", title: "B", body: "bravo", updated_by_sequence: 2 }),
        ],
        2,
      ),
    });
    render(<DocumentTileHarness />);
    expect(pulseWrapper("n1")?.className).not.toContain("ag-doc-refined");
    expect(pulseWrapper("n2")?.className).not.toContain("ag-doc-refined");

    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha",
              updated_by_sequence: 1,
            }),
            note({
              id: "n2",
              title: "B",
              body: "bravo-charlie",
              updated_by_sequence: 3,
            }),
          ],
          3,
        ),
      });
    });

    expect(pulseWrapper("n2")?.className).toContain("ag-doc-refined");
    // The unrelated sibling never pulses just because SOMETHING in the
    // document changed this fold.
    expect(pulseWrapper("n1")?.className).not.toContain("ag-doc-refined");
  });

  it("a tags-only re-upsert fold pulses nothing anywhere in the document", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [
          note({
            id: "n1",
            title: "A",
            body: "alpha",
            tags: ["x"],
            updated_by_sequence: 1,
          }),
        ],
        1,
      ),
    });
    render(<DocumentTileHarness />);

    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha",
              tags: ["x", "y"],
              updated_by_sequence: 2,
            }),
          ],
          2,
        ),
      });
    });

    expect(pulseWrapper("n1")?.className).not.toContain("ag-doc-refined");
  });

  it("the pulse is FOLD-SCOPED, not sticky: it clears once a LATER fold changes something else, rather than staying attached to the node forever", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [
          note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 1 }),
          note({ id: "n2", title: "B", body: "bravo", updated_by_sequence: 2 }),
        ],
        2,
      ),
    });
    render(<DocumentTileHarness />);

    // Fold 2: n1 changes -> n1 pulses.
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha-prime",
              updated_by_sequence: 3,
            }),
            note({
              id: "n2",
              title: "B",
              body: "bravo",
              updated_by_sequence: 2,
            }),
          ],
          3,
        ),
      });
    });
    expect(pulseWrapper("n1")?.className).toContain("ag-doc-refined");

    // Fold 3: n2 changes, n1 does NOT — this fold's own changedNodeIds no
    // longer includes n1, so its pulse class must be gone, not merely
    // "still animating in the background" (a wrongly-derived
    // `revisions > 0` gate would keep this class on forever).
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha-prime",
              updated_by_sequence: 3,
            }),
            note({
              id: "n2",
              title: "B",
              body: "bravo-charlie",
              updated_by_sequence: 4,
            }),
          ],
          4,
        ),
      });
    });
    expect(pulseWrapper("n1")?.className).not.toContain("ag-doc-refined");
    expect(pulseWrapper("n2")?.className).toContain("ag-doc-refined");
  });

  it("pulses ALL nodes in a fold that changes EXACTLY 6 nodes — the strobe-suppression rule's own boundary (`> 6`, not `>= 6`)", () => {
    const ids = ["n1", "n2", "n3", "n4", "n5", "n6"];
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        ids.map((id) =>
          note({ id, title: id, body: `body-${id}`, updated_by_sequence: 1 }),
        ),
        1,
      ),
    });
    render(<DocumentTileHarness />);

    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          ids.map((id) =>
            note({
              id,
              title: id,
              body: `body-${id}-edited`,
              updated_by_sequence: 2,
            }),
          ),
          2,
        ),
      });
    });

    for (const id of ids) {
      expect(pulseWrapper(id)?.className).toContain("ag-doc-refined");
    }
  });

  it("a node refined in TWO CONSECUTIVE folds pulses again the SECOND time too, not just the first — the class-identity retrigger guard alone can't tell these apart, so the wrapper must actually remount", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 1 })],
        1,
      ),
    });
    render(<DocumentTileHarness />);
    expect(pulseWrapper("n1")?.className).not.toContain("ag-doc-refined");

    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha-prime",
              updated_by_sequence: 2,
            }),
          ],
          2,
        ),
      });
    });
    const firstPulseEl = pulseWrapper("n1");
    expect(firstPulseEl?.className).toContain("ag-doc-refined");

    // Fold N+1 changes the SAME node's contentHash again, with NO
    // intervening fold that excluded it from `changedNodeIds` — the
    // rendered className string ("...ag-doc-refined") is byte-identical to
    // the previous fold's, so a class-attribute-only retrigger guard would
    // never restart the animation here.
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha-prime-2",
              updated_by_sequence: 3,
            }),
          ],
          3,
        ),
      });
    });
    const secondPulseEl = pulseWrapper("n1");
    expect(secondPulseEl?.className).toContain("ag-doc-refined");
    // A fresh DOM element every time this happens is what GUARANTEES the
    // CSS animation restarts (a newly-created element with the class
    // already present always plays its animation from the start) — same
    // element identity here would mean the retrigger silently didn't fire.
    expect(secondPulseEl).not.toBe(firstPulseEl);
  });

  it("suppresses ALL pulses in a fold that changes more than 6 nodes at once (design-a §1.4's strobe-suppression rule)", () => {
    const ids = ["n1", "n2", "n3", "n4", "n5", "n6", "n7"];
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        ids.map((id) =>
          note({ id, title: id, body: `body-${id}`, updated_by_sequence: 1 }),
        ),
        1,
      ),
    });
    render(<DocumentTileHarness />);

    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          ids.map((id) =>
            note({
              id,
              title: id,
              body: `body-${id}-edited`,
              updated_by_sequence: 2,
            }),
          ),
          2,
        ),
      });
    });

    for (const id of ids) {
      expect(pulseWrapper(id)?.className).not.toContain("ag-doc-refined");
    }
  });
});

describe("DocChangeAnchor — announce-don't-chase (ticket W10, synthesis audio-graph-a6b5 §1.6)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      materializedNotes: null,
      isCapturing: false,
    });
  });

  it("stays hidden for an IN-VIEWPORT change and appears for an OUT-OF-VIEWPORT one — direction-scoped (mocked scroll geometry)", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [
          note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 1 }),
          note({ id: "n2", title: "B", body: "bravo", updated_by_sequence: 2 }),
        ],
        2,
      ),
    });
    const { container } = render(<DocumentTileHarness />);
    const scrollEl = container.querySelector(
      ".overflow-y-auto",
    ) as HTMLDivElement;
    mockScrollGeometry(scrollEl, { scrollHeight: 2000, clientHeight: 100 });

    const n1El = container.querySelector('[data-note-id="n1"]') as HTMLElement;
    const n2El = container.querySelector('[data-note-id="n2"]') as HTMLElement;
    // n1 sits INSIDE the visible [0, 100) window; n2 sits far BELOW it.
    mockNodeRect(n1El, 10, 20, scrollEl);
    mockNodeRect(n2El, 500, 20, scrollEl);

    const aboveAnchor = container.querySelector(
      '[data-direction="above"]',
    ) as HTMLButtonElement;
    const belowAnchor = container.querySelector(
      '[data-direction="below"]',
    ) as HTMLButtonElement;
    // Nothing has changed yet (this is the first fold) — both inert, and
    // both OUT of the Tab order (a hidden, aria-hidden anchor that's still
    // Tab-reachable would be a keyboard "ghost stop" — reachable but
    // invisible and inert).
    expect(aboveAnchor.getAttribute("aria-hidden")).toBe("true");
    expect(belowAnchor.getAttribute("aria-hidden")).toBe("true");
    expect(aboveAnchor.tabIndex).toBe(-1);
    expect(belowAnchor.tabIndex).toBe(-1);

    // A real edit to the ALREADY-VISIBLE n1 must never surface an anchor —
    // the reader can already see the change.
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha-prime",
              updated_by_sequence: 3,
            }),
            note({
              id: "n2",
              title: "B",
              body: "bravo",
              updated_by_sequence: 2,
            }),
          ],
          3,
        ),
      });
    });
    expect(belowAnchor.getAttribute("aria-hidden")).toBe("true");
    expect(aboveAnchor.getAttribute("aria-hidden")).toBe("true");

    // A real edit to the OFF-SCREEN n2 must surface the BELOW anchor with
    // the right count, and leave the ABOVE one untouched.
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha-prime",
              updated_by_sequence: 3,
            }),
            note({
              id: "n2",
              title: "B",
              body: "bravo-charlie",
              updated_by_sequence: 4,
            }),
          ],
          4,
        ),
      });
    });
    expect(belowAnchor.getAttribute("aria-hidden")).toBe("false");
    expect(belowAnchor.textContent).toBe("1 updated below");
    expect(belowAnchor.tabIndex).toBe(0);
    expect(aboveAnchor.getAttribute("aria-hidden")).toBe("true");
  });

  it("clicking the anchor scrolls the container toward the change, dissolves the anchor, and moves focus to the (neutral, non-note) scroll container rather than stranding it on the now aria-hidden button", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 1 })],
        1,
      ),
    });
    const { container } = render(<DocumentTileHarness />);
    const scrollEl = container.querySelector(
      ".overflow-y-auto",
    ) as HTMLDivElement;
    mockScrollGeometry(scrollEl, { scrollHeight: 2000, clientHeight: 100 });
    const n1El = container.querySelector('[data-note-id="n1"]') as HTMLElement;
    mockNodeRect(n1El, 500, 20, scrollEl);

    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha-prime",
              updated_by_sequence: 2,
            }),
          ],
          2,
        ),
      });
    });

    const belowAnchor = container.querySelector(
      '[data-direction="below"]',
    ) as HTMLButtonElement;
    expect(belowAnchor.getAttribute("aria-hidden")).toBe("false");
    expect(scrollEl.scrollTop).toBe(0);

    belowAnchor.focus();
    expect(document.activeElement).toBe(belowAnchor);

    act(() => {
      belowAnchor.click();
    });

    // Scrolled toward the change (centers a 20px-tall node at content
    // offset 500 inside a 100px-tall viewport -> 500 + 10 - 50 = 460).
    expect(scrollEl.scrollTop).toBe(460);
    // Never stole focus onto the NOTE.
    expect(document.activeElement).not.toBe(n1El);
    // Self-dismissing: now that n1 is (per the mocked geometry, recomputed
    // against the new scrollTop) inside the visible window, the anchor
    // dissolves without any explicit close action.
    expect(belowAnchor.getAttribute("aria-hidden")).toBe("true");
    expect(belowAnchor.tabIndex).toBe(-1);
    // Focus moved to the scroll container itself — a real, visible,
    // non-aria-hidden element — rather than staying stranded on the button
    // that just dissolved out of the accessibility tree (axe's
    // `aria-hidden-focus` violation, WCAG 4.1.2).
    expect(document.activeElement).toBe(scrollEl);
    expect(scrollEl.getAttribute("aria-hidden")).not.toBe("true");
  });
});

describe("debounced sr-only announcement (ticket W10, synthesis audio-graph-a6b5 §1.6/L2)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      materializedNotes: null,
      isCapturing: false,
    });
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  function liveRegionText(container: HTMLElement): string {
    return (
      container.querySelector('[role="status"][aria-live="polite"]')
        ?.textContent ?? ""
    );
  }

  it("3 rapid real-edit folds inside the debounce window collapse to exactly ONE sr-only announcement", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [
          note({ id: "n1", title: "A", body: "v1", updated_by_sequence: 1 }),
          note({ id: "n2", title: "B", body: "v1", updated_by_sequence: 2 }),
          note({ id: "n3", title: "C", body: "v1", updated_by_sequence: 3 }),
        ],
        3,
      ),
    });
    const { container } = render(<DocumentTileHarness />);
    expect(liveRegionText(container)).toBe("");

    for (const [id, seq] of [
      ["n1", 4],
      ["n2", 5],
      ["n3", 6],
    ] as const) {
      act(() => {
        useAudioGraphStore.setState((state) => {
          const current = state.materializedNotes;
          if (!current) throw new Error("expected materializedNotes to be set");
          return {
            materializedNotes: {
              ...current,
              last_sequence: seq,
              notes: current.notes.map((n) =>
                n.id === id
                  ? { ...n, body: "edited", updated_by_sequence: seq }
                  : n,
              ),
            },
          };
        });
      });
      act(() => {
        vi.advanceTimersByTime(500);
      });
    }

    // Still inside the (repeatedly reset) 2s window.
    expect(liveRegionText(container)).toBe("");

    act(() => {
      vi.advanceTimersByTime(2000);
    });

    expect(liveRegionText(container)).toBe("Notes updated: 3 passages refined");
  });

  it("once a window flushes, the NEXT fold starts a fresh window and produces its OWN separate announcement", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [note({ id: "n1", title: "A", body: "v1", updated_by_sequence: 1 })],
        1,
      ),
    });
    const { container } = render(<DocumentTileHarness />);

    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [note({ id: "n1", title: "A", body: "v2", updated_by_sequence: 2 })],
          2,
        ),
      });
    });
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(liveRegionText(container)).toBe("Notes updated: 1 passage refined");

    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({ id: "n1", title: "A", body: "v3", updated_by_sequence: 3 }),
            note({ id: "n2", title: "B", body: "v1", updated_by_sequence: 4 }),
          ],
          4,
        ),
      });
    });
    // The PREVIOUS announcement's text must not still be sitting there.
    expect(liveRegionText(container)).toBe("Notes updated: 1 passage refined");
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(liveRegionText(container)).toBe("Notes updated: 2 passages refined");
  });

  it("a LATER window with the SAME passage count as the previous flush still mutates the live region text — otherwise a byte-identical setState is a no-op and a screen reader hears nothing", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [
          note({ id: "n1", title: "A", body: "v1", updated_by_sequence: 1 }),
          note({ id: "n2", title: "B", body: "v1", updated_by_sequence: 2 }),
        ],
        2,
      ),
    });
    const { container } = render(<DocumentTileHarness />);

    // Window 1: n1 changes -> exactly 1 passage refined.
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({ id: "n1", title: "A", body: "v2", updated_by_sequence: 3 }),
            note({ id: "n2", title: "B", body: "v1", updated_by_sequence: 2 }),
          ],
          3,
        ),
      });
    });
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    const first = liveRegionText(container);
    expect(first).toBe("Notes updated: 1 passage refined");

    // Window 2, well after window 1 flushed: a DIFFERENT node changes, but
    // the passage COUNT is the same (1 again) — the steady-state case a
    // long-running live session hits over and over.
    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({ id: "n1", title: "A", body: "v2", updated_by_sequence: 3 }),
            note({ id: "n2", title: "B", body: "v2", updated_by_sequence: 4 }),
          ],
          4,
        ),
      });
    });
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    const second = liveRegionText(container);

    // Must differ at the DOM level (a MutationObserver — the mechanism that
    // actually drives a screen reader's aria-live announcement — would see
    // nothing otherwise) even though the human-legible message is the same.
    expect(second).not.toBe(first);
    expect(second.replace(/\u200b/g, "")).toBe(first);
  });

  it("a pure tail append (appendedAtTail) never announces — the sticky-follow already surfaces it visually (disclosed choice)", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 1 })],
        1,
      ),
    });
    const { container } = render(<DocumentTileHarness />);

    act(() => {
      useAudioGraphStore.setState({
        materializedNotes: materializedNotes(
          [
            note({
              id: "n1",
              title: "A",
              body: "alpha",
              updated_by_sequence: 1,
            }),
            note({
              id: "n2",
              title: "B",
              body: "bravo",
              updated_by_sequence: 2,
            }),
          ],
          2,
        ),
      });
    });
    act(() => {
      vi.advanceTimersByTime(5000);
    });

    expect(liveRegionText(container)).toBe("");
  });

  it("renders EXACTLY ONE aria-live=polite region for the whole document tile, distinct from the recency chip's (non-live) sr-only text", () => {
    useAudioGraphStore.setState({
      materializedNotes: materializedNotes(
        [note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 1 })],
        1,
      ),
      sessionProjectionEvents: [
        notesPatch({ sequence: 1, created_at_ms: 1_700_000_000_000 }),
      ],
      asrSpanRevisions: [],
      loadedSessionId: null,
    });

    function DocumentTileHarnessWithRecencyChip() {
      const vm = useLiveDocumentModel();
      return (
        <WorkspaceTile
          id="document"
          title="Notes"
          headerSlot={
            <span>
              <DocRecencyChip />
              <LiveDocumentHeaderActions vm={vm} />
            </span>
          }
        >
          <LiveDocument vm={vm} />
        </WorkspaceTile>
      );
    }

    const { container } = render(<DocumentTileHarnessWithRecencyChip />);
    // The recency chip DOES render here (a real notes patch exists) and
    // carries its own `.sr-only` text — but with no `aria-live` at all
    // (that chip is a static-until-re-render label, not a live region; see
    // `DocRecencyChip`'s own doc). This ticket's region must be the ONLY
    // `aria-live="polite"` node in the whole tile.
    expect(container.querySelectorAll('[aria-live="polite"]')).toHaveLength(1);
    expect(container.querySelector(".ag-chip .sr-only")).toBeInTheDocument();
  });
});
