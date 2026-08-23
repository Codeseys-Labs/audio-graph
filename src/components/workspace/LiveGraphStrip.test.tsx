import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { useAudioGraphStore } from "../../store";
import type {
  AsrSpanRevisionEvent,
  GraphSnapshot,
  MaterializedGraph,
  ProjectionPatch,
} from "../../types";
import {
  GraphRecencyChip,
  type GraphStripMode,
  GraphStripModeSwitcher,
  LiveGraphStrip,
  useGraphStripMode,
} from "./LiveGraphStrip";

const GRAPH_STRIP_MODE_STORAGE_KEY = "ag.graphStripMode";

const EMPTY_SNAPSHOT: GraphSnapshot = {
  nodes: [],
  links: [],
  stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
};

function materializedGraph(
  overrides: Partial<MaterializedGraph> = {},
): MaterializedGraph {
  return {
    schema_version: 1,
    session_id: "s1",
    last_sequence: 1,
    nodes: [],
    edges: [],
    ...overrides,
  };
}

function materializedNode(id: string, seq: number) {
  return {
    id,
    name: `Node ${id}`,
    entity_type: "person",
    confidence: 1,
    valid_from_ms: 0,
    updated_by_sequence: seq,
    updated_at_ms: seq,
    basis: null,
    provenance: null,
  };
}

/**
 * `Element.textContent`/`Node.textContent` concatenates every descendant
 * text node with NO separator — `<span>Live</span><span>Nodes: 1</span>`
 * reads back as `"LiveNodes: 1"`, so a word-boundary regex (`/\bLive\b/`)
 * never finds a boundary between "Live" and "Nodes" and an element-adjacent
 * "Live" badge (the most realistic leak shape) escapes detection entirely.
 * Walk the DOM and join each individual text node with an explicit space
 * instead, so every element boundary becomes a real word boundary.
 */
function allTextJoinedWithSpaces(root: Node): string {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const parts: string[] = [];
  let node = walker.nextNode();
  while (node) {
    if (node.textContent) parts.push(node.textContent);
    node = walker.nextNode();
  }
  return parts.join(" ");
}

function graphPatch(overrides: Partial<ProjectionPatch>): ProjectionPatch {
  return {
    sequence: 1,
    kind: "graph",
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

/** Harness pairing the header-slot switcher with the persisted hook, the
 * same split `App.tsx` uses (one call site, two consumers). */
function ModeSwitcherHarness() {
  const [mode, setMode] = useGraphStripMode();
  return (
    <div>
      <GraphStripModeSwitcher mode={mode} onModeChange={setMode} />
      <span data-testid="current-mode">{mode}</span>
    </div>
  );
}

function LiveGraphStripHarness() {
  const [mode, setMode] = useGraphStripMode();
  return <LiveGraphStrip mode={mode} onModeChange={setMode} />;
}

describe("LiveGraphStrip — shared empty state (ticket W7)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      materializedProjectionGraph: null,
      graphSnapshot: EMPTY_SNAPSHOT,
      sessionProjectionEvents: [],
      loadedSessionId: null,
    });
    localStorage.clear();
  });

  it.each<GraphStripMode>([
    "focus",
    "canvas",
    "feed",
  ])("renders workspace.tile.graphEmpty for zero nodes in %s mode, never a mode-specific empty copy", (mode) => {
    localStorage.setItem(GRAPH_STRIP_MODE_STORAGE_KEY, mode);
    render(<LiveGraphStripHarness />);
    expect(screen.getByText("No graph activity yet")).toBeInTheDocument();
  });
});

describe("LiveGraphStrip — mode persistence round-trip (ticket W7)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      materializedProjectionGraph: null,
      graphSnapshot: EMPTY_SNAPSHOT,
      sessionProjectionEvents: [],
      loadedSessionId: null,
    });
    localStorage.clear();
  });

  it('defaults to "focus" with nothing persisted yet', () => {
    render(<ModeSwitcherHarness />);
    expect(screen.getByTestId("current-mode")).toHaveTextContent("focus");
  });

  it("persists a mode switch to localStorage and a fresh mount reads it back", () => {
    const { unmount } = render(<ModeSwitcherHarness />);
    fireEvent.click(screen.getByRole("tab", { name: "Canvas" }));
    expect(screen.getByTestId("current-mode")).toHaveTextContent("canvas");
    expect(localStorage.getItem(GRAPH_STRIP_MODE_STORAGE_KEY)).toBe("canvas");

    unmount();
    render(<ModeSwitcherHarness />);
    expect(screen.getByTestId("current-mode")).toHaveTextContent("canvas");
    expect(screen.getByRole("tab", { name: "Canvas" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("discards an invalid persisted value and falls back to focus", () => {
    localStorage.setItem(GRAPH_STRIP_MODE_STORAGE_KEY, "not-a-real-mode");
    render(<ModeSwitcherHarness />);
    expect(screen.getByTestId("current-mode")).toHaveTextContent("focus");
  });
});

describe("LiveGraphStrip — total-size counter reflects the FULL graph, not the focused subset (ticket W7)", () => {
  it("shows the total node count even when it exceeds the 12-node focus cap", () => {
    const nodes = Array.from({ length: 20 }, (_, i) =>
      materializedNode(`n${i}`, i),
    );
    useAudioGraphStore.setState({
      materializedProjectionGraph: materializedGraph({ nodes }),
      graphSnapshot: EMPTY_SNAPSHOT,
      sessionProjectionEvents: [],
      loadedSessionId: null,
    });
    localStorage.setItem(GRAPH_STRIP_MODE_STORAGE_KEY, "focus");
    render(<LiveGraphStripHarness />);
    expect(screen.getByText("Nodes: 20")).toBeInTheDocument();
  });
});

/**
 * Regression for the fix-agent finding: the hysteresis tick used to advance
 * on `useActiveGraphSnapshot`'s merged `snapshot` object REFERENCE, which
 * changes on EITHER `materializedProjectionGraph` or the legacy
 * `graphSnapshot` — so a paired backend full replace of the SAME accepted
 * patch (or any legacy-lane event) ticked the hysteresis with no real graph
 * patch behind it. Reproduces the exact repro from the finding: 5
 * sequential accepted patches put a node into hysteresis-held state
 * (`ticksUntouched === 1`), then two reference-only, same-`last_sequence`
 * updates (standing in for the paired `MATERIALIZED_GRAPH_UPDATE` replace)
 * must NOT evict it — only a change to `last_sequence` may advance a tick.
 */
describe("LiveGraphStrip — hysteresis ticks are patch-scoped, not merged-snapshot-reference-scoped (fix-agent regression)", () => {
  function namedMaterializedNode(name: string, seq: number) {
    return {
      id: name,
      name,
      entity_type: "person",
      confidence: 1,
      valid_from_ms: 0,
      updated_by_sequence: seq,
      updated_at_ms: seq,
      basis: null,
      provenance: null,
    };
  }

  beforeEach(() => {
    localStorage.setItem(GRAPH_STRIP_MODE_STORAGE_KEY, "focus");
  });

  it("survives a same-last_sequence full-replace (no accepted patch behind it) instead of eviction with zero real patches", () => {
    // Ticks 1-5: one accepted patch each, adding "Aaa".."Eee" in order.
    // After tick 5, "Bbb" (touched at tick 2, out of the top-3-sequence
    // window since tick 4) sits at ticksUntouched=1 — see the fix agent's
    // hand-traced derivation in its report.
    const names = ["Aaa", "Bbb", "Ccc", "Ddd", "Eee"];
    useAudioGraphStore.setState({
      materializedProjectionGraph: materializedGraph({
        last_sequence: 1,
        nodes: [namedMaterializedNode(names[0], 1)],
      }),
      graphSnapshot: EMPTY_SNAPSHOT,
      sessionProjectionEvents: [],
      loadedSessionId: null,
    });
    render(<LiveGraphStripHarness />);
    for (let seq = 2; seq <= 5; seq += 1) {
      act(() => {
        useAudioGraphStore.setState({
          materializedProjectionGraph: materializedGraph({
            last_sequence: seq,
            nodes: names
              .slice(0, seq)
              .map((name, i) => namedMaterializedNode(name, i + 1)),
          }),
        });
      });
    }
    expect(screen.getByText("Bbb")).toBeInTheDocument();

    // Two reference-only updates carrying the IDENTICAL last_sequence (5) —
    // a brand-new `MaterializedGraph` object, same content, standing in for
    // the backend's paired full replace of the patch already folded above.
    // Zero accepted patches happened; the hysteresis clock must not move.
    for (let i = 0; i < 2; i += 1) {
      act(() => {
        useAudioGraphStore.setState({
          materializedProjectionGraph: materializedGraph({
            last_sequence: 5,
            nodes: names.map((name, idx) =>
              namedMaterializedNode(name, idx + 1),
            ),
          }),
        });
      });
    }
    expect(screen.getByText("Bbb")).toBeInTheDocument();

    // Sanity: a REAL 6th accepted patch (last_sequence advances to 6) DOES
    // tick — confirms the gate isn't simply broken to never advance at all.
    act(() => {
      useAudioGraphStore.setState({
        materializedProjectionGraph: materializedGraph({
          last_sequence: 6,
          nodes: [
            ...names.map((name, idx) => namedMaterializedNode(name, idx + 1)),
            namedMaterializedNode("Fff", 6),
          ],
        }),
      });
    });
    expect(screen.getByText("Fff")).toBeInTheDocument();
  });
});

describe("LiveGraphStrip — honesty: never contains 'Live'; the OLD inline as-of text is gone (ticket W6, synthesis audio-graph-a6b5 §2)", () => {
  beforeEach(() => {
    localStorage.setItem(GRAPH_STRIP_MODE_STORAGE_KEY, "focus");
    useAudioGraphStore.setState({ asrSpanRevisions: [] });
  });

  it("LiveGraphStrip's own body renders NO 'as of' text and NO [data-tone] chip — ticket W6 moved the freshness claim to GraphRecencyChip in the tile's headerSlot, so there is exactly one source of truth, not two", () => {
    useAudioGraphStore.setState({
      materializedProjectionGraph: materializedGraph({
        nodes: [materializedNode("n1", 1)],
      }),
      graphSnapshot: EMPTY_SNAPSHOT,
      sessionProjectionEvents: [
        graphPatch({ sequence: 1, created_at_ms: 1_700_000_000_000 }),
      ],
      loadedSessionId: null,
    });
    render(<LiveGraphStripHarness />);
    // The whole rendered strip — header row, chips, everything — must never
    // contain "Live" anywhere. Word-boundary-joined (NOT raw `.textContent`,
    // which glues adjacent elements' text together with no separator and
    // would let an element-adjacent `<span>Live</span>` badge escape
    // detection — see `allTextJoinedWithSpaces`'s doc comment).
    expect(allTextJoinedWithSpaces(document.body)).not.toMatch(/\bLive\b/);
    // Pre-W6, `LiveGraphStrip` rendered its OWN plain "Graph as of ..." text
    // and no chip. W6 deletes that render site outright (not just adds a
    // second one) — this is the "old text is gone" acceptance criterion.
    expect(screen.queryByText(/as of/i)).not.toBeInTheDocument();
    expect(document.querySelector("[data-tone]")).not.toBeInTheDocument();
  });

  it("mutation-proof: a 'Live' badge adjacent to another element (no separator in raw textContent) is still caught", () => {
    useAudioGraphStore.setState({
      materializedProjectionGraph: materializedGraph({
        nodes: [materializedNode("n1", 1)],
      }),
      graphSnapshot: EMPTY_SNAPSHOT,
      sessionProjectionEvents: [
        graphPatch({ sequence: 1, created_at_ms: 1_700_000_000_000 }),
      ],
      loadedSessionId: null,
    });
    const { container } = render(<LiveGraphStripHarness />);
    const header = container.querySelector("div > div");
    const badge = document.createElement("span");
    badge.textContent = "Live"; // no leading/trailing whitespace, by design
    header?.insertBefore(badge, header.firstChild);

    // Raw textContent glues "Live" onto whatever follows it with no
    // separator — demonstrating why this assertion, not
    // `document.body.textContent`, is the one that must be used.
    expect(document.body.textContent).toMatch(/Live/);
    expect(allTextJoinedWithSpaces(document.body)).toMatch(/\bLive\b/);
  });
});

describe("GraphRecencyChip — the tone-routed freshness chip (ticket W6, synthesis audio-graph-a6b5 §2)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [],
      asrSpanRevisions: [],
      loadedSessionId: null,
    });
  });

  it("renders data-tone=neutral with the 'as of' text when fewer than 3 finalized turns have passed since the last graph patch", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        graphPatch({ sequence: 1, created_at_ms: 1_700_000_000_000 }),
      ],
      asrSpanRevisions: [
        finalizedRevision({ received_at_ms: 1_700_000_001_000, turn_id: "t1" }),
      ],
      loadedSessionId: null,
    });
    render(<GraphRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toBeInTheDocument();
    expect(chip).toHaveAttribute("data-tone", "neutral");
    expect(chip?.textContent).toMatch(/as of/i);
    expect(allTextJoinedWithSpaces(document.body)).not.toMatch(/\bLive\b/);
    // See `DocRecencyChip`'s (LiveDocument.tsx) identical regression guard:
    // this test's runtime resolves to en-US, which renders AM/PM unless the
    // call site pins an explicit locale + `hour12: false`.
    expect(chip?.textContent).not.toMatch(/AM|PM/i);
  });

  it("renders data-tone=warning with '-N turns' at the ratified >=3 threshold", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        graphPatch({ sequence: 1, created_at_ms: 1_700_000_000_000 }),
      ],
      asrSpanRevisions: [
        finalizedRevision({ received_at_ms: 1_700_000_001_000, turn_id: "t1" }),
        finalizedRevision({ received_at_ms: 1_700_000_002_000, turn_id: "t2" }),
        finalizedRevision({ received_at_ms: 1_700_000_003_000, turn_id: "t3" }),
      ],
      loadedSessionId: null,
    });
    render(<GraphRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toHaveAttribute("data-tone", "warning");
    expect(chip?.textContent).toMatch(/3/);
    expect(chip?.textContent).not.toMatch(/as of/i);
  });

  it("2 finalized turns behind stays neutral (the warning threshold is exclusive below 3)", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        graphPatch({ sequence: 1, created_at_ms: 1_700_000_000_000 }),
      ],
      asrSpanRevisions: [
        finalizedRevision({ received_at_ms: 1_700_000_001_000, turn_id: "t1" }),
        finalizedRevision({ received_at_ms: 1_700_000_002_000, turn_id: "t2" }),
      ],
      loadedSessionId: null,
    });
    render(<GraphRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toHaveAttribute("data-tone", "neutral");
  });

  it("without W3 evidence, never renders data-tone=success at this call site, even given a maximally-healthy-looking store state — absent-evidence pin held at the CALL SITE, not just liveWorkspaceTone.ts's own unit tests", () => {
    // See `DocRecencyChip`'s (LiveDocument.tsx) identical test: the module's
    // own unit tests pin that `evidence: null` blocks success, but nothing
    // previously asserted that THIS call site maps a patch with no
    // `basis_currency_at_apply` to `evidence: null`.
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        graphPatch({ sequence: 1, created_at_ms: Date.now() }),
      ],
      asrSpanRevisions: [],
      loadedSessionId: null,
    });
    render(<GraphRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toBeInTheDocument();
    expect(chip).not.toHaveAttribute("data-tone", "success");
    expect(chip).toHaveAttribute("data-tone", "neutral");
  });

  it("ticket W3 fix round: renders data-tone=success AND the 'Up to date' copy (not the color-only regression the fix-round review caught) when the latest graph patch carried {type: 'current'} — the ONLY input that can ever earn it", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        graphPatch({
          sequence: 1,
          created_at_ms: Date.now(),
          basis_currency_at_apply: { type: "current" },
        }),
      ],
      asrSpanRevisions: [],
      loadedSessionId: null,
    });
    render(<GraphRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toBeInTheDocument();
    expect(chip).toHaveAttribute("data-tone", "success");
    // Legible without color, and with the GRAPH-lane aria variant, not the
    // document chip's.
    const visible = chip?.querySelector('[aria-hidden="true"]');
    expect(visible?.textContent).toBe("Up to date");
    const srOnly = chip?.querySelector(".sr-only");
    expect(srOnly?.textContent).toMatch(/graph is up to date/i);
  });

  it("ticket W3: {type: 'appended_tail'} is present evidence of lag, not current-ness — stays neutral, never upgraded to success", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        graphPatch({
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
    render(<GraphRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toBeInTheDocument();
    expect(chip).not.toHaveAttribute("data-tone", "success");
    expect(chip).toHaveAttribute("data-tone", "neutral");
    expect(chip?.textContent).toMatch(/as of/i);
    expect(chip?.textContent).not.toMatch(/up to date/i);
  });

  it("ticket W3: a malformed/unrecognized basis_currency_at_apply.type maps to no evidence, not a crash or a success", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        graphPatch({
          sequence: 1,
          created_at_ms: Date.now(),
          // @ts-expect-error — deliberately malformed wire value.
          basis_currency_at_apply: { type: "not_a_real_tag" },
        }),
      ],
      asrSpanRevisions: [],
      loadedSessionId: null,
    });
    render(<GraphRecencyChip />);
    const chip = document.querySelector("[data-tone]");
    expect(chip).toBeInTheDocument();
    expect(chip).not.toHaveAttribute("data-tone", "success");
    expect(chip).toHaveAttribute("data-tone", "neutral");
  });

  it("renders nothing when the graph lane has never produced an accepted patch this session", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [],
      asrSpanRevisions: [],
      loadedSessionId: null,
    });
    render(<GraphRecencyChip />);
    expect(document.querySelector("[data-tone]")).not.toBeInTheDocument();
  });

  it("renders nothing for a loaded/reviewed session — no freshness claim about a finished session's own history", () => {
    useAudioGraphStore.setState({
      sessionProjectionEvents: [
        graphPatch({ sequence: 1, created_at_ms: 1_700_000_000_000 }),
      ],
      asrSpanRevisions: [
        finalizedRevision({ received_at_ms: 1_700_000_001_000, turn_id: "t1" }),
        finalizedRevision({ received_at_ms: 1_700_000_002_000, turn_id: "t2" }),
        finalizedRevision({ received_at_ms: 1_700_000_003_000, turn_id: "t3" }),
      ],
      loadedSessionId: "recorded-session-1",
    });
    render(<GraphRecencyChip />);
    expect(document.querySelector("[data-tone]")).not.toBeInTheDocument();
    expect(allTextJoinedWithSpaces(document.body)).not.toMatch(/\bLive\b/);
  });
});

describe("LiveGraphStrip — feed mode renders adds/renames from a patch fixture (ticket W7)", () => {
  beforeEach(() => {
    localStorage.setItem(GRAPH_STRIP_MODE_STORAGE_KEY, "feed");
  });

  it("renders an added node, a renamed node, and a new edge as three distinct lines", () => {
    useAudioGraphStore.setState({
      materializedProjectionGraph: materializedGraph({
        nodes: [materializedNode("n1", 1), materializedNode("n2", 1)],
      }),
      graphSnapshot: EMPTY_SNAPSHOT,
      loadedSessionId: null,
      sessionProjectionEvents: [
        graphPatch({
          sequence: 1,
          operations: [
            {
              type: "upsert_graph_node",
              id: "n1",
              name: "Acme Corp",
              entity_type: "organization",
            },
            {
              type: "upsert_graph_node",
              id: "n2",
              name: "Postgres",
              entity_type: "product",
            },
          ],
        }),
        graphPatch({
          sequence: 2,
          operations: [
            {
              type: "upsert_graph_node",
              id: "n1",
              name: "Acme Corporation",
              entity_type: "organization",
            },
            {
              type: "upsert_graph_edge",
              id: "e1",
              source: "n1",
              target: "n2",
              relation_type: "evaluates",
              weight: 1,
            },
          ],
        }),
      ],
    });
    render(<LiveGraphStripHarness />);
    expect(screen.getByText("+ Postgres (product)")).toBeInTheDocument();
    expect(screen.getByText("~ Acme Corporation renamed")).toBeInTheDocument();
    expect(
      screen.getByText("Acme Corporation -> Postgres"),
    ).toBeInTheDocument();
  });

  it("routes entity_type through the i18n entityType namespace instead of interpolating the raw wire value", () => {
    useAudioGraphStore.setState({
      materializedProjectionGraph: materializedGraph({
        nodes: [materializedNode("n1", 1)],
      }),
      graphSnapshot: EMPTY_SNAPSHOT,
      loadedSessionId: null,
      sessionProjectionEvents: [
        graphPatch({
          sequence: 1,
          operations: [
            // PascalCase, matching the ontology's own wire casing
            // (`src-tauri/src/ontology.rs`'s `ENTITY_TYPES`) — a naive
            // `{{entityType}}` interpolation of the raw value would render
            // "Organization" (capitalized, untranslated); the i18n-mapped
            // value is the lowercase translated string instead.
            {
              type: "upsert_graph_node",
              id: "n1",
              name: "Acme Corp",
              entity_type: "Organization",
            },
          ],
        }),
      ],
    });
    render(<LiveGraphStripHarness />);
    expect(screen.getByText("+ Acme Corp (organization)")).toBeInTheDocument();
    expect(screen.queryByText(/Organization/)).not.toBeInTheDocument();
  });
});
