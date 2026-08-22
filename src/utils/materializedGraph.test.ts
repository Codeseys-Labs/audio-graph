import { describe, expect, it } from "vitest";
import type { MaterializedGraph } from "../types";
import {
  fuzzyEntityNameMatch,
  materializedGraphToSnapshot,
} from "./materializedGraph";

function graph(overrides: Partial<MaterializedGraph> = {}): MaterializedGraph {
  return {
    schema_version: 1,
    session_id: "session-1",
    last_sequence: 1,
    nodes: [],
    edges: [],
    ...overrides,
  };
}

describe("materializedGraphToSnapshot", () => {
  it("keeps a present canonical empty graph authoritative", () => {
    expect(materializedGraphToSnapshot(graph())).toEqual({
      nodes: [],
      links: [],
      stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
    });
  });

  it("omits retired nodes and edges from the display snapshot", () => {
    const basis = { transcript_hash: "fnv1a64:test" };
    const provenance = {
      provider: "test",
      model: "test",
      prompt_id: "graph-v1",
    };
    const snapshot = materializedGraphToSnapshot(
      graph({
        nodes: [
          {
            id: "active",
            name: "Active",
            entity_type: "Topic",
            description: null,
            confidence: 0.8,
            valid_from_ms: 1,
            valid_until_ms: null,
            updated_by_sequence: 1,
            updated_at_ms: 2,
            basis,
            provenance,
          },
          {
            id: "retired",
            name: "Retired",
            entity_type: "Topic",
            description: null,
            confidence: 0.8,
            valid_from_ms: 1,
            valid_until_ms: 3,
            updated_by_sequence: 1,
            updated_at_ms: 3,
            basis,
            provenance,
          },
        ],
        edges: [
          {
            id: "retired-edge",
            source: "active",
            target: "retired",
            relation_type: "mentions",
            label: null,
            weight: 1,
            confidence: 0.8,
            valid_from_ms: 1,
            valid_until_ms: null,
            updated_by_sequence: 1,
            updated_at_ms: 2,
            basis,
            provenance,
          },
        ],
      }),
    );

    expect(snapshot?.nodes.map((node) => node.id)).toEqual(["active"]);
    expect(snapshot?.links).toEqual([]);
  });
});

// seed audio-graph-e700 sub-fixes 2/3: mirrors the Rust
// `fuzzy_entity_name_match` unit tests in `src-tauri/src/projections.rs`
// exactly, so the live incremental view and a replayed session never
// disagree about node identity.
describe("fuzzyEntityNameMatch", () => {
  it("merges genuine near-duplicate spellings", () => {
    expect(fuzzyEntityNameMatch("Postgres", "PostgreSQL")).toBe(true);
    expect(fuzzyEntityNameMatch("OpenAI", "Open AI")).toBe(true);
    expect(fuzzyEntityNameMatch("gpt-4", "gpt-4o")).toBe(true);
  });

  it("does not cross-merge distinct names sharing a long common prefix", () => {
    // The exact false-positive class this ticket's field evidence
    // describes: generic model-invented labels that differ only by a
    // trailing enumerator. A plain similarity score (Jaro-Winkler) puts
    // these ABOVE the "postgres"/"postgresql" pair, which is why this
    // function uses a prefix+ratio rule instead.
    expect(fuzzyEntityNameMatch("task 1", "task 2")).toBe(false);
    expect(fuzzyEntityNameMatch("decision 1", "decision 2")).toBe(false);
    expect(fuzzyEntityNameMatch("provider a", "provider b")).toBe(false);
    expect(fuzzyEntityNameMatch("Alice", "Bob")).toBe(false);
  });

  it("does not merge names below the prefix ratio floor", () => {
    expect(fuzzyEntityNameMatch("React", "React Native")).toBe(false);
  });

  // audio-graph-e700 fix: the fuzzy core used to filter to ASCII
  // `[a-z0-9]` only and measure length in UTF-16 code units, disagreeing
  // with the Rust mirror's Unicode-aware `char::is_alphanumeric()` core and
  // BYTE-length ratio for any non-ASCII name — silently diverging the live
  // incremental view from a replayed session's node identity. Both sides
  // now use the same definition (Unicode letters/numbers, length in
  // Unicode code points), so these must match the Rust unit tests exactly.
  it("resolves non-ASCII names identically to the Rust backend", () => {
    // Accented Latin: the ASCII-only core used to strip the accent entirely
    // ("José" -> "jos"), making it a false PREFIX of "jose" at ratio 0.75.
    // Keeping "é" makes "josé"/"jose" NOT a prefix relationship at all.
    expect(fuzzyEntityNameMatch("José", "Jose")).toBe(false);
    // CJK: the ASCII-only core produced an EMPTY string for both names, so
    // this always returned false regardless of similarity. The Unicode
    // core treats Han characters as letters, giving a genuine prefix
    // relationship at ratio 2/3.
    expect(fuzzyEntityNameMatch("東京", "東京都")).toBe(true);
  });
});
