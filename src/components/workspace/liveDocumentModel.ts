/**
 * `liveDocumentModel` — the living-document typed-outline view-model
 * (ticket W5, synthesis audio-graph-a6b5). Pure, framework-free, and
 * unit-testable without a DOM: `LiveDocument.tsx` is the only consumer.
 *
 * Binding decision (synthesis §2, design-a §1.1): the document is a typed
 * outline in the store, not markdown text re-parsed on every tick. Per-node
 * stable ids (derived from `MaterializedNote.id`) mean React patches nodes
 * in place instead of remounting a freshly re-parsed tree — `outlineToMarkdown`
 * exists purely for the copy/export path, never for rendering.
 *
 * LEGACY MODE IS THE SHIPPING MODE (R4, gate-narrowed): today's wire data
 * carries no `heading_level` on any note — W1 (audio-graph-a6b5) has not
 * landed on the Rust side. `notesToOutline` must therefore produce an
 * honest, readable outline from the ordered `MaterializedNotes.notes` array
 * AS-IS, with zero fabricated structure. When `heading_level` starts
 * appearing on real notes this same function renders sections without a
 * second edit — the two code paths below (heading vs. no heading) are
 * exercised by every call, driven purely by what each note happens to
 * carry.
 *
 * Sections are intentionally FLAT, not a nested tree: a note carrying
 * `heading_level` opens a new section (heading = that note's title); every
 * following note without its own `heading_level` nests under it, until the
 * next heading-bearing note starts a new one. This is R4's ratified
 * narrowing — "sections come only from model-emitted heading_level, no
 * ontology-derived sections" — and deliberately does not attempt
 * design-b's `clamp(heading_level ?? 3, prevLevel+1, 4)` multi-level
 * heading NESTING (a section-of-sections tree); `headingLevel` is kept only
 * to pick which heading TAG (`h2`/`h3`/`h4`) a section renders as, absorbed
 * without another view-model shape change if a future ticket wants real
 * nesting.
 */

export const DOC_UNSECTIONED = "__doc_unsectioned__";

/** A note's `heading_level` is clamped into this range once it exists,
 * mirroring the Rust-side normalizer design-b describes (§1.4) — never
 * trust a wire value blindly, even though W1 clamps server-side too. */
const MIN_HEADING_LEVEL = 2;
const MAX_HEADING_LEVEL = 4;

/** Minimal shape this module needs from `MaterializedNote` /
 * `MaterializedNotes` (`../../types`) — declared locally so this file has
 * zero import surface and stays trivially unit-testable; the real types
 * are structurally compatible and callers pass them directly. */
export interface LiveDocumentSourceNote {
  id: string;
  title: string;
  body: string;
  tags: string[];
  heading_level?: number | null;
  updated_by_sequence: number;
}

export interface LiveDocumentSource {
  last_sequence: number;
  notes: LiveDocumentSourceNote[];
}

/**
 * One rendered outline entry. `id` is the ONLY thing React keys on — it
 * must stay stable across a patch that doesn't change this node's content,
 * or the "no remount of unchanged nodes" contract (W5 test list) breaks.
 */
export interface DocNode {
  /** `note.id` for a note's first line; `${note.id}#${i}` for line `i` of
   * a multi-line body (see `splitBodyLines`). Stable across re-folds as
   * long as the note's line count doesn't change. */
  id: string;
  /** The section this node renders under — a heading note's id, or
   * `DOC_UNSECTIONED`. */
  sectionId: string;
  /** 0 = the note's own line; 1 = an indented sub-bullet parsed out of a
   * multi-line body. Reserved up to 2 (design-a §1.2's hard cap) for a
   * future body grammar that can express one more indent level than
   * today's plain-text line splitter does. */
  depth: 0 | 1 | 2;
  /** Bold run-in shown before `text` — `note.title` on a node's own first
   * line, `null` on every sub-bullet and on a heading note's own body
   * (the title already rendered as that section's heading). */
  lead: string | null;
  text: string;
  tags: string[];
  /** `updated_by_sequence`, verbatim — debugging-only, never rendered on
   * the primary surface (design-a §1.5: `seq` leaves the visual surface). */
  seq: number;
  /** djb2 of `${lead ?? ""} ${text}` — TAGS EXCLUDED BY DESIGN. The
   * field-measured 37% byte-identical + 35% tags-only-churn upsert classes
   * must never change this value, so neither ever produces a visual pulse,
   * a revision bump, or an announcement. */
  contentHash: number;
  /** The note's own `updated_by_sequence` the last time `contentHash`
   * changed (not the last time this note was merely re-upserted). */
  changedAtSeq: number;
  /** Count of hash-CHANGING upserts observed since this node was first
   * folded, not the raw upsert count. */
  revisions: number;
}

export interface DocSection {
  /** A heading note's id, or `DOC_UNSECTIONED`. */
  id: string;
  /** `null` for the unsectioned bucket — never fabricated (design-a §1.2:
   * "No fabricated heading"). */
  heading: string | null;
  /** Clamped `heading_level`, or `null` when `heading === null`. Only
   * meaningful for picking a heading tag; see the module doc. */
  headingLevel: number | null;
  nodes: DocNode[];
}

export interface LiveDocumentVM {
  sections: DocSection[];
  lastSequence: number;
  /** Node ids whose `contentHash` changed (or that are newly present) in
   * THIS fold only — never accumulated across folds. Empty on the very
   * first fold of a session (nothing to compare against yet — folding a
   * freshly loaded/replayed session must not announce every node as
   * "just changed"). The "model hooks" W10 builds pulse/announcement
   * polish on top of; W5 renders no motion from this list itself. */
  changedNodeIds: string[];
  /**
   * True when this fold's only structural difference from the previous
   * one is new node(s) appended at the very end of the document — i.e. no
   * existing node moved, and no existing node's content changed. This is
   * the signal `LiveDocument`'s L1 sticky-follow gates on (append-only
   * follow; a rewrite anywhere else in the document never autoscrolls).
   * Always `false` on the first fold of a session (nothing to compare
   * against, and a freshly mounted/replayed document must not jump).
   */
  appendedAtTail: boolean;
}

const EMPTY_VM: LiveDocumentVM = {
  sections: [],
  lastSequence: 0,
  changedNodeIds: [],
  appendedAtTail: false,
};

/** djb2 — small, dependency-free, and stable across engines/runs (unlike
 * `Object.prototype.toString`-based hashing or `JSON.stringify` + length
 * shortcuts). Collisions are harmless here: a false-negative "unchanged"
 * would just mean a real edit doesn't pulse, which — at this string-space
 * size — is not a risk worth a heavier hash for. */
function djb2(input: string): number {
  let hash = 5381;
  for (let i = 0; i < input.length; i++) {
    hash = (hash * 33) ^ input.charCodeAt(i);
  }
  return hash >>> 0;
}

function contentHashOf(lead: string | null, text: string): number {
  return djb2(`${lead ?? ""} ${text}`);
}

function clampHeadingLevel(level: number): number {
  if (!Number.isFinite(level)) return MIN_HEADING_LEVEL;
  return Math.min(
    MAX_HEADING_LEVEL,
    Math.max(MIN_HEADING_LEVEL, Math.round(level)),
  );
}

/**
 * Split a note body into rendered lines. Line 0 is always the node's own
 * text; every following non-empty line becomes a `depth: 1` (or `2`, for a
 * doubly-indented line) sub-bullet. This is a plain-text heuristic, NOT
 * design-b's shared W1 body grammar (§1.3/§4.1) — that grammar lives in the
 * Rust normalizer and hasn't landed; this heuristic reads whatever
 * structure already exists in today's body text (blank-line-separated
 * lines, optional leading `-`/`*`/`+` markers, optional indent) rather than
 * inventing any. When W1's grammar lands this function is the one seam a
 * follow-up ticket replaces — the rest of the model is unaffected.
 */
function splitBodyLines(body: string): Array<{ depth: 1 | 2; text: string }> {
  const rawLines = body.split("\n");
  const nonEmpty = rawLines.filter((line) => line.trim().length > 0);
  const source = nonEmpty.length > 0 ? nonEmpty : [body];
  return source.slice(1).map((raw) => {
    const leadingSpaces = raw.match(/^ */)?.[0].length ?? 0;
    const depth: 1 | 2 = leadingSpaces >= 2 ? 2 : 1;
    const text = raw.trim().replace(/^[-*+]\s+/, "");
    return { depth, text };
  });
}

function firstLine(body: string): string {
  const nonEmpty = body.split("\n").find((line) => line.trim().length > 0);
  return (nonEmpty ?? body).trim();
}

/** Look up a previously folded node by id across every section. Built once
 * per fold, not per node. */
function indexPreviousNodes(
  previous: LiveDocumentVM | null,
): Map<string, DocNode> {
  const index = new Map<string, DocNode>();
  if (!previous) return index;
  for (const section of previous.sections) {
    for (const node of section.nodes) index.set(node.id, node);
  }
  return index;
}

function foldNode(
  id: string,
  sectionId: string,
  depth: 0 | 1 | 2,
  lead: string | null,
  text: string,
  tags: string[],
  seq: number,
  previousById: Map<string, DocNode>,
  hasPreviousFold: boolean,
  changedNodeIds: string[],
): DocNode {
  const contentHash = contentHashOf(lead, text);
  const prior = previousById.get(id);
  if (prior) {
    if (prior.contentHash === contentHash) {
      return {
        id,
        sectionId,
        depth,
        lead,
        text,
        tags,
        seq,
        contentHash,
        changedAtSeq: prior.changedAtSeq,
        revisions: prior.revisions,
      };
    }
    changedNodeIds.push(id);
    return {
      id,
      sectionId,
      depth,
      lead,
      text,
      tags,
      seq,
      contentHash,
      changedAtSeq: seq,
      revisions: prior.revisions + 1,
    };
  }
  // New node. Only announce it as "changed" once there was a previous fold
  // to be new RELATIVE TO — a session's first fold has nothing to compare
  // against, so nothing in it is a "change" yet (see `changedNodeIds` doc).
  if (hasPreviousFold) changedNodeIds.push(id);
  return {
    id,
    sectionId,
    depth,
    lead,
    text,
    tags,
    seq,
    contentHash,
    changedAtSeq: seq,
    revisions: 0,
  };
}

function appendNoteNodes(
  section: DocSection,
  note: LiveDocumentSourceNote,
  lead: string | null,
  previousById: Map<string, DocNode>,
  hasPreviousFold: boolean,
  changedNodeIds: string[],
): void {
  // A heading note (`lead === null`, since its title already became the
  // section heading) with an empty body contributes NO content node — the
  // heading alone is real content; fabricating an empty bullet under it
  // would not be. A non-heading note always contributes its own node even
  // with an empty body: `lead` (the title) is the only thing that note has
  // to show, and dropping it would silently delete the note from view.
  if (lead === null && note.body.trim().length === 0) return;
  const primaryText = firstLine(note.body);
  section.nodes.push(
    foldNode(
      note.id,
      section.id,
      0,
      lead,
      primaryText,
      note.tags,
      note.updated_by_sequence,
      previousById,
      hasPreviousFold,
      changedNodeIds,
    ),
  );
  const rest = splitBodyLines(note.body);
  rest.forEach((line, i) => {
    const id = `${note.id}#${i + 1}`;
    section.nodes.push(
      foldNode(
        id,
        section.id,
        line.depth,
        null,
        line.text,
        note.tags,
        note.updated_by_sequence,
        previousById,
        hasPreviousFold,
        changedNodeIds,
      ),
    );
  });
}

function flatNodeOrder(vm: LiveDocumentVM): string[] {
  return vm.sections.flatMap((section) => section.nodes.map((node) => node.id));
}

/**
 * True iff `vm` reflects a fold that actually rendered SOMETHING (at least
 * one section — a bullet or a bare heading). `notesToOutline` returns a
 * structurally-empty VM (`sections: []`) both when `previous` is `null` (a
 * brand-new session) AND when a real, sequenced, zero-notes patch was
 * folded (§W2 acceptance: ≥15% of ticks legitimately carry
 * `operations: []`, and those patches persist). Both cases must be treated
 * as "no prior fold to diff against" — otherwise the fold immediately
 * following an empty-ops patch sees a non-null-but-empty `previous` and
 * wrongly reports every node in the first real batch as newly "changed"
 * plus `appendedAtTail: true`, defeating the very guard `changedNodeIds`/
 * `appendedAtTail` document for a session's first fold.
 */
function hadPriorRenderedContent(
  vm: LiveDocumentVM | null,
): vm is LiveDocumentVM {
  return vm !== null && vm.sections.length > 0;
}

/**
 * True iff `next`'s full node order is exactly `previous`'s, with one or
 * more ids appended at the end, AND none of the carried-over ids are in
 * `changedNodeIds` (a rewrite anywhere in the existing document — even one
 * that happens to land alongside a tail append — disqualifies "append
 * only").
 */
function isAppendOnlyAtTail(
  previous: LiveDocumentVM | null,
  nextOrder: string[],
  changedNodeIds: string[],
): boolean {
  if (!hadPriorRenderedContent(previous)) return false;
  const prevOrder = flatNodeOrder(previous);
  if (nextOrder.length <= prevOrder.length) return false;
  for (let i = 0; i < prevOrder.length; i++) {
    if (nextOrder[i] !== prevOrder[i]) return false;
  }
  const changed = new Set(changedNodeIds);
  for (let i = 0; i < prevOrder.length; i++) {
    if (changed.has(prevOrder[i])) return false;
  }
  return true;
}

/**
 * Fold the current `MaterializedNotes` snapshot against the PREVIOUS fold's
 * output — never against raw patches/`sessionProjectionEvents` — so this is
 * O(current note count), not O(patches × ops). Pass `null` for `previous`
 * on the first fold of a session (or after a session switch); every
 * subsequent call should pass this function's own last return value.
 *
 * DISCLOSED SCOPE NOTE: within that O(current note count) walk, every
 * node's `contentHash` IS recomputed every fold (`foldNode` calls
 * `contentHashOf` unconditionally), even for nodes a given patch never
 * touched — the ticket's "do not rehash the whole document per patch"
 * clause is honored only in the O(patches × ops) sense above, not
 * literally. A `prior.lead === lead && prior.text === text` short-circuit
 * was tried and reverted: it would skip the djb2 call for unchanged nodes,
 * but it also routes around `contentHashOf` entirely on exactly the
 * tags-only-churn path the "contentHash excludes tags" mutation test
 * exists to guard — silently defeating that guarantee. At field scale
 * (~100 short notes) this is a fixed, single-pass djb2 walk over short
 * strings per fold — not a hot path — so the honest fix is this
 * disclosure, not a shortcut that trades away test sensitivity for an
 * unmeasured, likely-negligible win.
 */
export function notesToOutline(
  previous: LiveDocumentVM | null,
  notes: LiveDocumentSource | null,
): LiveDocumentVM {
  if (!notes || notes.notes.length === 0) {
    return { ...EMPTY_VM, lastSequence: notes?.last_sequence ?? 0 };
  }

  const previousById = indexPreviousNodes(previous);
  const hasPreviousFold = hadPriorRenderedContent(previous);
  const changedNodeIds: string[] = [];

  const sections: DocSection[] = [
    { id: DOC_UNSECTIONED, heading: null, headingLevel: null, nodes: [] },
  ];

  for (const note of notes.notes) {
    const headingLevel =
      note.heading_level != null ? clampHeadingLevel(note.heading_level) : null;
    if (headingLevel !== null) {
      const section: DocSection = {
        id: note.id,
        heading: note.title,
        headingLevel,
        nodes: [],
      };
      sections.push(section);
      appendNoteNodes(
        section,
        note,
        null,
        previousById,
        hasPreviousFold,
        changedNodeIds,
      );
    } else {
      const section = sections[sections.length - 1];
      appendNoteNodes(
        section,
        note,
        note.title,
        previousById,
        hasPreviousFold,
        changedNodeIds,
      );
    }
  }

  // Drop the synthetic leading bucket when nothing landed in it (e.g. the
  // very first note already carries a heading) — never render an empty,
  // unlabeled section. A real heading section is kept even with zero body
  // nodes: the heading itself is real content.
  const finalSections = sections.filter(
    (section) => section.heading !== null || section.nodes.length > 0,
  );

  const nextOrder = finalSections.flatMap((section) =>
    section.nodes.map((node) => node.id),
  );

  return {
    sections: finalSections,
    lastSequence: notes.last_sequence,
    changedNodeIds,
    appendedAtTail: isAppendOnlyAtTail(previous, nextOrder, changedNodeIds),
  };
}

/**
 * Serialize the outline to markdown for the copy/export path ONLY — never
 * used for rendering (design-a §1.1: markdown is the serialization, not the
 * runtime). Headings become `#`-runs at `headingLevel` (default 3 — kept
 * for parity with a `heading === null` guard that should be unreachable);
 * bullets nest by `depth`; a bold run-in renders as `**lead** text`.
 */
export function outlineToMarkdown(vm: LiveDocumentVM): string {
  const lines: string[] = [];
  for (const section of vm.sections) {
    if (section.heading !== null) {
      const level = section.headingLevel ?? 3;
      lines.push(`${"#".repeat(level)} ${section.heading}`);
      if (section.nodes.length > 0) lines.push("");
    }
    for (const node of section.nodes) {
      const indent = "  ".repeat(node.depth);
      const bulletText = node.lead
        ? `**${node.lead}** ${node.text}`
        : node.text;
      lines.push(`${indent}- ${bulletText}`);
    }
    lines.push("");
  }
  // Collapse the trailing run of blank lines from the per-section
  // separators above into exactly one trailing newline.
  return `${lines.join("\n").replace(/\n+$/, "")}\n`;
}
