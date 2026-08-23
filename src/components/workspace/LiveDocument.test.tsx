import { act, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
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

  it("never renders data-tone=success at this call site, even given a maximally-healthy-looking store state — phase-1-never-success pin held at the CALL SITE, not just liveWorkspaceTone.ts's own unit tests", () => {
    // `laneRecencyChipTone.test.ts` pins that `evidence: null` structurally
    // blocks success. That pin says nothing about whether THIS call site
    // still passes `evidence: null` — a future edit could thread a real
    // value through DocRecencyChip's `laneRecencyChipTone(...)` call before
    // W3 lands, and nothing here would fail. Assert on the rendered output
    // instead: 0 turns behind, a just-now patch — the most success-looking
    // real store state phase 1 can produce.
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
