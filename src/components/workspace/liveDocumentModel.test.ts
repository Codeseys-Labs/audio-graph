import { describe, expect, it } from "vitest";
import {
  DOC_UNSECTIONED,
  type LiveDocumentSource,
  type LiveDocumentVM,
  notesToOutline,
  outlineToMarkdown,
} from "./liveDocumentModel";

function note(
  overrides: Partial<LiveDocumentSource["notes"][number]> & { id: string },
) {
  return {
    id: overrides.id,
    title: overrides.title ?? "Untitled",
    body: overrides.body ?? "",
    tags: overrides.tags ?? [],
    heading_level: overrides.heading_level ?? null,
    updated_by_sequence: overrides.updated_by_sequence ?? 1,
  };
}

describe("notesToOutline — legacy mode (no heading_level, real field-data shape)", () => {
  // Fixture mirrors the field-triage shape cited in the ticket/constraints:
  // single-utterance verbatim quote captures, ordered, no structure.
  const legacySource: LiveDocumentSource = {
    last_sequence: 3,
    notes: [
      note({
        id: "n1",
        title: "Quote 1",
        body: "We should ship by Friday.",
        tags: ["deadline"],
        updated_by_sequence: 1,
      }),
      note({
        id: "n2",
        title: "Quote 2",
        body: "Per-seat pricing was rejected.",
        updated_by_sequence: 2,
      }),
      note({
        id: "n3",
        title: "Quote 3",
        body: "Finance to confirm the seat floor.",
        updated_by_sequence: 3,
      }),
    ],
  };

  it("renders every note as a flat, unsectioned bullet in artifact order", () => {
    const vm = notesToOutline(null, legacySource);

    expect(vm.sections).toHaveLength(1);
    expect(vm.sections[0].id).toBe(DOC_UNSECTIONED);
    expect(vm.sections[0].heading).toBeNull();
    expect(vm.sections[0].nodes.map((n) => n.id)).toEqual(["n1", "n2", "n3"]);
    expect(vm.sections[0].nodes.map((n) => n.text)).toEqual([
      "We should ship by Friday.",
      "Per-seat pricing was rejected.",
      "Finance to confirm the seat floor.",
    ]);
    // The bold run-in is the note title, exactly today's card title.
    expect(vm.sections[0].nodes[0].lead).toBe("Quote 1");
    expect(vm.sections[0].nodes[0].depth).toBe(0);
    expect(vm.lastSequence).toBe(3);
  });

  it("never fabricates a heading for unsectioned content", () => {
    const vm = notesToOutline(null, legacySource);
    for (const section of vm.sections) {
      if (section.id === DOC_UNSECTIONED) {
        expect(section.heading).toBeNull();
        expect(section.headingLevel).toBeNull();
      }
    }
  });

  it("splits a multi-line body into a primary node plus depth-1 sub-bullets", () => {
    const source: LiveDocumentSource = {
      last_sequence: 1,
      notes: [
        note({
          id: "n1",
          title: "Pricing",
          body: "Tiered pricing agreed for Q3.\n- Finance to confirm the seat floor.\nOpen: does SSO stay in the enterprise tier?",
          updated_by_sequence: 1,
        }),
      ],
    };
    const vm = notesToOutline(null, source);
    const nodes = vm.sections[0].nodes;
    expect(nodes).toHaveLength(3);
    expect(nodes[0]).toMatchObject({
      id: "n1",
      depth: 0,
      lead: "Pricing",
      text: "Tiered pricing agreed for Q3.",
    });
    expect(nodes[1]).toMatchObject({
      id: "n1#1",
      depth: 1,
      lead: null,
      text: "Finance to confirm the seat floor.",
    });
    expect(nodes[2]).toMatchObject({
      id: "n1#2",
      depth: 1,
      lead: null,
      text: "Open: does SSO stay in the enterprise tier?",
    });
  });

  it("treats a doubly-indented body line as a depth-2 sub-bullet, not depth-1", () => {
    const source: LiveDocumentSource = {
      last_sequence: 1,
      notes: [
        note({
          id: "n1",
          title: "Pricing",
          body: "Tiered pricing agreed for Q3.\n- Finance to confirm the seat floor.\n  - Specifically the enterprise tier.",
          updated_by_sequence: 1,
        }),
      ],
    };
    const vm = notesToOutline(null, source);
    const nodes = vm.sections[0].nodes;
    expect(nodes).toHaveLength(3);
    expect(nodes[1]).toMatchObject({ id: "n1#1", depth: 1 });
    expect(nodes[2]).toMatchObject({
      id: "n1#2",
      depth: 2,
      text: "Specifically the enterprise tier.",
    });
  });

  it("returns an empty VM (no fabricated section) when there are no notes yet", () => {
    const vm = notesToOutline(null, { last_sequence: 0, notes: [] });
    expect(vm.sections).toEqual([]);
    expect(vm.changedNodeIds).toEqual([]);
    expect(vm.appendedAtTail).toBe(false);
  });

  it("tolerates a null materialized-notes snapshot", () => {
    const vm = notesToOutline(null, null);
    expect(vm.sections).toEqual([]);
    expect(vm.lastSequence).toBe(0);
  });
});

describe("notesToOutline — heading_level nesting (W1's optional field, tolerated today)", () => {
  const sectioned: LiveDocumentSource = {
    last_sequence: 4,
    notes: [
      note({
        id: "intro",
        title: "Kickoff notes",
        body: "General framing before any topic heading.",
        updated_by_sequence: 1,
      }),
      note({
        id: "pricing",
        title: "Pricing model",
        heading_level: 2,
        body: "Tiered pricing agreed for Q3.",
        updated_by_sequence: 2,
      }),
      note({
        id: "pricing-followup",
        title: "Seat floor",
        body: "Finance to confirm the seat floor.",
        updated_by_sequence: 3,
      }),
      note({
        id: "hiring",
        title: "Hiring",
        heading_level: 3,
        body: "",
        updated_by_sequence: 4,
      }),
    ],
  };

  it("opens a new section on a heading-bearing note and nests following notes under it", () => {
    const vm = notesToOutline(null, sectioned);

    expect(vm.sections.map((s) => s.id)).toEqual([
      DOC_UNSECTIONED,
      "pricing",
      "hiring",
    ]);
    expect(vm.sections[0].nodes.map((n) => n.id)).toEqual(["intro"]);

    const pricingSection = vm.sections[1];
    expect(pricingSection.heading).toBe("Pricing model");
    expect(pricingSection.headingLevel).toBe(2);
    // The heading note's own body becomes a node (lead: null — the title
    // already rendered as the heading), and the following non-heading note
    // nests under the SAME section.
    expect(pricingSection.nodes.map((n) => n.id)).toEqual([
      "pricing",
      "pricing-followup",
    ]);
    expect(pricingSection.nodes[0].lead).toBeNull();

    const hiringSection = vm.sections[2];
    expect(hiringSection.heading).toBe("Hiring");
    // A heading note with an empty body still renders as a real section
    // with zero content nodes — the heading itself is real content.
    expect(hiringSection.nodes).toEqual([]);
  });

  it("clamps an out-of-range heading_level into [2, 4] rather than refusing it", () => {
    const vm = notesToOutline(null, {
      last_sequence: 1,
      notes: [
        note({ id: "a", title: "A", heading_level: 0, updated_by_sequence: 1 }),
        note({
          id: "b",
          title: "B",
          heading_level: 99,
          updated_by_sequence: 1,
        }),
      ],
    });
    const [sectionA, sectionB] = vm.sections;
    expect(sectionA.headingLevel).toBe(2);
    expect(sectionB.headingLevel).toBe(4);
  });
});

describe("notesToOutline — contentHash excludes tags (dedupe pin)", () => {
  const base = note({
    id: "n1",
    title: "Quote",
    body: "We should ship by Friday.",
    tags: ["deadline"],
    updated_by_sequence: 1,
  });

  it("a tags-only upsert changes nothing in the rendered output", () => {
    const first = notesToOutline(null, { last_sequence: 1, notes: [base] });
    expect(first.sections[0].nodes[0].revisions).toBe(0);

    const tagsOnlyChurn: LiveDocumentSource = {
      last_sequence: 2,
      notes: [
        { ...base, tags: ["deadline", "urgent", "q3"], updated_by_sequence: 2 },
      ],
    };
    const second = notesToOutline(first, tagsOnlyChurn);
    const node = second.sections[0].nodes[0];

    // No pulse: nothing landed in changedNodeIds this fold.
    expect(second.changedNodeIds).toEqual([]);
    // No revision bump, no changedAtSeq advance.
    expect(node.revisions).toBe(0);
    expect(node.changedAtSeq).toBe(1);
    // contentHash itself is unchanged even though tags differ.
    expect(node.contentHash).toBe(first.sections[0].nodes[0].contentHash);
    // Tags still ride along for the provenance popover — dedupe hides the
    // churn from the RENDER/change signal, it doesn't drop the data.
    expect(node.tags).toEqual(["deadline", "urgent", "q3"]);
  });

  it("a byte-identical re-upsert (same title/body/tags) produces no pulse", () => {
    const first = notesToOutline(null, { last_sequence: 1, notes: [base] });
    const identical: LiveDocumentSource = {
      last_sequence: 2,
      notes: [{ ...base, updated_by_sequence: 2 }],
    };
    const second = notesToOutline(first, identical);
    expect(second.changedNodeIds).toEqual([]);
    expect(second.sections[0].nodes[0].revisions).toBe(0);
  });

  it("a tags-only churn also leaves appendedAtTail false — it is not an append, and must not trigger sticky-follow", () => {
    const first = notesToOutline(null, { last_sequence: 1, notes: [base] });
    const tagsOnlyChurn: LiveDocumentSource = {
      last_sequence: 2,
      notes: [
        { ...base, tags: ["deadline", "urgent", "q3"], updated_by_sequence: 2 },
      ],
    };
    const second = notesToOutline(first, tagsOnlyChurn);
    expect(second.appendedAtTail).toBe(false);
  });

  it("a real body edit DOES change the hash, bump revisions, and announce", () => {
    const first = notesToOutline(null, { last_sequence: 1, notes: [base] });
    const edited: LiveDocumentSource = {
      last_sequence: 2,
      notes: [
        { ...base, body: "We should ship by Monday.", updated_by_sequence: 2 },
      ],
    };
    const second = notesToOutline(first, edited);
    const node = second.sections[0].nodes[0];
    expect(second.changedNodeIds).toEqual(["n1"]);
    expect(node.revisions).toBe(1);
    expect(node.changedAtSeq).toBe(2);
  });
});

describe("notesToOutline — stable node identity across a patch (no remount of unchanged nodes)", () => {
  const sourceV1: LiveDocumentSource = {
    last_sequence: 2,
    notes: [
      note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 1 }),
      note({ id: "n2", title: "B", body: "bravo", updated_by_sequence: 2 }),
    ],
  };

  it("keeps an untouched node's id, revisions, and changedAtSeq identical across a fold that only edits a SIBLING", () => {
    const first = notesToOutline(null, sourceV1);
    const untouchedBefore = first.sections[0].nodes.find((n) => n.id === "n1");
    expect(untouchedBefore).toBeDefined();

    const sourceV2: LiveDocumentSource = {
      last_sequence: 3,
      notes: [
        sourceV1.notes[0],
        { ...sourceV1.notes[1], body: "bravo-charlie", updated_by_sequence: 3 },
      ],
    };
    const second = notesToOutline(first, sourceV2);
    const untouchedAfter = second.sections[0].nodes.find((n) => n.id === "n1");

    expect(untouchedAfter?.id).toBe("n1");
    expect(untouchedAfter?.contentHash).toBe(untouchedBefore?.contentHash);
    expect(untouchedAfter?.revisions).toBe(untouchedBefore?.revisions);
    expect(untouchedAfter?.changedAtSeq).toBe(untouchedBefore?.changedAtSeq);
    // Only the sibling that actually changed is reported.
    expect(second.changedNodeIds).toEqual(["n2"]);
  });

  it("detects an append-only tail change and flags appendedAtTail", () => {
    const first = notesToOutline(null, sourceV1);
    const appended: LiveDocumentSource = {
      last_sequence: 3,
      notes: [
        ...sourceV1.notes,
        note({ id: "n3", title: "C", body: "charlie", updated_by_sequence: 3 }),
      ],
    };
    const second = notesToOutline(first, appended);
    expect(second.appendedAtTail).toBe(true);
    expect(second.changedNodeIds).toEqual(["n3"]);
  });

  it("does not flag appendedAtTail when a rewrite lands alongside a tail append", () => {
    const first = notesToOutline(null, sourceV1);
    const rewriteAndAppend: LiveDocumentSource = {
      last_sequence: 3,
      notes: [
        { ...sourceV1.notes[0], body: "alpha-prime", updated_by_sequence: 3 },
        sourceV1.notes[1],
        note({ id: "n3", title: "C", body: "charlie", updated_by_sequence: 3 }),
      ],
    };
    const second = notesToOutline(first, rewriteAndAppend);
    expect(second.appendedAtTail).toBe(false);
  });

  it("never flags appendedAtTail on the very first fold of a session", () => {
    const first = notesToOutline(null, sourceV1);
    expect(first.appendedAtTail).toBe(false);
    expect(first.changedNodeIds).toEqual([]);
  });

  it("treats the fold after a sequenced but note-less patch as the effective first fold (no pulse-storm, no forced appendedAtTail)", () => {
    // §W2 acceptance: real accepted patches can carry `operations: []` — a
    // sequenced, zero-notes fold that must not be mistaken for "a real
    // previous fold with content" by the next one.
    const emptyOpsFold = notesToOutline(null, { last_sequence: 1, notes: [] });
    expect(emptyOpsFold.sections).toEqual([]);

    const firstContent: LiveDocumentSource = {
      last_sequence: 2,
      notes: [
        note({ id: "n1", title: "A", body: "alpha", updated_by_sequence: 2 }),
        note({ id: "n2", title: "B", body: "bravo", updated_by_sequence: 2 }),
      ],
    };
    const vm = notesToOutline(emptyOpsFold, firstContent);
    expect(vm.changedNodeIds).toEqual([]);
    expect(vm.appendedAtTail).toBe(false);
    expect(vm.sections[0].nodes.map((n) => n.revisions)).toEqual([0, 0]);
  });
});

describe("outlineToMarkdown — copy/export shape", () => {
  it("serializes unsectioned bullets with a bold lead and no heading line", () => {
    const vm: LiveDocumentVM = {
      sections: [
        {
          id: DOC_UNSECTIONED,
          heading: null,
          headingLevel: null,
          nodes: [
            {
              id: "n1",
              sectionId: DOC_UNSECTIONED,
              depth: 0,
              lead: "Quote 1",
              text: "We should ship by Friday.",
              tags: [],
              seq: 1,
              contentHash: 0,
              changedAtSeq: 1,
              revisions: 0,
            },
          ],
        },
      ],
      lastSequence: 1,
      changedNodeIds: [],
      appendedAtTail: false,
    };
    const md = outlineToMarkdown(vm);
    expect(md).toBe("- **Quote 1** We should ship by Friday.\n");
    expect(md).not.toContain("#");
  });

  it("serializes a heading section with nested depth-1 bullets, in order", () => {
    const vm: LiveDocumentVM = {
      sections: [
        {
          id: "pricing",
          heading: "Pricing model",
          headingLevel: 2,
          nodes: [
            {
              id: "pricing",
              sectionId: "pricing",
              depth: 0,
              lead: null,
              text: "Tiered pricing agreed for Q3.",
              tags: [],
              seq: 1,
              contentHash: 0,
              changedAtSeq: 1,
              revisions: 0,
            },
            {
              id: "pricing#1",
              sectionId: "pricing",
              depth: 1,
              lead: null,
              text: "Finance to confirm the seat floor.",
              tags: [],
              seq: 1,
              contentHash: 0,
              changedAtSeq: 1,
              revisions: 0,
            },
          ],
        },
      ],
      lastSequence: 1,
      changedNodeIds: [],
      appendedAtTail: false,
    };
    const md = outlineToMarkdown(vm);
    expect(md).toBe(
      "## Pricing model\n\n- Tiered pricing agreed for Q3.\n  - Finance to confirm the seat floor.\n",
    );
  });

  it("renders a heading with zero content nodes as a bare heading line", () => {
    const vm: LiveDocumentVM = {
      sections: [
        { id: "hiring", heading: "Hiring", headingLevel: 3, nodes: [] },
      ],
      lastSequence: 1,
      changedNodeIds: [],
      appendedAtTail: false,
    };
    expect(outlineToMarkdown(vm)).toBe("### Hiring\n");
  });
});
