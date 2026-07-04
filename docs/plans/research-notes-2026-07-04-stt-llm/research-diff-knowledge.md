---
title: Diff-based, patch-not-append maintenance of an evolving document + knowledge graph
date: 2026-07-04
tags: [llm-diff-editing, temporal-kg, belief-revision, crdt, provenance, agent-memory]
status: draft
---

# Diff-Based, Patch-Not-Append Maintenance of an Evolving Note-and-Graph

## Framing

The task splits cleanly into two coupled substrates and one decision policy:

1. **The prose note** (an evolving document). New info should PATCH it in place, not append. → LLM structured-edit formats (search/replace, JSON-patch, unified diff), plus a concurrent-edit substrate (CRDT/OT) if multiple writers touch it.
2. **The knowledge graph** (facts/edges). New info should RETROACTIVELY UPDATE prior facts, and when a fact genuinely supersedes another, that supersession must be recorded and acknowledged, not silently overwritten. → bi-temporal knowledge graphs, RDF provenance, belief revision / truth maintenance.
3. **The decision**: patch-in-place vs. record-a-supersession — a contradiction-detection + confidence problem, with adversarial failure modes on both sides (silent overwrite of correct info; hallucinated retraction).

The load-bearing insight across every source: **do not delete or clobber; invalidate-and-supersede while preserving history, and address edits by CONTENT not by position.**

---

## 1. LLM-driven document patching — emit a minimal edit, not a regeneration

### 1a. The format taxonomy and what actually works

Aider defines the canonical edit-format menu, and the empirical ranking has been reproduced repeatedly:

- **whole / full-rewrite** — model re-emits the entire file. Always applies; cost scales linearly with file length. Cheapest option only for very short targets (< ~300 tokens of content the anchor overhead would otherwise dominate) [edit-formats-aider; agentpatterns; arxiv:2604.27296 §5].
- **diff (SEARCH/REPLACE block)** — model emits exact `old_string` / `new_string`; harness substitutes. Used by Aider, Claude Code's `FileEditTool`, and Anthropic's `str_replace_based_edit_tool`. This is the practical default for frontier API models above ~300 tokens [edit-formats-aider; agentpatterns; tsukino chapter-10].
- **udiff (unified diff)** — model emits `@@ hunks @@`. Aider *strips the line numbers* and interprets each hunk as a fuzzy search/replace, because **"GPT is terrible at working with source code line numbers … backed up by many quantitative benchmark experiments"** [unified-diffs-aider]. Line-numbered unified diff is "for patch tools and humans, not LLMs" [agentpatterns].
- **AST / structure-aware diff (FuncDiff/BlockDiff)** — anchor an edit to a named AST node, not a position. Most reliable in a 2026 benchmark (three models scored 100%, zero format failures) but requires tree-sitter at apply-time and ideally a fine-tuned model [geometric-ast-edits; arxiv:2604.27296 §3].

### 1b. Why line numbers and JSON-wrapped diffs are traps

Two independently-verified failure classes:

- **Positional anchors drift.** LLMs cannot reliably produce or track line numbers; any single-character deviation in a strict unified-diff hunk header (`@@ -old,+new @@`, prefix chars, context counts) discards the whole patch [unified-diffs-aider; tsukino chapter-10]. The fix is *content addressing*: unique surrounding text (search/replace) or an AST node name (FuncDiff). AgentPatterns states the mechanism directly: "replace positional anchors (line numbers) with content anchors (unique strings or AST blocks) so the model emits coherent code" [agentpatterns].
- **JSON-wrapping the edit is brittle.** Aider found "when it's unpacked from JSON, or the JSON decode just fails entirely" — escaping source inside a JSON string breaks; the raw fenced diff is more robust [unified-diffs-aider]. But see §1c — for *structured* targets (the KG), JSON-patch wins.

### 1c. Search/replace's own failure: delimiter collision and non-unique anchors — and the KG-patching counterpoint

Search/replace is not free of hazards:

- **Delimiter collision.** "To Diff or Not to Diff" (arxiv:2604.27296) rejects search/replace for its benchmark precisely because "the search/replace style can fail to patch if the source code contains characters identical to its special delimiters … a direct violation of the reliability requirements"; they adopt a hunk-rewrite content-addressed style instead.
- **Non-unique anchor.** Anthropic's editor tool *requires the match to be unique* — multiple matches error and force the model to expand context [agentpatterns; Claude text-editor docs]. Graphiti/arxiv:2604.27296 handle this by "progressively expanding the anchor content until it [is unique]."
- **Hallucination-resistance is the upside.** If the model "remembers" `handleError()` but the file was refactored to `processError()`, search/replace fails *loudly* (error: "String to replace not found"), forcing a re-read — whereas full-rewrite would silently emit a file containing the stale `handleError()`, overwriting the correct code with no error at all [tsukino chapter-10]. **This is the single most important argument for patch-not-append: a failed patch is a visible signal; a bad regeneration is a silent corruption.**

**For a structured JSON KG specifically, the calculus flips toward JSON-Patch (RFC 6902).** The `json-correction-loop` result is stark: when an LLM full-regenerates a 100-entity / 183-edge JSON knowledge graph on critic feedback, `gpt-4o-mini` fixes **0/8** flagged defects and burns 73K tokens; a surgical RFC-6902 patch loop fixes **8/8 at ~5× fewer tokens** [github/warpspaceinc/json-correction-loop]. Two design lessons from that project: (i) restrict the patch vocabulary — they implement only `add`/`replace`/`remove` and *reject* `move`/`copy` by design; (ii) naive patching alone only fixes 35–64% because the LLM patches the *symptom* (edge with wrong predicate) not the *root cause* (entity whose type flipped) — a `path_finder` sub-agent that redirects symptom→root-cause pointers is what closes the gap to 100%.

**Recommendation for this system:** prose note → SEARCH/REPLACE blocks (or AST/section-anchored if the note has stable headings); structured KG → RFC-6902 JSON-Patch with a restricted op set and a root-cause pointer step. Never full-regenerate the note as the update primitive — a failed patch is recoverable; a hallucinated regeneration is silent data loss.

---

## 2. Temporal / bi-temporal knowledge graphs — the supersede-don't-delete substrate

This is the most directly applicable body of work, and Graphiti/Zep is the canonical implementation.

### 2a. Bi-temporal model: four timestamps, two timelines

Graphiti (the engine inside Zep) implements a **bi-temporal model** [zep-arxiv:2501.13956 §2.2.3]. Every fact-edge (`EntityEdge`) carries four timestamps split across two timelines:

- **Valid time (T) — when the fact was true in the real world:**
  - `valid_at` — when the fact became true.
  - `invalid_at` — when the fact stopped being true.
- **Transaction time (T′) — when the system knew it:**
  - `created_at` — when the edge was written to the DB (always present).
  - `expired_at` — when the edge was superseded/invalidated in the DB (nullable).

[getzep-graphiti temporal-model; graphiti concepts/knowledge-graphs; blog.getzep beyond-static-knowledge-graphs]. Valid-time comes from an LLM extraction prompt over the episode (handles absolute *and* relative dates like "two weeks ago", resolved against a `reference_time`); transaction-time comes from ingestion wall-clock [blog.getzep].

This split is exactly the valid-time vs. transaction-time distinction from the temporal-database literature (Snodgrass et al.: "On the semantics of 'now'"; "Supporting valid-time indeterminacy" [openalex]). It lets the system answer two different "when" questions and, critically, handle **late-arriving / out-of-order facts**: a fact learned today (`created_at`=now) can be marked as having been true since 2022 (`valid_at`=2022) [getzep-graphiti temporal-model].

### 2b. Supersede-not-delete: edge invalidation on contradiction

The core mechanism, verified at code level:

> "When new information contradicts existing knowledge, Graphiti **invalidates old edges without deleting them** … preserving the complete history of what the system knew and when." [getzep-graphiti temporal-model]

The pipeline on each new episode [zep-arxiv §2.2.3; blog.getzep; graphiti_core/utils/maintenance/edge_operations.py:484]:
1. Extract entities + candidate edges from the episode.
2. For each new edge, retrieve **semantically similar existing edges** and run an **LLM invalidation prompt** giving those existing edges as context.
3. On a detected temporally-overlapping contradiction:
   - Old edge: set `invalid_at` ← new fact's `valid_at`, and `expired_at` ← now.
   - Create a NEW edge with the updated fact (`invalid_at`=None, `expired_at`=None).
   - **Regenerate the fact string on the invalidated edge to reflect updated knowledge** [blog.getzep].
4. To decide *which* of two conflicting edges is the one to expire, sort by `valid_at` and **invalidate the one that occurred earlier in the real world** — e.g., `MARRIED_TO` vs. `DIVORCED_FROM`, or Sarah "VP at TechCo (valid 2022)" superseded by "CEO at Acme (valid 2024)": old edge → `invalid_at=2024, expired_at=now`; new edge → open [getzep-graphiti temporal-model; blog.getzep].

The paper is explicit that the policy is **"consistently prioritizes new information when determining edge invalidation"** and that the graph "dynamically updates … in a **non-lossy manner**" [zep-arxiv]. Zep outperforms MemGPT on DMR (94.8% vs 93.4%) and gains up to 18.5% on LongMemEval's temporal-reasoning tasks with 90% lower latency — evidence the mechanism helps real cross-session synthesis, not just theory [zep-arxiv abstract].

### 2c. The explicit supersession edge — `invalidated_by`

The OpenAI cookbook "Temporal Agents with Knowledge Graphs" refines Graphiti and adds an **explicit provenance link between the new fact and the fact it retires**: the Invalidation Agent will "Detect contradictions and mark outdated entries with `t_invalid`" and **"Link newer statements to those they invalidate with `invalidated_by`"** [openai-cookbook temporal_agents §3.1.2]. This is the "supersession must be acknowledged" requirement realized as a graph edge, not just a nulled timestamp — you can traverse *why* a fact was retired and *what* replaced it. The cookbook also adds **statement typing** (STATIC / DYNAMIC / TEMPORAL_EVENT / OPINION), so invalidation only fires where it makes sense: static facts ("X was born in Y") never expire; only dynamic/temporal statements are candidates for supersession — a cheap, high-value filter against over-eager invalidation.

### 2d. Provenance to source (episodes) and RDF alternatives

Graphiti keeps an **episode subgraph** as the raw ground-truth stream; every derived entity/fact traces back to the episode(s) that produced it [zep-arxiv §2; graphiti README]. This is the KG analogue of git blame.

For an RDF-native design, the statement-level provenance options (survey: arxiv:2305.08477) are, in ascending desirability:
- **RDF reification / n-ary relations** — the only W3C-standard route, but "considerable design flaws," verbose, and referential-transparency issues.
- **Named graphs** — put each version/source in its own graph; a "blame view" is a union of versioned named graphs [w3c/rdf-ucr wiki].
- **RDF-star (RDF 1.2)** — embed a triple as the subject/object of another (`<< :Sarah :roleAt :TechCo >> :validFrom 2022 ; :retractedIn <v2>`), far less verbose than reification, SPARQL-star for querying; already used in Wikidata-style provenance and YAGO4 [arxiv:2305.08477; metaphacts RDF-star blog; w3c/rdf-ucr]. The `:retractedIn` / `:statedIn` pattern is the RDF equivalent of Graphiti's `expired_at` + `invalidated_by`.

---

## 3. Belief revision & truth maintenance — the theory of retract-and-supersede

### 3a. AGM belief revision — the ideal, and why "minimal change" matters

Classical **AGM** (Alchourrón, Gärdenfors, Makinson 1985) frames an agent maintaining logically consistent beliefs under new information, via three operations: **expansion** (add a belief with no conflict), **revision** (add a belief that contradicts current beliefs — must remove enough old beliefs to stay consistent), **contraction** (give up a belief) [fundamental-problems-model-editing §3; arxiv:2112.13557 "AGM Belief Revision, Semantically"]. The governing principle is **minimal change / informational economy**: when you must retract to accommodate a contradiction, retract as *little* as possible. Katsuno–Mendelzon separate **revision** (world is static, my info improves) from **update** (world itself changed) — directly mapping onto our patch-vs-supersede distinction: revision ≈ *patch-in-place* (I was wrong before); update ≈ *record-supersession* (the fact genuinely changed) [arxiv:2104.14512; arxiv:2602.23302 "KM update contained in AGM revision"].

**Direct LLM tie-in:** "Fundamental Problems With Model Editing" (arxiv:2406.19354) argues model editing *inherits* AGM's problems: the goal of "maintaining logically consistent beliefs when updated" is the belief-revision goal, so editing inherits the **ripple/entailment problem** — updating one fact should update its logically-entailed facts (learn an animal is a vertebrate → update credence it's venomous), but "creating entailment data … is sometimes nearly impossible, since we do not know what facts are entailed." It also names the **"coherence at all cost"** problem: maintaining full logical closure is unboundedly expensive, so a practical system must bound how much consistency-repair it does per update.

### 3b. Truth Maintenance Systems — the engineering realization

TMS/JTMS/ATMS are the operational answer to "how do I retract a belief and everything that depended on it":

- **JTMS (Doyle)** — maintains a **dependency network**: nodes = beliefs, plus justification nodes recording *why* each belief is held (its in-list / out-list). New info can force **retraction of previously-derived conclusions** — this is nonmonotonic reasoning — and the network is **relabeled** to propagate the retraction [kr.tuwien rr1702; temple cis587 tms]. The key artifact: a belief carries its **justification**, so when a premise is withdrawn, the system knows exactly which downstream conclusions to withdraw.
- **Dependency-directed backtracking** — "the justification of a sentence … provides the natural indication of what assumptions need to be changed if we want to invalidate that sentence" [temple tms]. A **contradiction node** firing triggers targeted backtracking to the assumption(s) responsible — not a blind re-derivation [dekleer back-to-backtracking].
- **ATMS (de Kleer)** — instead of one consistent context, maintain *all* environments simultaneously; each belief is labeled with the set of assumption-sets that support it. You never retract/relabel — you switch environments. Cost: exponential label blow-up [dekleer ATMS foundations; temple tms].

**Design lesson for the note/KG:** store the **justification/provenance** of every derived fact (which episode/source, which upstream facts). This is what makes a *principled* retraction possible — you can invalidate a fact *and its dependents* rather than leaving orphaned conclusions. Graphiti's episode-provenance + `invalidated_by` is a lightweight JTMS: the episode is the justification, `invalidated_by` is the retraction edge. The gap: Graphiti does not propagate entailment (§3a ripple problem) — it invalidates the directly-contradicted edge, not facts logically downstream of it.

---

## 4. CRDT / OT — only if concurrent writers touch the same note-state

Relevant only if multiple agents/humans edit the *same* note concurrently; for a single-writer pipeline it is over-engineering.

- **OT (Operational Transformation, ~late 1980s)** — transforms concurrent operations against each other to preserve **convergence, causality preservation, and intention preservation** [Sun et al. TOCHI 1998, doi:10.1145/274444.274447]. Still powers the majority of production co-editors (Google Docs lineage).
- **CRDT (Conflict-free Replicated Data Types, ~2006, orig. WOOT)** — operations are designed to natively commute; merge via a join-semilattice (state-based) or causal-delivery of ops (op-based) [ACM Comput. Surv. doi:10.1145/3695249]. Text CRDTs (RGA, Treedoc, and modern **Eg-walker**, arxiv:2409.14252) are the list-editing case. **Tombstones** mark deletes without physically removing (so concurrent ops still reference them) — conceptually identical to bi-temporal `expired_at`: don't delete, mark dead.
- **The debate:** Sun et al. (doi:10.1145/3392825) argue empirically that despite CRDT's "superiority" claims, OT remains the real-world choice and CRDTs carry hidden complexity/algorithmic flaws for co-editing. For this system the practical read: if you need concurrent note editing, OT is battle-tested; if you need offline/P2P merge without a server, CRDT. The KG-fact layer should NOT be modeled as a text CRDT — model it as the bi-temporal graph (§2), where "concurrent contradictory facts" is a *semantic* conflict resolved by valid-time ordering, not a *syntactic* merge.

---

## 5. Deciding patch-in-place vs. record-a-supersession

Synthesizing across the sources, the decision reduces to **contradiction classification + confidence + fact-type**:

**Patch-in-place (revision — "I was wrong / imprecise before")** when:
- The new info *corrects* or *refines* the same fact without the world having changed (a typo, a more precise value, a clarification). AGM: this is revision under a static world / KM revision [arxiv:2104.14512].
- On the note: a SEARCH/REPLACE that tightens wording. On the KG: an RFC-6902 `replace` on the same edge (Graphiti even does this — it *regenerates the fact string* on an edge). No new edge, no supersession record needed.

**Record-a-supersession (update — "the fact genuinely changed")** when:
- The new info contradicts the old AND both were true at *different* real-world times (Sarah was VP, now CEO). AGM/KM: update, not revision. Graphiti: temporally-overlapping contradiction → invalidate old edge (`invalid_at`, `expired_at`) + new edge + `invalidated_by` link [getzep-graphiti; openai-cookbook].
- The rule of thumb from Graphiti: **when two edges conflict, invalidate the one with the earlier `valid_at`** and keep the later as current [blog.getzep].

**How to detect the contradiction (the hard part):**
- Graphiti/OpenAI-cookbook: an **LLM invalidation prompt** compares the candidate fact against *semantically-similar existing edges only* (retrieval-scoped, not the whole graph — bounds cost), with **bidirectionality checks** and **episodic-type constraints** to cut spurious comparisons [openai-cookbook §3.1.2].
- Fact typing as a gate: STATIC facts are never invalidated; only DYNAMIC/TEMPORAL_EVENT statements are supersession candidates [openai-cookbook]. This is a cheap, high-precision filter against over-eager retraction.
- Confidence: the "Knowledge Conflicts for LLMs" survey (arxiv:2403.08319) classifies conflicts as **context-memory, inter-context, and intra-memory** — for our case the relevant one is inter-context (new source vs. stored fact). It notes solutions weight sources by recency/reliability; the safe default is Graphiti's "prioritize new information" but *only after* the contradiction is confidently detected.

---

## 6. Adversarial / failure modes (the reason patch-not-append is safety-critical)

**Silent overwrite of correct information:**
- Full-file regeneration is the primary vector: the model reconstructs from a stale memory and silently clobbers correct current content with no error signal [tsukino chapter-10]. Patch formats (search/replace, JSON-patch) fail *loudly* on a bad anchor, which is why they're safer.
- In parametric model editing, the **ripple effect** silently corrupts logically-related facts: editing one fact damages the model's memory of entailed facts, "Ripple Effect in Hidden Space" being especially "elusive … difficult to detect" and compounding with edit count [arxiv:2403.07825; aclanthology 2024.emnlp-main.700 GradSim]. Analogue for a KG: invalidating an edge without invalidating its dependents leaves inconsistent orphans (the §3a entailment gap).
- **Knowledge distortion / Round-Edit**: editing a fact then trying to revert it does NOT restore the original — "irreversible damage," "may even enhance hallucination" [arxiv:2310.02129 ConflictEdit/RoundEdit]. Lesson: retractions are not free undo; keep the *original* fact addressable (bi-temporal history), don't rely on re-editing to reverse a mistake.

**Hallucinated retraction / over-eager invalidation:**
- **Over-ripple**: after an edit the model over-generalizes, answering the edited target even to unrelated related queries [aclanthology 2024.emnlp-main.700]. KG analogue: an aggressive invalidation prompt marks *correct* facts as superseded.
- Mitigations converge on: scope contradiction checks to semantically-similar edges only, gate by fact-type (STATIC never expires), require temporal overlap before invalidating, and keep `invalidated_by` provenance so a bad retraction is auditable and reversible [openai-cookbook; getzep-graphiti].

**Cumulative corruption of evolving memory (the systemic risk):**
The SSGM paper (arxiv:2603.11768) is the key adversarial source. Unlike static RAG where "errors are isolated to a single retrieval step, errors in evolving memory systems are **cumulative and persistent**." It gives a four-dimensional failure taxonomy across three interfaces:
1. **Ingestion → memory poisoning** (adversarial injection stored as truth).
2. **Consolidation → semantic drift** (facts distorted through repeated summarization) and **procedural drift**.
3. **Retrieval → memory hallucination** (hallucinated content stored as truth) and **temporal obsolescence** (fact is correct but stale — the exact failure Graphiti's `invalid_at` addresses).
SSGM's prescription: **decouple memory evolution from execution** and enforce **consistency verification + ground-truth anchoring + temporal decay** *before* any consolidation write — i.e., don't let the agent be both the sole generator and validator of its own memory. This is the strongest architectural recommendation for this system: a **validation gate** (contradiction check + confidence + type gate) between "new info arrives" and "note/graph mutated," with all mutations going through patch/supersede ops that preserve history rather than in-place clobbers.

---

## Concrete recommendation for the note-and-graph system

1. **Note (prose):** update via SEARCH/REPLACE blocks (content-anchored; expand anchor until unique) or section/AST-anchored edits. Never full-regenerate as the update primitive. A failed patch = a signal to re-read, not silent loss.
2. **Graph (facts):** bi-temporal edges with `valid_at`/`invalid_at` (world) + `created_at`/`expired_at` (system). Update structured state via RFC-6902 `add`/`replace`/`remove` (reject `move`/`copy`); route symptom→root-cause before patching.
3. **Supersession:** on a confidently-detected, temporally-overlapping contradiction of a DYNAMIC/TEMPORAL fact, invalidate-don't-delete (set `invalid_at`+`expired_at`), create the new edge, and link `invalidated_by` (or RDF-star `:retractedIn`) so the supersession is acknowledged and auditable. Sort by `valid_at` to pick which edge to retire.
4. **Patch-vs-supersede gate:** correction of a static/imprecise fact → patch in place (revision); genuine world-change of a dynamic fact → record supersession (update). Gate by fact-type; scope contradiction checks to semantically-similar entries only.
5. **Provenance = justification:** every derived fact traces to its source episode (JTMS-style). This is what makes principled retraction (and reversal of a bad retraction) possible.
6. **Validation gate before every write** (SSGM): the agent must not be sole generator+validator; interpose a consistency/confidence/type check so cumulative drift and hallucinated retractions are caught before they persist.

---

## Sources (vault note-ids and URLs)

- Zep temporal KG paper — vault `zep-a-temporal-knowledge-graph-architecture-for-agent-memory` (arxiv:2501.13956)
- Graphiti bi-temporal model docs — vault `bi-temporal-data-model-graphiti` (getzep-graphiti.mintlify.app/concepts/temporal-model)
- Graphiti README — vault `readmemd-2` (github getzep/graphiti)
- Zep engineering blog on evolving edges — vault `beyond-static-graphs-engineering-evolving-relationships` (blog.getzep.com/beyond-static-knowledge-graphs)
- OpenAI cookbook Temporal Agents with KGs — vault `temporal-agents-with-knowledge-graphs` (developers.openai.com/cookbook … temporal_agents) — `invalidated_by`, statement typing
- Aider edit formats — vault `edit-formats-aider`; unified-diffs blog — vault `unified-diffs-make-gpt-4-turbo-3x-less-lazy-aider`
- To Diff or Not to Diff — vault `260427296-to-diff-or-not-to-diff-...` (arxiv:2604.27296) — delimiter-collision risk, hunk-rewrite
- AgentPatterns edit-format selection — vault `edit-format-selection-diff-vs-search-replace-vs-full-rewrite-agentpatternsai`
- Geometric AST-edits benchmark — vault `ast-edits-the-code-editing-format-nobody-uses-geometric`
- json-correction-loop (RFC-6902 KG patching) — github.com/warpspaceinc/json-correction-loop
- Claude Code editing strategy chapter — notes.tsukino.dev … 05-code-editing-strategy (hallucination-safety of search/replace)
- Fundamental Problems With Model Editing — vault `fundamental-problems-with-model-editing-...` (arxiv:2406.19354) — AGM inheritance, ripple/entailment, coherence-at-all-cost
- Knowledge Conflicts for LLMs survey — vault `240308319-knowledge-conflicts-for-llms-a-survey` (arxiv:2403.08319)
- SSGM governing evolving memory — vault `governing-evolving-memory-in-llm-agents-...` (arxiv:2603.11768) — 4-dim failure taxonomy, validation gate
- TMS/JTMS/ATMS — temple cis587/tms; kr.tuwien rr1702 (JTMS review); de Kleer ATMS foundations & back-to-backtracking (dekleer.org)
- AGM/KM — arxiv:2112.13557, 2104.14512, 2602.23302, 2307.05629 (contraction)
- Ripple/distortion failures — arxiv:2403.07825 (ripple mitigation SIR), aclanthology 2024.emnlp-main.700 (GradSim), arxiv:2310.02129 (ConflictEdit/RoundEdit), aclanthology 2024.findings-emnlp.550 (pitfalls survey), 2024.findings-acl.902 (catastrophic forgetting)
- CRDT/OT — Sun et al. TOCHI 1998 (doi:10.1145/274444.274447) & doi:10.1145/3392825 (OT-vs-CRDT critique); ACM CSUR doi:10.1145/3695249 (CRDT survey); Eg-walker arxiv:2409.14252
- RDF provenance — arxiv:2305.08477 (survey: reification/named-graphs/RDF-star/PROV-O); metaphacts RDF-star blog; w3c/rdf-ucr wiki (blame view, `:retractedIn`)
- Temporal DB foundations — Snodgrass "semantics of now" (doi:10.1145/249978.249980), "valid-time indeterminacy" (doi:10.1145/288086.288087)
