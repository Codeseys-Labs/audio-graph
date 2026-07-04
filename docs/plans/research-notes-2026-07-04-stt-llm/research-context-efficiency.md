# Making a Live STT→LLM Notes Loop Contextually Efficient

**Research question:** How do you keep a live speech-to-text → LLM notes loop contextually efficient — i.e. NOT re-sending every transcribed word to the model on every tick — while preserving quality?

**Date:** 2026-07-04
**Scope:** (1) incremental / rolling context strategies, (2) prompt & session caching mechanics (Anthropic / OpenAI / Google), (3) delta / event-driven LLM invocation, (4) cost/quality tradeoffs and failure modes.

---

## TL;DR — the recommended architecture

A live STT→LLM notes loop should combine four independent levers. They are complementary, not alternatives:

1. **Tiered memory (hot + warm), incrementally maintained.** Keep the last N raw transcript turns verbatim (the "hot buffer", typically 5–20 turns), plus a *rolling* running summary of everything older (the "warm region"). Update the warm summary by folding in ONLY the turn(s) about to fall out of the hot buffer — never re-summarize the whole history from scratch. This bounds per-turn input to ~constant size regardless of meeting length.
2. **Structured / typed state alongside the summary.** Pin must-never-lose facts (names, IDs, numbers, decisions, rejected options + *why*) in an application-managed JSON/state layer injected at the top of each prompt — do NOT trust the prose summarizer to retain them. This is the single most-cited mitigation for summary drift.
3. **Prompt caching with a stable prefix.** Order the prompt static→dynamic (system + tools + pinned state first, then the append-only transcript, then the varying instruction). Cache the stable prefix so re-sent tokens bill at ~10% (Anthropic/Gemini) to 50% (OpenAI gpt-4o) of input price. Append-only keeps cache hits alive.
4. **Event/delta-driven invocation.** Only call the summarizing/notes LLM on meaningful boundaries — end-of-turn, topic shift, or a debounced settle timer — not on every partial ASR token. Cheap local signals (VAD end-of-turn, embedding/Jaccard topic-change detection) gate the expensive LLM call.

Do NOT re-send every word: re-sending the full transcript every tick is O(N²) in tokens over a session and is the failure teams hit first ("Request contains too many tokens" once one power-user session runs long) [gemilab].

---

## 1. Incremental / rolling context strategies

### The two-tier "hot buffer + warm summary" pattern (dominant production pattern)

Multiple independent 2026 practitioner sources converge on the SAME structure:

- **Hot region / hot buffer:** the last N raw conversation turns, kept verbatim — "typically 10–20 turns" [tianpan-gradual]; "last 5–10 turns stored verbatim … to preserve the immediate vibe and prosody" [chainofcraft].
- **Warm region:** "a persistent, structured summary of everything older than the hot region" [tianpan-gradual]. It is "the only part that is allowed to be lossy. Its only job is to provide high-level continuity" [chainofcraft].
- **Crucially, neither region ever goes away entirely — you're compressing, not deleting** [tianpan-gradual].

### Rolling / incremental update (NOT full re-summarization)

The load-bearing detail: when the hot region overflows, **do not re-summarize everything from scratch.** "You identify the turn that's about to fall off the edge of the hot region, summarize only that turn (or a small batch), and merge that new mini-summary into the persistent warm summary. The persistent summary grows incrementally rather than being replaced wholesale" [tianpan-gradual]. "Use incremental 'rolling' summaries. Instead of re-summarizing the entire history every time, update your existing summary with only the newest chunk. This reduces compute costs and prevents 'recursive hallucination' where the summary of a summary becomes fiction" [chainofcraft].

This is the "anchored iterative approach — where the summary is updated, not rebuilt — [that] avoids the compounding errors of full-reconstruction" [tianpan-gradual].

A concrete cadence example from a shipped system: compression runs "every fourth turn past the sixth"; they tried `> 10 && % 5 === 0` first, then tuned smaller thresholds down because "the smaller numbers reduced 'you said earlier' failures." They also keep a `key_qa_pairs` field of recent verbatim Q&A pairs alongside the rolling summary [sourceshift]. The Gemini practitioner reports that with rolling summaries "the history typically settles around 1,500–2,500 tokens regardless of how many turns have happened. That stability is the entire point: per-turn input cost stops growing" [gemilab].

### Academic grounding

- **Recursively Summarizing Enables Long-Term Dialogue Memory in LLMs** (Wang et al., arXiv 2308.15022, 2023, ~78 cites) — proposes recursively generating/updating summaries as long-term dialogue memory so chatbots recall past info and stay consistent; the canonical academic statement of the rolling-summary idea. [arxiv-2308.15022]
- **MemGPT: Towards LLMs as Operating Systems** (Packer et al., arXiv 2310.08560, 2023) — "virtual context management" inspired by OS hierarchical memory: a small in-context "main memory" plus paged "external memory," with the LLM issuing function calls to move data in/out. Frames the hot/warm tiering as an OS memory-hierarchy problem. [arxiv-2310.08560]
- **MemoryBank** (Zhong et al., arXiv 2305.10250, 2023) — long-term memory with an Ebbinghaus-forgetting-curve update mechanism; canonical for *selective* retention over time. [arxiv-2305.10250]
- **Generative Agents** (Park et al., arXiv 2304.03442, 2023, ~4654 cites) — the "memory stream" + reflection + retrieval (recency/importance/relevance scoring) architecture; the most-cited memory-architecture paper, source of the retrieve-by-score idea. [arxiv-2304.03442]
- **ReadAgent / gist memory** (Lee et al., arXiv 2402.09727) — human-inspired "gist memory": store compressed gists, page back to raw episodes on demand — increases effective context up to 20×. Supports the pattern of keeping raw transcript retrievable rather than discarded. [arxiv-2402.09727]

### When to send raw delta vs a running summary

Practitioner consensus (decision rule):
- **Send raw** for the hot buffer (recent turns) — recency-sensitive reasoning, prosody, "you just said." Cheap because bounded and cache-friendly (append-only).
- **Send summary** for older material where high-level continuity is enough.
- **Send structured/typed facts** (NOT summary) for anything whose loss breaks the app: names, numbers, decisions, action items, rejected alternatives + reasons. See §4.
- **Retrieve on demand** (semantic search over stored raw turns) when a specific old detail is needed and cross-session lookup matters — instead of stuffing everything in context [sourceshift; chainofcraft].

### Semantic chunking / topic segmentation of the transcript

For the notes/RAG layer (as opposed to the live loop), fixed-token chunking is the "most common mistake … a 512-token boundary will land mid-sentence, mid-topic. Always segment by topic or speaker turn" [charleschen-rag]. Topic segmentation options, cheapest→best:
- **Embedding similarity (DeepTiling):** rolling-window embeddings, cosine-similarity depth curve to find boundaries — *zero LLM calls* for segmentation. One tool does a 5-min transcript in ~30s using `nomic-embed-text` for boundaries + one LLM call per topic [erikbahena; charleschen-rag].
- **LLM segmentation:** better for subtle shifts ("Speaking of which…") but ~$0.01/page [charleschen-rag].
- **Zero-shot NLI topic classification per speaker turn** (e.g. `facebook/bart-large-mnli`, ~50 segments/s on GPU) fired on each `end_of_turn` webhook [meetstream].
Target 300–800 tokens per topic chunk; split at speaker transitions, not arbitrary token counts [charleschen-rag].

---

## 2. Prompt / session caching mechanics

The core mechanic across ALL vendors: **the cache matches the longest common PREFIX; anything after the first changed token is not a hit.** Therefore order the prompt **static content first, variable content last**, and grow the transcript **append-only** so the cached prefix stays intact.

### Anthropic prompt caching (most explicit control) [anthropic-caching]

- **`cache_control` breakpoints:** up to **4** explicit breakpoints per request; "define up to 4 cache breakpoints if you want to cache different sections that change at different frequencies." Each breakpoint writes one cache entry = a cumulative hash of the prefix ending at that block.
- **Minimum cacheable length (tokens):** Opus 4.8 / Sonnet 5 / Sonnet 4.6 = **1,024**; Haiku 3.5 = **2,048**; Haiku 4.5 = **4,096**; Fable 5 / Mythos 5 = **512**. "Shorter prompts cannot be cached, even if marked … no error is returned."
- **TTL:** default **5 minutes**, refreshed at no cost on reuse. Optional **1-hour** TTL via `"cache_control": {"type":"ephemeral","ttl":"1h"}`.
- **Pricing multipliers (vs base input):** 5-min cache **write = 1.25×**; 1-hour cache **write = 2×**; cache **read = 0.1×** (i.e. cached tokens re-read at 10% of input price). Example on Opus 4.8 ($5/MTok input): 5m write $6.25, 1h write $10, read **$0.50**/MTok.
- **Longest-prefix match + lookback:** on each request the system hashes the prefix at your breakpoint; if no match it "walks backward one block at a time" looking for prior writes — lookback window **20 blocks**.
- **Invalidation hierarchy:** changing **tools** invalidates tools+system+messages; changing **system** invalidates system+messages; changing **messages** invalidates only the messages cache. So keep tools/system byte-stable.
- **Ordering rule (directly on-point for live transcripts):** *"Place the breakpoint on the last block that stays identical across requests. For a prompt with a static prefix and a varying suffix (timestamps, per-request context, the incoming message), that is the end of the prefix, not the varying block."* The documented anti-pattern: putting the breakpoint on the block that carries a per-request timestamp — it changes every request and never hits. Put the timestamp/varying text AFTER the breakpoint.
- **Incremental / automatic caching for growing conversations:** *"With automatic caching, the cache point moves forward automatically as conversations grow. Each new request caches everything up to the last cacheable block, and previous content is read from cache."* Documented progression: req1 caches System+…+User(2); req2 reads System→User(2) from cache and writes the new Asst(2)+User(3); etc. Works as long as depth stays under the 20-block lookback.
- **Token accounting gotcha:** the response `input_tokens` field = **only tokens AFTER the last breakpoint**, not the whole prompt. `total = cache_read_input_tokens + cache_creation_input_tokens + input_tokens`.

### OpenAI prompt caching (automatic, zero-config) [openai-caching; openai-cookbook-201; aicostcheck]

- **Automatic:** "works automatically on all your API requests (no code changes required) and has no additional fees." Enabled for gpt-4o and newer.
- **Threshold:** caching kicks in only for prompts **≥ 1,024 tokens**. ("Say you have a 900 token prompt — you'll never get a cache hit.")
- **Prefix match:** "Cache hits are only possible for **exact prefix matches** within a prompt." Routing uses a hash of "typically the first 256 tokens."
- **Discount VARIES BY MODEL** (newer models = steeper discount, from the official cookbook table): gpt-4o **50%** ($2.50→$1.25); gpt-4.1 **75%** ($2.00→$0.50); gpt-5-nano / gpt-5.2 **~90%**; gpt-realtime audio 98.75%. Rule of thumb still quoted broadly: "50% off … automatic" for the 4o/4.1 line [aicostcheck]. Headline claim: "reduce latency by up to 80% and input token costs by up to 90%."
- **TTL:** cached prefixes "generally remain active for **5 to 10 minutes of inactivity, up to a maximum of one hour**"; extended retention up to 24h.
- **Ordering rule:** "Place static content like instructions and examples at the **beginning** of your prompt, and put variable content, such as user-specific information, **at the end**." Longer-but-stable can be cheaper than short-but-uncacheable: a 1,100-token prompt at 70% cache rate saves ~55% vs a 900-token prompt that never caches.
- **Routing:** `prompt_cache_key` parameter improves hit rate "when many requests share long, common prefixes" — set it per session so a session's turns route to the same cache-warm machine.

### Google Gemini context caching [gemini-caching; vertex-caching]

- **Implicit caching:** on by default for Gemini 2.5+; automatic, no code. **Implicit caching provides a 90% discount on cached tokens vs standard input** (Vertex). To raise hit rate: "put large and common contents at the beginning of your prompt" and "send requests with similar prefix in a short amount of time."
- **Explicit caching:** you cache content once, reference it by resource name; **guaranteed** discount — **90% on Gemini 2.5+**, 75% on Gemini 2.0. Storage billed by TTL (**default 1 hour**, no max). Implicit caching has no storage cost.
- **Minimum cache token count:** Gemini 3 family = **4,096**; Gemini 2.5 = **2,048** (2 family = 2,048).
- Note: "The model doesn't make any distinction between cached tokens and regular input tokens. Cached content is a prefix to the prompt." Reinforces prefix-ordering discipline.

### How a live-append transcript interacts with a cached prefix (the key design rule)

Put the ORDER as: **[system prompt] → [tool defs] → [pinned structured state + warm summary] → [hot-buffer transcript, append-only] → [per-tick varying instruction/timestamp].**

- Everything before the append point is a stable prefix → cache read at 10–50% of price.
- Because STT appends new final segments to the END, each tick's new tokens are a suffix; the prefix hash is preserved → high cache-hit rate. This is exactly Anthropic's documented "cache point moves forward automatically" behavior.
- **Watch out:** if you inject a *changing* timestamp, "current time," or a re-generated summary NEAR THE FRONT of the prompt, you bust the entire cache every tick. Regenerate the warm summary only on the incremental cadence (every K turns), and when you do, accept the one-time cache write. Keep any per-tick timestamp at the very end.
- **TTL interaction:** the 5-min default TTL fits a live conversation with sub-5-min gaps between turns (each turn refreshes the cache for free). For meetings with long silent stretches, consider Anthropic's 1h TTL (2× write) or Gemini explicit-cache TTL so the prefix survives the gap.

---

## 3. Delta / event-driven LLM invocation

**Principle: don't call the LLM on every partial ASR token — call it on meaningful boundaries, gated by cheap local signals.**

### Turn-boundary triggers (VAD / end-of-turn)

STT/voice frameworks emit turn lifecycle events. Pipecat fires `on_user_turn_stopped` with the complete transcript when a turn ends, and a fallback `on_user_turn_idle_timeout` (**default 5.0 s** of VAD silence) to retrigger the LLM after the user stops [pipecat]. Meeting-transcription APIs fire a `transcription.processed` webhook **per utterance** with `end_of_turn: true` — classify/process immediately on that event [meetstream]. **Only final (non-partial) segments should reach the pipeline** — partials are noise [eridanus].

### Topic-boundary triggers (only prompt when the topic actually shifts)

A well-documented live pipeline (Eridanus): the Hub forwards each FINAL transcript segment to a pipeline that (a) pre-filters — "skip short or filler-heavy utterances" (`isSubstantive`), (b) updates a lightweight conversation-state tracker, and (c) **only triggers the expensive step when `HasChangedSignificantly()` is true**, measured by **Jaccard similarity on content words** (stopwords removed) against previously seen topics [eridanus].

### Debounce + cooldown (the search-as-you-type technique)

"In a fast conversation, the topic might 'change' five times in 30 seconds… Instead of firing five searches, the pipeline resets a timer on each change and only executes when the conversation settles" — plus a hard **minimum 60 s between broadcasts** cooldown so results queue instead of flooding [eridanus]. An `AckBroadcast` marks a topic "handled" immediately so concurrent segments don't fire duplicate calls (race guard) [eridanus].

### Event-sourced batch triggers (async, not live)

For after-the-fact notes, an EventBridge "Object Created" on transcript upload triggers chunked parallel analysis (Step Functions Map), chunks overlapping ~2K tokens, merged with Jaccard-0.75 dedup [sjfischr; meeting-analyzer]. Relevant if the "notes" step can be async rather than live.

### Composite gate (recommended)

`should_invoke = final_segment AND is_substantive(text) AND (end_of_turn OR topic_changed) AND debounce_settled AND cooldown_elapsed`. Cheap embedding/Jaccard/VAD signals do the gating; the LLM runs only on the survivors. This turns an O(tokens-per-tick) loop into O(calls-per-meaningful-event).

---

## 4. Cost/quality tradeoffs and failure modes (adversarial)

### Recency-only truncation (keep last N, drop the rest) — cheapest, worst quality

"The first instinct is to keep only the last N turns. The implementation takes ten lines, but it ruins the user experience. A user who told the bot their name eleven turns ago will hear the bot ask for it again" [gemilab]. "Pruning by recency alone throws away exactly the kind of information that makes a chat useful." Academic framing: "recency-based truncation or static summarization often causes early, high-impact user constraints to drift out of effective context" (Adaptive Focus Memory, arXiv:2511.12712, as quoted by [sourceshift]).

### Summary drift / semantic erosion — the central failure mode of rolling summaries

Documented, named failure modes (converged across sources):

- **Precision decay under multi-pass ("Memory Entropy" / "Telephone"):** "'Exactly 512 records' becomes 'about 500 records' becomes 'several hundred records'" [tianpan-artifacts]. Goal drift: "'investigate the latency of the Colony SDK's polling client' → 'interested in SDK performance' → 'discussing technical infrastructure'" [thecolony].
- **Negation inversion:** "Negations don't just degrade — they invert. 'User prefers Python but Rust is a backup' → 'User prefers Python' → 'User knows Python.' After three cycles, the constraint has semantically reversed" [tianpan-artifacts].
- **Information drift / premature commitment:** "'I might want to use React' can slowly morph into 'User is building a React app.' The agent starts providing code for a framework you haven't even committed to" [chainofcraft].
- **Dropped rejection reasons:** "'User rejected approach A because of latency' … summarizers frequently preserve the rejection but drop the reason, or preserve the option name but lose that it was rejected. Either error sends the agent back toward the same dead end" [tianpan-artifacts]. "If A was considered and rejected twelve turns ago, the model needs to know that and why" [tianpan-gradual].
- **Fabricated facts:** "A model summarizing … will occasionally introduce information that was implied rather than stated … once that fabrication enters the summary, it becomes 'ground truth' for all future turns" [tianpan-gradual].
- **Collapsed conditionals:** "'if the API rate limit proves to be an issue, then use caching' becomes a flat 'caching is being used' — the conditionality disappears" [tianpan-gradual].
- **Context poisoning:** an early mistake (a typo'd DB name) not marked superseded persists — "the agent will keep trying to connect to a ghost database for the rest of the session" [chainofcraft].
- **Lost specifics (STT-relevant):** "The most common failure mode of rolling summaries is that the summarizer drops user names, reservation numbers, code snippets, or URLs — exactly the things that, if lost, make the bot useless" [gemilab].

Quantified impact: analysis of enterprise agent failures finds "roughly **65% are attributable to context drift and memory loss** — not raw context exhaustion. Teams run out of context window less often than they run out of *accurate* context window" [tianpan-artifacts]. Multi-turn task success drops vs single-turn (58%→35% cited), and naive compression can *shift* the failure from "out of context" to "wrong context" — harder to detect because "the agent continues confidently" [tianpan-artifacts].

### Mitigations (what the sources actually recommend)

1. **Incremental (anchored) updates, not full re-summarization** — see §1; avoids the multi-pass "recursive hallucination"/Telephone cascade [chainofcraft; tianpan-gradual].
2. **Structured facts over prose.** Store must-survive info as atomic **subject-relation-object triples** / typed facts, which "preserve the precision that prose summarization loses, and they're independently searchable" [tianpan-artifacts]. "Hold critical facts in a structured memory layer OUTSIDE the rolling summary … extract these in your application code, store them as JSON, and inject them at the top of every prompt. Do not rely on the summarizer's good intentions" [gemilab]. The Colony argues for a deterministic **state machine** for the "skeleton" (objective/state) with NL only for the "flesh," pinning the high-level objective to a hard state so drift can't move it [thecolony].
3. **Proactive typed extraction at WRITE time** (vs ahead-of-time summarization). "Any 'ahead-of-time' summarization acts as a bottleneck: you compress before you know what the next question will hinge on" (Beyond Static Summarization, arXiv 2601.04463, per [sourceshift]); "From Lossy to Verified" (arXiv 2602.17913) names the "write-before-query barrier" — a summary can lose a decisive constraint (their example: an allergy) with no provenance to recover it [sourceshift].
4. **Preserve provenance / verbatim key pairs.** Keep a `key_qa_pairs` of recent verbatim exchanges with relevance keywords as a partial recovery path [sourceshift]. Keep raw transcript retrievable (don't delete) for on-demand recovery [tianpan-gradual].
5. **Audit loop.** "Save every summary … and once a week sample a few and ask the model to flag any important fact in the original transcript but not in the summary" — catch the summarizer's blind spots and iterate the prompt [gemilab].
6. **Compute cost of summarizing itself:** "Constant summarization cycles consume tokens and VRAM … every extra call to a summarization chain is a latency hit that slows the primary interaction loop" — another reason to gate summarization on the incremental cadence, not every tick [thecolony].

### When the simple rolling summary is "good enough" vs when you need more machinery

Rolling summary + recent verbatim pairs is sufficient when: conversations are **bounded** (one meeting/doc/session), the cost of forgetting one fact is "user re-asks" (not safety/legal/medical), and most sessions are **under ~50 turns** [sourceshift]. If any flip: use tiered memory (MemGPT/Letta-style) for unbounded chats; proactive typed extraction if losing one fact is unacceptable; vector search over history if cross-session lookup matters [sourceshift].

### The cost math

- Naive re-send-everything: per-turn input grows linearly, total session cost ~O(N²). One user's long session silently blows the token ceiling first [gemilab].
- Rolling summary: input "settles around 1,500–2,500 tokens regardless of how many turns" → per-turn cost stops growing [gemilab].
- Caching stacks on top: the stable prefix (system + pinned state + already-seen hot buffer) reads at 10% (Anthropic/Gemini) to 50% (OpenAI 4o) of input price; only the newest appended segment pays full price. Anthropic's own guidance: a slightly longer but stable prefix that crosses the cache threshold is cheaper than a short prompt that never caches [openai-cookbook-201].

---

## Sources

**Vendor docs (primary):**
- [anthropic-caching] Anthropic — Prompt caching. https://platform.claude.com/docs/en/docs/build-with-claude/prompt-caching (4 breakpoints, 1024/2048/4096 min, 5m/1h TTL, 1.25×/2×/0.1× multipliers, 20-block lookback, longest-prefix, automatic-caching-moves-forward, static-first ordering).
- [openai-caching] OpenAI — Prompt caching guide. https://developers.openai.com/api/docs/guides/prompt-caching (automatic, ≥1024 tokens, exact-prefix, static-first, 5–10min→1h TTL, up to 80% latency / 90% cost, prompt_cache_key).
- [openai-cookbook-201] OpenAI Cookbook — Prompt Caching 201. https://developers.openai.com/cookbook/examples/prompt_caching_201 (per-model discount table: gpt-4o 50%, gpt-4.1 75%, gpt-5.2 90%; longer-stable-prefix-cheaper).
- [gemini-caching] Google — Gemini API context caching. https://ai.google.dev/gemini-api/docs/caching (implicit default 2.5+, min tokens 4096/2048, prefix-ordering, explicit TTL default 1h).
- [vertex-caching] Google Cloud — Vertex/Agent Platform context caching overview. https://cloud.google.com/vertex-ai/generative-ai/docs/context-cache/context-cache-overview (implicit 90% discount, explicit 90%/75%, min 4096/2048).

**Academic (arXiv):**
- [arxiv-2308.15022] Recursively Summarizing Enables Long-Term Dialogue Memory in LLMs (Wang et al., 2023; ~78 cites).
- [arxiv-2310.08560] MemGPT: Towards LLMs as Operating Systems (Packer et al., 2023).
- [arxiv-2305.10250] MemoryBank: Enhancing LLMs with Long-Term Memory (Zhong et al., 2023; Ebbinghaus forgetting).
- [arxiv-2304.03442] Generative Agents: Interactive Simulacra of Human Behavior (Park et al., 2023; ~4654 cites; memory-stream + retrieval scoring).
- [arxiv-2402.09727] ReadAgent: A Human-Inspired Reading Agent with Gist Memory (Lee et al.; up to 20× effective context).
- Also surfaced/relevant: Lost in the Middle (2307.03172 — position bias motivates keeping key facts near ends, not buried mid-context); StreamingLLM/attention sinks (2309.17453 — KV-cache reuse for streaming decode); Longformer (2004.05150 — sliding-window attention); LongLLMLingua (2310.06839 — prompt compression). Adaptive Focus Memory (2511.12712), Beyond Static Summarization (2601.04463), From Lossy to Verified (2602.17913) — cited secondhand via [sourceshift]; verify directly before load-bearing use.

**Practitioner / engineering blogs (2026):**
- [tianpan-artifacts] "Context Compression Artifacts: What Your Summarization Middleware Is Silently Losing" — tianpan.co (2026-05-05). Failure fault-lines, atomic-triple mitigation, 65% figure.
- [tianpan-gradual] "Gradual Context Replacement: Managing Long AI Conversations Without Losing Quality" — tianpan.co (2026-05-07). Hot/warm regions, anchored incremental update.
- [gemilab] "Compressing Gemini API Chat History with Rolling Summaries" — gemilab.net (2026-04-27). Two-tier rolling summary, 1500–2500 token settle, structured-facts mitigation, audit loop.
- [chainofcraft] "Summarization != Memory" — chainofcraft.substack.com (2026-02-04). Hot buffer 5–10 turns, Memory Entropy, information drift / context poisoning / goldfish effect.
- [thecolony] "Natural language summaries cause semantic erosion in agent long-term memory" — thecolony.cc (2026-05-30). State-machine skeleton + NL flesh; summarization compute cost.
- [sourceshift] "The simplest survivable form of chat memory" — blog.sourceshift.io (2026-01-04). Cadence tuning, key_qa_pairs, when-simple-is-enough decision rule.
- [eridanus] "How Knowledge Context Works" — research.jayphen.com. Live pipeline: substantive pre-filter, Jaccard topic-change gate, debounce + 60s cooldown, AckBroadcast race guard.
- [meetstream] "Zero-Shot Topic Classification" — meetstream.ai. Per-turn NLI classification on end_of_turn webhook.
- [pipecat] Pipecat turn events docs. on_user_turn_stopped, 5.0s idle timeout.
- [charleschen-rag] "How to Build RAG for Meeting Notes and Transcripts" — wiki.charleschen.ai. Topic segmentation over fixed-token chunking, 300–800 token chunks.
- [erikbahena] transcript-ai (GitHub). DeepTiling embedding boundary detection, zero LLM calls for segmentation.
- [sjfischr / meeting-analyzer] meeting_transcript_analyzer (GitHub). EventBridge trigger, overlapping chunks, Jaccard-0.75 dedup.
- [aicostcheck] "Prompt Caching Savings 2026: OpenAI vs Anthropic" — aicostcheck.com (2026-03-03). OpenAI 50% automatic vs Anthropic 90%.

**Caveats:** Some 2026-dated model names/prices from WebFetch of live pricing pages (e.g. "gpt-5.2") could not be independently double-checked and may include model-name artifacts — treat exact SKUs/prices as indicative; the Anthropic multipliers (1.25×/2×/0.1×), the prefix-ordering rule, and the tiered-memory + incremental-update pattern are the robust, cross-source-verified findings. Three secondhand-cited arXiv IDs (2511.12712, 2601.04463, 2602.17913) were not fetched directly.
