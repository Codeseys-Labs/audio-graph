# System-One model landscape for AudioGraph

**Research date: 2026-11-21.** Public-source research, not a new inference benchmark. Prices below are USD per million input tokens unless stated otherwise; local weights have no per-request API fee, but hardware, electricity, packaging and licensing still cost something. “Not published/verified” means this investigation did not establish the fact, not that the capability cannot exist.

## Question and direct answer

Which small, fast decision/classification models can AudioGraph evaluate now, and how do they connect to its existing `/v1/systemone` contract?

**Keep the current native Laya and D3 engines as the first evaluation tier; add a small encoder baseline and evaluate Baseten's Joey-1 as the most interesting newly published small decision model. Use hosted services as explicitly authorized comparators, not fallback routes.** The principal changes since the October work are:

1. **Jev is real and current direct TypeSafe stable is `jev-1.14.0`**, now also the target of `jev-latest`; the existing October live result was **1.13**, not evidence for 1.14. Cloudflare's `typesafe/jev` listing still identifies **1.13**. [J1][][J2][][C4][]
2. **Clef and Clef-flash are real, Apache-2.0, open-weight decision models**; current Cloudflare also offers **Clef-omni**, a 30B-total/3B-active multimodal decision model. Native Clef-flash already exists in AudioGraph, although the product route layer still reserves `NativeClef`. Do not commission its implementation again. [C1][][C2][][C3][][C5][][R3][][R5][]
3. The user's **“claeric” is almost certainly Clairic**—an identification inference, not an exact spelling match. **Clairic and Clairic-flash were released open-weight on November 18**, at 27B and 9B. Crucially, the updated extraction architecture **does generate tokens for ordinary text/number fields**, in parallel slots. The October claim that all Clairic extraction is a single non-autoregressive pass is no longer correct. It is **not** a drop-in implementation of AudioGraph's choice/noul/score contract. [X1][][X2][][X3][]
4. **Baseten is now a model publisher, not merely a host.** Its documented shared System One Model APIs include **Joey-1, Elephant-1, Mammoth-1, Groundhog-1 and Bison-1**. D3 and D3-mini have dedicated-deployment templates, a different availability/billing category. No first-party evidence found makes Laya or proprietary Jev a Baseten shared Model API. [B1][][B2][][B3][][B4][][B5][][B6][][B7][][B8][]
5. **D3 has upstream tags through `v3.1.3`; Laya's Python package is now 0.5.2** (0.5.0 released November 19; 0.5.1 and 0.5.2 November 21). Laya 0.5 changes the default reader/decision architecture to Vega; it is **not an update that can simply be dropped into the existing classic ModernBERT ONNX bundle**. [D1][][D7][][L1][][L2][]
6. **OpenAI and Google now have genuine typed Decisions APIs**, not just JSON generation: OpenAI `/v1/decisions` with `gpt-6-luna`, and Google `/v1beta/decisions` with `gemini-decision-1` (announced November 11). Both need explicit Rust transport adapters. Anthropic's verified alternative remains schema-constrained **Messages generation**, not an equivalent published probability endpoint. [O1][][O2][][G1][][G2][][G3][][A1][]

This is a scoped, verified shortlist, not a claim to enumerate every model on a changing leaderboard. A leaderboard name alone is not evidence of downloadable weights, an accessible endpoint or calibrated application probabilities.

## 1. What compatibility actually means here

### The checked-in contract, not a generic “OpenAI-compatible” label

[`decision-contract/src/lib.rs`](../../src-tauri/crates/decision-contract/src/lib.rs) defines:

- `DecisionRequest { state: Text | Json, questions: OrderedMap }`; question instructions are **strings**, choice descriptions are `Option<String>`, and score criteria are ordered strings.
- `Choice`: a selected key, a probability for every requested key, and required `confidence`.
- `Noul`: `noul = P(true)`, optional binary probabilities and optional confidence. The HTTP validator currently requires binary map keys **`false` and `true`** when the map is present.
- `Score`: a scalar on zero-based rubric indices, a vector of per-level probabilities, and required confidence. Canonical wire serialization uses keys `"0"`, `"1"`, etc.; contiguous numeric-key maps and arrays deserialize.
- No typed extraction values, spans, bounding boxes, media attachments, refusal answer variant, prediction sets, or per-answer abstention fields. `WireRequest`/`WireResponse` preserve original vendor JSON and unknown metadata; that preservation does not add typed execution capabilities. [R1][][R2][]

The contract is a common subset, not the entire newest vendor schema. Jev's Box, Clairic fields, Liquid extraction, and media extensions need separate typed design before use. Do not smuggle them through JSON state and claim that backend-owned media/PCM policy has been satisfied. [J1][][X2][][Q1][][R1][][R6][]

**Confidence is not portable.** Native engines use probability of the reported answer; HTTP engines preserve vendor confidence. Baseten documents concentration rather than correctness probability, vLLM next-token confidence includes vocabulary label mass, and Google describes top-outcome probability. A shared `confidence > 0.8` gate would compare different quantities. Preserve raw values and fit route/model/schema-specific thresholds on human labels. [B2][][V1][][G2][][R2][][R3][]

### Integration classes and concrete wire mapping

| Candidate | Existing contract fit | Required adaptation / important boundary |
|---|---|---|
| TypeSafe Jev | **Direct**, existing engine | `POST https://api.typesafe.ai/v1/systemone`, bearer key, `{model,state,questions}`. Existing `EndpointKind::TypeSafe` hardcodes `jev-latest`; version selection needs a deliberate change if evaluations must pin 1.13/1.14. Current limits do not encode Jev's separate request/question budgets. [J1][][J2][][R2][] |
| Cloudflare Jev | **Existing wrapped transport** | Account `/ai/run`, `{model:"typesafe/jev",input:{state,questions}}`; unwrap REST `result`/`success`. Not `@cf/typesafe/jev`. Keep its route and served identity distinct from TypeSafe direct. [C4][][R2][] |
| Cloudflare Clef-flash | **Existing wrapped transport** | Account `/ai/run/@cf/cloudflare/clef-flash`, flat decision request; existing envelope handling. [C2][][R2][] |
| Clef / Clef-omni hosted | **Same decision shape, new model/route variants** | Add the exact account model URLs and bounds; current `CloudflareModel` only has `Jev` and `ClefFlash`. Text-only first. Audio/video are additional content contracts, not justified by matching question JSON. [C1][][C3][][R2][] |
| Native Laya/D3/Clef-flash | **Already implement `DecisionEngine`** | Reuse their registry/model-manager boundaries; do not add vendor HTTP protocols inside the native crate. New checkpoint/runtime families still require parity and new pins. Native Clef product admission remains separate. [R3][][R5][] |
| Baseten's five Model APIs | **Near-direct System One wire, new hosted transport** | Bearer key at `https://inference.baseten.co/v1/systemone`; model slug selects backend. Optional `service_tier`/`max_len`. Enforce **512 total options per request** (noul counts two), in addition to per-question limits; `EngineLimits` has no aggregate-budget field. Reject unsupported top-level scalar JSON state before dispatch. Preserve vendor concentration confidence. [B1][][B2][] |
| Baseten dedicated D3 / custom Laya | **Deployment-specific** | Use the actual deployed URL/auth/body contract, not a fabricated shared Model API slug. A public cloud deployment cannot use the existing `LocalHttp` kind: loopback-only checks deliberately prohibit this. [B7][][B8][][B9][][R2][] |
| Local vLLM / Ollama | **Existing HTTP engines, with compatibility caveat** | Keep IP-literal loopback, redirects/proxies disabled, and Ollama `:cloud` rejected. Common client caps remain 64 questions/26 options/10 levels, even when a specialized upstream supports more. [R2][][V1][][V2][] |
| GLiClass, fixed-head encoder, NLI, reranker | **New scoring adapter, not native wire** | Compile typed questions into supported model tasks; convert logits into justified distributions; return typed answers in Rust. Unsupported general questions must return `UnsupportedQuestion`, not invented probabilities. [E1][][E2][][E3][][E4][] |
| OpenAI Decisions | **New transport adapter** | Map `state` to `input`; question map to ordered array with `name`; `noul` to `predicate`; choice criteria to `choices[{value,description}]`; score criteria to `levels`. Map answer arrays back by unique name, choice `{value,probability}` entries to keyed maps and score entries by numeric `value`. Predicate `probability` becomes `noul`. A provider refusal is **not** a zero-probability decision; fail/abstain outside `Answer` rather than fabricate a class. [O1][][O2][] |
| Google Decisions | **New transport adapter, very close typed shape** | `state` becomes `input`; `x-goog-api-key`; noul criteria keys become `true_description`/`false_description`. Convert `usage.total_input_tokens`/`total_output_tokens`; current examples lack a served-model field, so stamp model identity as **Requested**, not Served. Choice null descriptions need the documented empty-string/name-only representation. Enforce Google's restricted question/option keys; map JSON state explicitly as serialized text where necessary. [G2][] |
| Clairic / GLiNER extraction | **Not losslessly expressible** | Preserve values, nulls, offsets, confidence, completeness and provenance in an extraction contract; don't coerce a free numeric value into ordinal `Score`. A separate bounded candidate-validation question can still use `noul`/`choice`. [X2][][E5][][R1][] |

**A concrete current interoperability gap:** the current vLLM guide returns `noul.probabilities = {"yes":p,"no":1-p}`; AudioGraph's HTTP validator accepts only `true`/`false`. **[INFERENCE from source comparison]** that documented response would deserialize but be rejected by `validate_answers`. It was not reproduced against a running server in this research. A boundary normalization must preserve the original `WireResponse` bytes and avoid rewriting vendor confidence. Parser fixtures should cover both key conventions and inconsistent pairs. [V1][][R2][]

## 2. Already-integrated local families: upstream status and evaluation value

### Laya: distinguish the classic checkpoints from the new default

| Variant | Size / context / language | Performance and calibration evidence | Deployment, license, price |
|---|---|---|---|
| Classic English `convaiinnovations/laya` | **421M** including decision components; ModernBERT-large; card context **512**. English. | Vendor T4 example: **39.5 ms**, one question; English typed-decisions accuracy **0.362**, versus **0.766** for the task fine-tune. RLCD/proper-scoring-rule training and shipped calibration are useful mechanisms, not guarantees under domain/language shift. [L1][] | Local PyTorch/ONNX; Apache-2.0, no API fee. AudioGraph uses pinned `receptron/laya-onnx`, not moving upstream `main`. [L1][][R5][] |
| Classic multilingual `laya-multilingual` | **322M**, mmBERT-base; default **1,024**, upstream supports `max_len=8192`; claimed 100+ languages. | Vendor T4 **32.8 ms** for one question; XNLI 15-language mean **0.731**, versus English checkpoint **0.521**. Usable performance is not uniform across claimed languages. Longer 8k execution and its accuracy are not inherited by the installed 1k route. [L1][] | Local; Apache-2.0. Current AudioGraph model manager now has a pinned `soyelmismo/laya-multilingual-onnx` source plus file hashes—newer repo evidence than ADR-0049's “distribution pending” text. Packaging remains tracked by `audio-graph-fb61`. [R5][] |
| `laya-typed-decisions` | **421M**, context **1,024**, four workflow domains | **0.766** vendor accuracy is not an AudioGraph meeting-label benchmark. Useful off-the-shelf comparator before task-specific fine-tuning. [L1][] | Local Apache-2.0 checkpoint; needs a separately pinned export and model identity; not already the shipped English ONNX model. |
| **Laya 0.5 / Vega** | Default frozen **0.8B reader**, alternative 4B/27B readers; advertised **73,728** context; multimodal front ends. | Vendor reports fitted-stream ECE **0.028** on banking77 and **0.031** on typed-decisions with a 4B reader, plus conformal sets/abstention. These involve learned heads and explicit label counts, not zero-shot calibration of the existing encoder. The published small example routes all 20 held-out cases to review after fitting when calibration evidence is insufficient. [L1][][L2][] | Package Apache-2.0; reader licenses separately matter. 0.5.2 observed in primary PyPI metadata. Python/reader integration is new work; no Rust/ONNX parity or AudioGraph latency established. Its `Escalator`/teacher and adaptive routing must **not** override AudioGraph's no-fallback/no-silent-egress policy. [L2][][R3][] |

The latest package keeps classic encoders selectable explicitly. The model card's “new in 0.5” and “checkpoints unchanged in 0.3.20” sections describe **different layers of the release**, not proof that 0.5 weights match AudioGraph's ONNX pin. [L1][][L2][]

AudioGraph's language routing presently detects non-Latin script in state/questions, not full language identity: Latin-script French or Spanish can still select English. **[INFERENCE]** multilingual evaluation must include these cases, not just Japanese/Hindi. Upstream itself reports an English-checkpoint Khmer example with zero accuracy at high confidence, illustrating why confidence alone cannot repair unsupported-language routing. [L1][][R5][]

### Decision 3.0: still real, open, and broader upstream than the native subset

All rows support **choice/noul/ordinal score**, local weights, text/JSON and upstream image/video decisions; none is free-form extraction. All are **Apache-2.0**. Parameter counts below include vision components and are not the smaller text-body counts in AudioGraph's runtime memory tables. Vendor latency is warm, one request at a time on **AMD MI325X**, not a desktop promise. [D1][][D2][][D3][][D4][][D5][][D6][]

| Checkpoint | Released parameter count | Vendor text median | AudioGraph native status |
|---|---:|---:|---|
| `d3-edge` | **0.59B**, including 0.10B vision | **6.6 ms** | Integrated, text only; warm RTX 5090 short-request p50 **6.28 ms**, CPU p95 **400 ms**. [D2][][R7][] |
| `d3-lite` | **0.85B**, including 0.10B vision | **13.6 ms** | Integrated; RTX 5090 p50 **11.3 ms**, CPU p95 **764 ms**. [D3][][R7][] |
| `d3-nano` | **2.21B**, including 0.33B vision | **13.4 ms** | Integrated; CUDA BF16 p50 **9.63 ms**, CPU FP32 p95 **1,711 ms**. [D4][][R7][] |
| `d3-mini` | **4.54B**, including 0.33B vision | **18.1 ms** | Integrated; CUDA BF16 p50 **14.7 ms**, CPU FP32 p95 **3,851 ms**. [D5][][R7][] |
| `d3-flash` | **8.39B**, including 0.46B vision | **19.2 ms** | Upstream/local server candidate; not one of AudioGraph's four native registrations. [D6][][R5][] |
| `d3` | **26.09B**, including 0.46B vision | **54.4 ms** | Upstream quality comparator; too large to assume an ordinary desktop budget. [D1][] |

Local API fee is zero; production demo access is not a latency/SLA contract. D3/mini Baseten hosting is discussed separately below. Qwen-derived multilingual ability and AudioGraph's French/Japanese parity fixtures establish input handling, **not per-language calibrated quality**. No route-specific multilingual ECE/Brier evidence was established. [D1][][D2][][D3][][D4][][D5][][D6][][R7][][R8][]

**Version status:** Hugging Face primary refs list `v3.1.3` for both full D3 and edge. Observed edge `main` was `bee5e872f75b9c5d1405767bf244a6948b95c0af`, full D3 `main` was `62f07e54c6c3a5ff5f695107377edc43e1a86ea8`, modified November 2. AudioGraph edge remains pinned to `c454ee39...`; an upstream ref change does **not** by itself establish a weight change. The full D3 card says the scored v3.1.0 weights are unchanged in its current version. Review changed files/config/prompt/readout and rerun parity before a pin migration; don't replace the pinned artifacts merely because a tag is newer. [D7][][D8][][R5][]

**Calibration/limits:** upstream leaderboard scores primarily demonstrate task quality, not meeting-domain calibration. AudioGraph's native temperature 1.0 and FP32/BF16 parity establish runtime behavior, not calibrated authority. D3 performs a forward per question rather than Clef's joint schema head. Long state and many questions therefore materially change latency. Native “no configured state limit” is not unlimited practical memory. The current upstream full-D3 card still warns about bidirectional full-context profiling and version-specific serving installation; pin the actual server build rather than treating developer-preview docs as a stable-release guarantee. [D1][][R7][][R8][][V1][]

## 3. TypeSafe and Cloudflare hosted/open-weight decision choices

| Model | Shape, size, context | Latency, language, calibration | License, price and AudioGraph decision |
|---|---|---|---|
| **Jev 1.14 direct** | Choice **2–255**, score **2–10**, noul; additionally Box/image functionality outside our contract. Size/weights undisclosed. **64k request / 32k question**; opt-in long-context header raises both to **262k**. | Multilingual improved in 1.14, no complete evaluated language inventory verified. RLCD is vendor evidence. Existing AudioGraph **1.13**, 12-public-span replay: p50 **87.8 ms**, p95 **428.0 ms**, kind agreement 12/12 and ECE **0.0367** against heuristics—not independently labeled accuracy and not a 1.14 benchmark. [J1][][J3][][R4][] | Proprietary hosted service, **$0.042**, output free; no self-hosting weights verified. Best low-friction cloud comparator because adapter/key slot exist; pin version before comparing thresholds. [J1][][J2][] |
| **Jev via Cloudflare** | `typesafe/jev`, listed **1.13**, context **32,000** | No separate current end-to-end benchmark established. Don't transfer direct-TypeSafe network measurements or 1.14 language changes. [C4][] | Third-party API route, **$0.042**, free output; Cloudflare token/account, separate egress provenance. |
| **Clef** | **27B**, **65,536** hosted context; choice/noul/score; text/JSON and multimodal upstream | Cloudflare launch suite median **209.3 ms**, p95 **238.6 ms**, versus flash 38.8/122.4. Joint schema scoring, Brier/label-smoothed losses and RLCD-style objectives are disclosed. Launch task scores are not current Jev-1.14 comparisons or AudioGraph ECE. Multilingual task-level calibration not verified. [C1][][C5][] | Apache-2.0 local weights; hosted **$0.24**, output free. New hosted enum/route; native 27B port is not a cheap alias for flash. |
| **Clef-flash** | **9B**, hosted **24,576** context; native release renderer **16,384** total-token budget | Launch median **38.8 ms**, p95 **122.4 ms**. Existing AudioGraph BF16 warm **57.6/60.0 ms p50/p95** for 3 questions/335 tokens on RTX 5090; **279/284 ms** for 4 questions/1,930 tokens. Pre-review parity numbers must not be relabeled as current after the cosine-clamp fix (`audio-graph-a1c7`). [C2][][C5][][R9][] | Apache-2.0, hosted **$0.038**, output free. Native implementation exists; ~**14.2 GB** measured device use and **19 GB** full download make it a workstation option, not smallest-model default. Quantization is existing `audio-graph-5678`. [R9][] |
| **Clef-omni** | **30B total / 3B active**, **64,000** hosted context; text/JSON/images/audio/video; joint non-generative decisions | No controlled AudioGraph or comparable latency established in this research. Do not infer 3B memory from active count. Multilingual calibration inventory not verified. [C3][][C6][] | Apache-2.0 released weights, hosted **$0.15**, output free. Relevant future raw-audio comparator, but adding `RawAudio` egress and capture/timing contracts is separate from text-decision integration. |

Cloudflare's older launch article says Jev is text-only; current TypeSafe documents images. Use current provider API documentation for input support, and identify the model/version used by any historical benchmark. [C5][][J1][]

### “Claeric” resolved: Clairic is extraction, and its November architecture matters

**Verified spelling: Clairic**, with **Clairic-flash** as the smaller current sibling. The November release explicitly supersedes the preview: 27B Clairic occupies the original endpoint, and 9B moves to flash. Both have Apache-2.0 open weights. They accept typed `fields` rather than decision `questions`: text/spans, numbers, boolean/enum/object/list, date/time/datetime/duration, compute, estimates and forecasts. Extraction answers can contain `value`, `p_null`, sequence confidence, evidence offsets, `valid`/`complete`; estimates include interval/quantile information. [X1][][X2][][X3][]

- **Latency:** vendor self-hosted full-HTTP text-field p50/p95 **358/736 ms** for Clairic, **155/328 ms** for flash, 96 requests each. Image rows are **453/976** and **305/605 ms**. These aren't Cloudflare-region measurements, and the source does not make them AudioGraph hardware measurements. [X1][]
- **Context/price:** hosted model pages advertise **65,536** tokens. On November 21 both cost **$0.10/M input**; the announced December 1 standard Clairic rate is **$0.24**, flash remains **$0.10**. Input may be billed repeatedly across passes; inspect `usage.passes`. Free output billing **does not mean zero generated tokens**. [X1][][X4][][X5][]
- **Calibration/language:** confidence/null estimates and bounded intervals are useful interfaces; vendor internal field-accuracy tests are not a measured AudioGraph calibration curve. No comprehensive extraction-language quality guarantee verified. Model-derived/calculated values must not automatically become source-grounded graph facts. [X1][][X2][]
- **Documentation conflict:** hosted pages retain “preview” and old truncation/pass prose while parameter sections say input is never truncated; November release says graduation and redesigned generation. Treat the released schema/model/version and `truncated`/completeness indicators as explicit test targets; do not silently choose whichever prose is convenient. [X1][][X2][][X4][][X5][]

**AudioGraph implication:** evaluate exact transcript span extraction only through a separately designed extraction/provenance path. Converting enum/boolean fields into decision answers would lose the complete per-option probability contract; converting estimates into ordinal scores would change their meaning. Route outputs through existing graph/claim validators; decision confidence never supplies evidence by itself. [R1][][R3][][R6][]

### Other verified Workers AI classification choices

The catalog also lists **`@cf/huggingface/distilbert-sst-2-int8`**, a fixed **English sentiment** classifier at **$0.0263/M**, and **`@cf/baai/bge-reranker-base`**, a query/context relevance scorer at **$0.00311/M**. These have different payloads (`text`, or `query`/`contexts`) and no arbitrary instruction-defined System One contract. The catalog's BGE embeddings are encoders, not calibrated decisions. No relevant single-request latency or application calibration measurements were established for these hosted routes. They are task-specific baselines, not substitutes for Jev/Clef. [C7][][C8][][C9][]

## 4. Baseten: published models versus hosting someone else's model

### Published and shared hosted models that actually exist

All five are in Baseten's documented **shared Model API** list and expose choice/noul/score; none of these APIs is a free-form extraction interface. Cards label language **English**; base-model multilingual ability is not evidence of multilingual decision calibration. Self-hostable weights use **MIT for Baseten modifications plus Apache-2.0 for the base model**, not an unspecified “Baseten proprietary model license.” API service terms remain separate. [B1][][B2][][B3][][B4][][B5][][B6][][B8a][][B8b][]

| Model / Model API slug | Size / trained context | Default hosted price | Quality/calibration and latency evidence |
|---|---|---:|---|
| **Joey-1**, `decision-models/baseten-joey-2b-latest` | **2B**, Qwen3.5-2B; **16,384** | **$0.01** | CE+Brier training and explicit uncertainty option; vendor public-index results. No comparable warm single-request p50/p95 established here. **Most interesting new small open-weight challenger** to D3 nano. [B3][][B6][] |
| **Elephant-1**, `decision-models/baseten-elephant-9b-latest` | **9B**, Qwen3.5-9B; **16,384** | **$0.035** | CE+Brier; vendor score **62.65** on Jev Index 0.4, with other tables explicitly using public-only or replica scores. Not comparable to D3 0.3.2 or AudioGraph labels without rerunning the same evaluation. Latency unverified here. [B3][][B4][] |
| **Mammoth-1**, `decision-models/baseten-mammoth-27b-latest` | **27B**, Qwen3.5-27B; **16,384** | **$0.10** | CE+Brier; vendor Jev Index 0.4 **71.24** and own AA replica, not a permanent world-ranking claim. Latency unverified here. [B3][][B5][] |
| **Groundhog-1**, `decision-models/baseten-groundhog-latest` | **26B total / ~4B active**, Gemma-4 MoE; **131,072** | **$0.035** | Same newer recipe family; card explicitly says **not yet scored on AA Decision Index**. No model-specific ECE/Brier or controlled latency established here. [B3][][B8a][] |
| **Bison-1**, `decision-models/baseten-bison-latest` | **31B**, Gemma-4 dense; **131,072** | **$0.10** | Card attributes AA Index v2.1 **76**, **Brier 0.14**, to November 19 scoring. This is the strongest explicit external calibration claim among the Baseten cards read, but remains a card-reported result, not an independently rerun experiment here. Latency unverified here. [B3][][B8b][] |

Do **not** reuse the October free-week language as today's price: elephant/mammoth cards say it ended October 27. Current pricing lists the rates above. Shared APIs bill only input for these models; their `usage.output_tokens` may still report one token per question. Flex is approximately 90% of default price, priority normally 2×; exact tier/region/account terms must be pinned in an evaluation record. [B1][][B2][][B3][]

Architecture is not interchangeable with the native D3/Clef implementations. Baseten's Qwen models use **block-bidirectional full attention with a shared prefix and independent suffixes**, not D3's entire per-question bidirectional prompt or Clef's joint schema head. **[INFERENCE]** AudioGraph's Qwen3.5 primitives may be reusable, but a new renderer/mask/readout and parity fixtures are still necessary; a model-ID alias is not a correct native implementation. [B10][][R8][][R9][]

### Hosting D3, Laya and Jev

- **D3 and D3-mini:** verified Baseten library entries expose **Dedicated deployment** buttons, not the five-model shared API table. Use actual deployment auth and endpoint instructions; pricing is dedicated compute, not a verified D3 token tariff. [B7][][B8][][B3][]
- **Other D3 sizes:** technically plausible via custom container/Truss, but a production template for every size was not verified. Label this as a proposed custom deployment, not existing shared hosting. [B9][]
- **Laya:** public local weights and Baseten's custom Python/container deployment mechanism make hosting feasible **[INFERENCE]**. No first-party shared `laya` slug or turnkey library entry was verified by targeted searches. For AudioGraph this sends transcript content off-device while forfeiting Laya's local-first advantage; only useful for a controlled comparison or managed fleet need. [L1][][B9][]
- **Jev:** no public weights verified, so “deploy Jev on Baseten” is not justified. One can write a proxy to TypeSafe **[INFERENCE]**, but that is a **second processor/hop**, not native Jev hosting or a Baseten model. It would need both credential scopes, explicit disclosure and separate approval; it is not recommended here. [J1][][B9][][R6][]

## 5. Credible small alternatives and how to score them honestly

For the following local alternatives, no per-call API price applies; hosting adds its own charges. No AudioGraph end-to-end latency/calibration run was executed. Do not turn batch throughput into single-item latency, and do not describe arbitrary sigmoid scores as calibrated probabilities.

| Candidate | Supported tasks and size | License / languages / speed evidence | Contract adaptation and evaluation role |
|---|---|---|---|
| **GLiClass Instruct** | Arbitrary-label zero/few-shot classification; prompts and label descriptions; edge **32.7M**, base **187M**, large **439M**. Choice directly after single-label scoring; noul via explicitly trained two-outcome schema; score via ordinal levels. No general extraction. | Apache-2.0 for checked edge card. Vendor average F1 **0.4922/0.6525/0.7199** respectively; no calibrated AudioGraph probability evidence. Multilingual family below is preferable to assuming English transfers. [E1][][E2][] | Best tiny dynamic-label baseline. Preserve single-label softmax versus multi-label sigmoid semantics; multiple independent labels are multiple noul questions, not one exclusive choice. Fit held-out temperature/thresholds; new native ONNX adapter/export/parity required. |
| **GLiClass Multilang / Modern V3** | Multilang edge **~140M**, mini **~288M**, ultra **~1.72B**; ModernBERT-based V3 base **151M**, large **399M**. | Checked multilang-mini Apache-2.0; **20 training languages**, cross-lingual labels/text. Mini vendor **513.4 samples/s**, batch 8, RTX PRO 6000; not 1.95 ms single-call latency. Modern V3 published task F1/speed, not calibration. [E1][][E6][] | Compare mini for multilingual routing, edge for CPU budget. Native runtime support and per-language meeting accuracy remain unverified. |
| **ModernBERT fixed-head fine-tune** | Base **149M**, large **395M** backbone, **8,192** native context. A supervised head gives fixed-taxonomy choice; binary head gives noul; ordinal/categorical rubric head gives score. Base MLM alone is **not** a ready question-answering classifier. | Apache-2.0; English/code training. No universal latency, calibration or task accuracy without selecting/fitting a checkpoint. [E7][] | Strong boring option for stable `agent_proposal_kind`/`agent_signal_grade`. Smaller than Laya's general decision head; doesn't accept novel label taxonomies without retraining. Export frozen task head to Rust/ORT and version schema with weights. |
| **Ettin fixed-head fine-tune** | Encoder family **17M, 32M, 68M, 150M, 400M, 1B**; same supervised task shapes as ModernBERT. | **MIT**, English; published encoder-versus-decoder comparisons, not AudioGraph quality. No verified tiny arbitrary-typed decision checkpoint or portable p50/p95 just from the base card. [E8][] | Evaluate 32M/68M for CPU-only devices against the same human-labeled set. Less runtime/memory ambition than a 2–9B general model; that's an architectural inference, not a measured win. |
| **ModernBERT NLI zero-shot fine-tune** | Verified `MoritzLaurer/ModernBERT-base-zeroshot-v2.0`, ~149M backbone; arbitrary labels as hypotheses, no extraction. | Apache-2.0; primarily English benchmark mix. Card reports **3,472 texts/s mean** on A100, batch 128; not desktop latency. No app calibration reported. [E3][] | Compile each option's meaning as a hypothesis. Cost scales with state/label pairs. Treat score rubrics similarly only after validating ordinal behavior. |
| **Multilingual NLI cross-encoder** | Verified `MoritzLaurer/mDeBERTa-v3-base-mnli-xnli`; entailment/neutral/contradiction. Size is base encoder class; exact parameter total not established from the read card. | **MIT**, 100-language pretraining and 15-language XNLI evaluation; vendor mean XNLI **0.808**. No comparable latency or classification calibration measured here. [E4][] | Relevant for claim support. **Neutral is not contradiction**: don't turn lack of entailment into proof of falsehood. Prefer explicit support/contradiction/unknown choice or separate conservative predicates. Entailment softmax across candidates conditions on that candidate set; add an out-of-scope option. |
| **Reranker-as-classifier** | Verified `BAAI/bge-reranker-v2-m3`, multilingual query/passage relevance; retrieval score, not arbitrary decision semantics. Exact parameter total/latency not established from the cited card. | Apache-2.0. The documented optional `[0,1]` normalization is **sigmoid**, not calibration evidence. [E9][] | Useful for candidate entity matches, relevance or evidence ranking. Learn/calibrate a binary support mapping or a normalized candidate distribution on task labels; independent relevance scores do not inherently sum to one. Do not invent a rubric distribution from one scalar. |
| **GLiNER** | NER/span extraction over supplied entity types; v2.1 English small/medium/large **166M/209M/459M** and multilingual **209M** as reported by the family card. | **v2/v2.1 Apache-2.0**; older v0/v1 cards list **CC-BY-NC-4.0**. Multilingual checkpoint exists; no complete language calibration inventory or portable latency verified here. [E5][] | Alternative to extraction, **not a sentence-level GLiClass substitute**. Keep spans/types/confidence in extraction results. A separate decision engine may judge an already extracted candidate; do not collapse span scores into sentence probabilities. |

### Small instruction models with logprob scoring

**Verified implementation route:** current vLLM exposes `/v1/systemone` for supported generative architectures with a **next-token read strategy**. Its docs explicitly list Qwen3-0.6B, Qwen3.5-0.8B and Gemma4 variants. The Qwen3.5-0.8B card is Apache-2.0, advertises 201 languages/dialects and 262,144 context; this does not establish decision calibration or useful 262k desktop latency. [V1][][Q2][]

For mutually exclusive one-token labels with logits `z_i`, the adapter computes `p_i = softmax(z_i/T)`. For noul, score **both** true and false; for score, compute `sum(i*p_i)` and retain the entire distribution. Tokenization, template, candidate ordering, temperature, precision and quantization are part of model identity. Multi-token labels require sequence scoring or a verified one-token code mapping, not just the first token. [V1][]

vLLM also reports **`label_mass`**, the vocabulary probability assigned to all label tokens, and defines confidence using that mass. High normalized candidate probability with low label mass is not strong model agreement. A finite top-k logprob response can omit labels; don't fill missing candidates with zero or fabricate a complete probability vector. Batch/cache dependence exists unless the documented invariant mode is used. Treat this as a baseline with calibration/abstention evaluation, not automatic equivalence to RLCD/post-trained decision weights. [V1][]

### Additional real typed-decision families worth a bounded watch/evaluation slot

- **Liquid d1:** verified `d1-3B` (**3.12B**, 32,768 context) and newer `d1-2B` (**1.92B**, 128k), with typed decisions and 16 named languages. Newer 2B also advertises extraction/estimation, which is outside this contract. Vendor 3B latency: **8 ms RTX 4090 / 30 ms Apple M5 Pro**; 2B **5.7 ms H100 / 3.8 ms MI325X**, not matched measurements. Local weights and hosted access exist, but a current hosted price was not verified here. **LFM Open License v1.0**, not Apache: commercial rights have a $10M annual-revenue threshold; legal review before distribution. vLLM documents a Liquid strategy for **d1-3B**; do not assume the newer LFM3 2B works through that older strategy unchanged. [Q1][][Q3][][Q4][][V1][]
- **Fastino GLiDE:** verified current open-weight **27B**, System One Python/HTTP plus hosted `https://api.fastino.ai/v1/systemone`, **$0.04/M input**, output free. Card warm vendor median **112 ms** on RTX PRO 6000 96GB; ~52.2GB weights, 80GB+ GPU recommendation. Not a small desktop default. Community license requires commercial attribution and a separate agreement above $10M production commercial annual revenue; not Apache for Fastino modifications. Multilingual decision-calibration coverage not verified. API default **`store:true`** must be overridden to **false** for a privacy-first evaluation. Card returns score **argmax level** plus separate `expected_level`; our adapter must preserve that distinction rather than silently treating the integer as the expected score. Card local 40k default and hosted docs 65,536 budget also differ. No independent AudioGraph calibration evidence. [F1][][F2][][F3][]

## 6. OpenAI, Google and Anthropic: decision endpoints versus structured generation

| Service | Real current capability | Performance / cost / size / language / calibration | AudioGraph recommendation |
|---|---|---|---|
| **OpenAI Decisions, `gpt-6-luna`** | `POST /v1/decisions`; predicate, choice, score; **1–400 questions**, score **2–10** levels; text/JSON/images, no direct audio/file input. Probability arrays, distinct from `/v1/systemone`. | Vendor says about **10× faster than Responses**, no portable absolute p50/p95 verified. **$0.10/M uncached input**, no output charge; model caching rates **$0.01 read / $0.125 write**, with long-context/regional premiums. Size/weights not disclosed, hosted proprietary. Multilingual task inventory and route-calibration curves not established here. Its ordinary model page's **922k maximum input** is not proof that every Decisions request accepts that budget. [O1][][O2][][O3][] | Worth an authorized cloud comparison, **not** a generated-JSON workaround. Reuse `openai_api_key` with a distinct `decision.openai_*` descriptor/route. Preserve refusals, missing confidence, usage and actual served identity; no invented binary confidence. |
| **Google Decisions, `gemini-decision-1`** | `POST https://generativelanguage.googleapis.com/v1beta/decisions`; noul/choice/score, **1,000 questions**, **2–255 options/levels**, **262,144** context; text/images/audio/video/PDF. No extraction generation. | Announced **Nov 11**. Vendor **50–100 ms model compute / 150–250 ms end-to-end**, contexts up to 16k and nearly flat 1–100 questions; not a 256k latency promise. **$0.10/M input**, output 0; no caching/batch/flex/priority. Proprietary; size and comprehensive language quality not disclosed. Vendor calibrated-probability claim, not measured AudioGraph calibration. Noul is clipped to **[0.01,0.99]**, so a >0.99 threshold never fires. [G1][][G2][][G3][] | Most compelling new hosted multi-question comparator. Reuse `gemini_api_key`; separate from Gemini Live and generation. Start with public **text-only** fixtures, not session PCM. Choice keys are sorted UTF-8, ties alphabetic; test option permutation. Free-tier content may improve Google products whereas paid terms differ: never use free-tier availability as privacy approval. [G2][][G4][] |
| **Anthropic Claude Haiku 5.5 structured Messages** | Real schema-constrained `/v1/messages`, `output_config.format` JSON schema or strict tools. Can generate classification, scoring and extraction JSON, but no equivalent dedicated typed-probability Decisions API verified in the official API/structured-output sources searched. | Released **Oct 7**, **1M context**; proprietary, parameters undisclosed. **$0.10 input / $0.50 output** up to 100k prompt, **$0.50/$2.50** above. “Fastest” in its lineup, not a measured decision latency. Multilingual classification calibration not established. A generated `confidence:0.97` is text content, not classifier probability. [A1][][A2][] | Useful offline rubric/label-assistance comparator with independent human adjudication; **not** a calibrated `DecisionEngine` merely because JSON matches `Answer`. No Anthropic key slot is currently in AudioGraph's allowlist; direct integration needs a new scoped slot and descriptor. [R10][] |
| **Ordinary OpenAI/Gemini structured generation** | Real JSON-schema output: OpenAI Responses/Chat, Gemini generation/Interactions. These can classify/extract but generate text and can refuse or truncate. | Example verified OpenAI `gpt-5.4-nano` **$0.20 input / $1.25 output**, now marked **Deprecated** in its current model page; not the recommended new baseline. Parameters undisclosed. Use actual tokenizer logprobs only where the exact endpoint/model documents them; an enum guarantee is not a probability guarantee. [O4][][G5][] | Prefer the genuine Decisions endpoints when testing typed decisions. Do not turn token likelihood of JSON punctuation or model-written confidence into calibrated class probability. |

Cloud “supports ZDR” is not “this request used ZDR.” OpenAI describes eligibility/agreement requirements; Cloudflare's model pages advertise zero retention; Baseten cards state default ZDR. Verify applicable service configuration, subprocessors, retention, region and legal terms before private content. No authenticated request, paid inference, credential access or private-session export was performed for this report. [O1][][C1][][C2][][B6][][R6][]

## 7. Ranked evaluation plan for AudioGraph

This is an **engineering-priority ranking [INFERENCE]**, not a measured accuracy leaderboard. It weighs existing integration, local privacy, CPU/consumer-GPU feasibility and calibration risk.

1. **Current classic Laya + D3 edge/lite, against heuristics.** Lowest integration uncertainty; Laya is the CPU-oriented starting point, D3 edge/lite the existing CUDA-oriented starting point. Include real multilingual/ASR-error cases, Latin-script routing cases, long/fragmented spans, out-of-scope states and intentional ambiguity. Existing CPU/Metal/DirectML constraints remain; do not make the RTX 5090 the default user machine. Reuse `audio-graph-f284`, `1f5c`, `7e3d`, `377a`, `26b4`, not duplicate tasks. [R7][][R8][][R5][]
2. **Joey-1 (2B) versus D3 nano/mini**, initially a pinned local server if a compatible audited runtime is available, otherwise public-fixture Baseten shared API comparison. It is the clearest new small decision-specific checkpoint. No claim that it is already faster/better on meetings, or that its block-prefix architecture works in today's D3 native engine. [B6][][B10][]
3. **Tiny supervised Ettin/ModernBERT and GLiClass Instruct edge/base.** Stable meeting-label taxonomies can justify specialized encoders; GLiClass tests dynamic-label generalization. Benchmark CPU p95, peak RSS and per-language risk before spending effort on a new native backend. Compare with the existing Laya fine-tuning work, not a second independent dataset effort. [E1][][E7][][E8][][R3][]
4. **Existing native Clef-flash**, for installed high-memory GPUs and multi-question workloads. First finish the already-open post-review parity and quantization work. It is not a fresh provider implementation task. [R9][]
5. **Hosted comparison tier: Jev 1.14, Gemini Decision 1, Baseten Joey/Elephant/Bison, Cloudflare Clef-flash.** Jev has the least adapter work; Google brings a genuine new multi-question endpoint; Baseten adds small/quality/long-context controls. Hosted comparison order is not permission for automatic failover. Large Mammoth/Clef/GLiDE are optional offline quality controls, not mandatory hot-path providers. [J1][][G1][][B1][][C2][][F1][]
6. **Laya 0.5/Vega and Liquid d1 as bounded exploratory evaluations**, after checking pin/license/runtime and separating any learned/adaptive policy from route authority. Richer confidence/prediction-set outputs don't automatically satisfy ADR-0049. [L2][][Q1][][R3][]
7. **Clairic/GLiNER are an extraction track**, and NLI/rerankers are evidence/candidate-scoring tracks. Do not rank them as interchangeable whole-contract engines. [X2][][E4][][E5][][E9][]

### Common provider-addition requirements, applied to each family

The named decision route is the integration point, **not** a new React-side client or an entry in the text-generation fallback walker. [`route.rs`](../../src-tauri/src/decision/route.rs) currently has `HostedJev`, `HostedCloudflare`, `LocalHttp`, `NativeLaya`, `NativeD3`, reserved `NativeClef`; new hosted transports require explicit engine/egress handling, not masquerading as local vLLM. Every current route starts with an empty calibration list; the existing shadow posture is not permission to export private text. [R2][][R3][]

| Addition | Registry descriptor / credential decision | Runtime and evidence needed |
|---|---|---|
| Existing Jev/Cloudflare variants | Reuse existing `decision.jev_typesafe`, `decision.jev_cloudflare`, `decision.clef_flash_cloudflare` and slots `typesafe_api_key`, `cloudflare_workers_ai_token`; new Clef/omni routes need distinct model/provider metadata. | Check current model/version and cardinalities, explicit hosted admission, served identity, no-content readiness and content-write policy. Existing descriptors' generic/unknown retention metadata is not a completed privacy review. |
| Baseten shared/dedicated | Proposed separate `decision.baseten_*` descriptor(s), **new `baseten_api_key`** credential slot; dedicated endpoint identity distinct from shared `inference.baseten.co`. Do not overload `openai_api_key`. | System One auth/body adapter, 512-option aggregate limit, tier accounting, pinned identity, error/response fixtures, dedicated endpoint allowlisting and policy guard. |
| OpenAI / Google Decisions | Proposed `decision.openai_*`, `decision.gemini_*`; reuse existing **`openai_api_key` / `gemini_api_key`**, not realtime/LLM provider authorization. | Separate decision API translators, limits/key validation, response refusals/safety failures, honest requested/served identity and usage. Multimodal later needs additional data classes and consent. |
| Anthropic / Fastino / Liquid hosted | Proposed provider-specific descriptor and credential slot; none is currently an allowed named slot. Fastino must explicitly send `store:false`. | Do not enable generated-self-confidence as probability; verify actual endpoint/API before provider implementation. Preserve license/storage semantics and reject undocumented task shapes. |
| Native encoder/new open decision engine | Proposed `decision.<family>` `LocalOnly`, feature/model-manager requirement, **no credential slot**. Existing native descriptors need not be duplicated for a model-pin update. | Immutable revisions + per-file digests/licenses, exact tokenizer/formatter/readout, native parity, portable availability, no inference-time network dependencies. No runtime Python is introduced by this research. |
| Local HTTP experiment | Existing loopback descriptors may suffice for compatible server models; no key by default. Boundary remains **`UserConfiguredEndpoint`**, not proof that a server cannot relay. | Keep loopback literals/no proxies/no redirects/`:cloud` rejection; fix documented noul key mismatch; verify model/version/cardinality and no hidden cloud teacher. |
| Clairic / GLiNER extraction | Separate extraction-capability design, not a false `decision.*` drop-in. Cloudflare credential may be reused but **decision-route consent is not extraction/media consent**. | Define values/null/completeness/span offsets and source anchoring, truncation behavior and graph-validator admission before selectable runtime use. |

All **content-bearing hosted additions**, including shadow evaluation on private text, require the complete [Provider Addition Content-Egress Checklist](../designs/provider-architecture.md#provider-addition-content-egress-checklist): registry privacy data classes; no-content readiness; session command gate; low-level policy guard **before the first write**; blocked-policy fixture proving no content went out; payload/secret-free diagnostics; exact transport-specific notes. Declare `TranscriptText`, plus `Notes`/`GraphContext` when included; raw audio/media needs its own truthful metadata. Add producer/data-movement coverage and keep credentials in Rust's keychain-default facade with presence-only IPC and synchronized allowlists. Discovery/probe success does not authorize content. [R6][][R10][]

**Evaluation gate:** reuse `src-tauri/src/decision/eval.rs` for replay, but don't treat heuristic-agreement ECE as truth calibration. Use the human-labeled corpus already planned by `audio-graph-f284`, disjoint training/calibration/test sessions, per-option/language counts, Brier/NLL/ECE, risk-versus-coverage, precision at abstention threshold, warm/cold p50/p95, peak RAM/VRAM, total prompt tokens and actual billable usage. Pin served model, precision, tokenizer/template and schema version; preserve raw confidence. Existing `audio-graph-6345` already covers independent holdout authority. This report does not promote a route or change graph/notes admission. [R3][][R4][]

## 8. Uncertainties and checks deliberately not claimed

- Public pages move independently of model aliases, schemas and deployed implementations. Current Jev aliases, Clairic rollout and Laya 0.5 default changed after the original October work. Revalidate exact served identity at evaluation time. [J1][][X1][][L2][]
- A properly scored training objective is not proof of calibration on noisy ASR, code-switching, novel ontology labels or user-specific contexts. Accuracy/F1/leaderboard rank, ONNX parity and ECE are different measurements. No new quality or runtime inference measurements were run here.
- Vendor latency tables have different hardware, prompt lengths, batching, question counts and network boundaries. The report labels those boundaries instead of constructing a false global speed ranking.
- Baseten public cards, API availability and current pricing were checked; no account entitlement, deployment, regional availability or billing receipt was checked. No evidence established public Baseten Jev/Laya shared APIs.
- Clairic's current hosted docs retain contradictory preview/truncation prose; the November source explicitly establishes open weights and parallel autoregressive extraction. No historical-preview behavior is assumed for today's alias. [X1][][X4][][X5][]
- D3 refs are newer than AudioGraph's pins, but a complete artifact diff/weight-hash migration review was outside this research. Laya package version does not version an already exported ONNX artifact. No code/model downloads/credentials were changed.
- No build, tests, linter, formatter, authenticated model smoke or private-session replay was run. Verification here was source/contract inspection, primary release metadata, and read-only Seeds duplication checks. Integration verification remains with the owner.

## Sources

All web sources below were read on 2026-11-21. Vendor cards/blogs are **primary statements of their own results**, not independent endorsement. The only reported application measurements are explicitly attributed to the existing repository reports.

### Product/API primary sources

- [J1][] — [https://docs.typesafe.ai/models.md](https://docs.typesafe.ai/models.md)
- [J2][] — [https://docs.typesafe.ai/api/system-one.md](https://docs.typesafe.ai/api/system-one.md)
- [J3][] — [https://docs.typesafe.ai/changelog.md](https://docs.typesafe.ai/changelog.md)
- [C1][] — [https://developers.cloudflare.com/workers-ai/models/clef/](https://developers.cloudflare.com/workers-ai/models/clef/)
- [C2][] — [https://developers.cloudflare.com/workers-ai/models/clef-flash/](https://developers.cloudflare.com/workers-ai/models/clef-flash/)
- [C3][] — [https://developers.cloudflare.com/workers-ai/models/clef-omni/](https://developers.cloudflare.com/workers-ai/models/clef-omni/)
- [C4][] — [https://developers.cloudflare.com/ai/models/typesafe/jev/](https://developers.cloudflare.com/ai/models/typesafe/jev/)
- [C5][] — [https://blog.cloudflare.com/clef-decision-models/](https://blog.cloudflare.com/clef-decision-models/)
- [C6][] — [https://huggingface.co/Cloudflare/clef-omni](https://huggingface.co/Cloudflare/clef-omni)
- [C7][] — [https://developers.cloudflare.com/workers-ai/models/distilbert-sst-2-int8/](https://developers.cloudflare.com/workers-ai/models/distilbert-sst-2-int8/)
- [C8][] — [https://developers.cloudflare.com/workers-ai/models/bge-reranker-base/](https://developers.cloudflare.com/workers-ai/models/bge-reranker-base/)
- [C9][] — [https://developers.cloudflare.com/workers-ai/models/](https://developers.cloudflare.com/workers-ai/models/)
- [X1][] — [https://blog.cloudflare.com/open-clairic/](https://blog.cloudflare.com/open-clairic/)
- [X2][] — [https://huggingface.co/Cloudflare/clairic](https://huggingface.co/Cloudflare/clairic)
- [X3][] — [https://huggingface.co/Cloudflare/clairic-flash](https://huggingface.co/Cloudflare/clairic-flash)
- [X4][] — [https://developers.cloudflare.com/workers-ai/models/clairic/](https://developers.cloudflare.com/workers-ai/models/clairic/)
- [X5][] — [https://developers.cloudflare.com/workers-ai/models/clairic-flash/](https://developers.cloudflare.com/workers-ai/models/clairic-flash/)
- [B1][] — [https://docs.baseten.co/inference/model-apis/decision-models](https://docs.baseten.co/inference/model-apis/decision-models)
- [B2][] — [https://docs.baseten.co/reference/inference-api/system-one](https://docs.baseten.co/reference/inference-api/system-one)
- [B3][] — [https://www.baseten.co/pricing/](https://www.baseten.co/pricing/)
- [B4][] — [https://huggingface.co/baselabs/elephant-1](https://huggingface.co/baselabs/elephant-1)
- [B5][] — [https://huggingface.co/baselabs/mammoth-1](https://huggingface.co/baselabs/mammoth-1)
- [B6][] — [https://huggingface.co/baselabs/joey-1](https://huggingface.co/baselabs/joey-1)
- [B7][] — [https://www.baseten.co/library/d3/](https://www.baseten.co/library/d3/)
- [B8][] — [https://www.baseten.co/library/d3-mini/](https://www.baseten.co/library/d3-mini/)
- [B8a][] — [https://huggingface.co/baselabs/groundhog-1](https://huggingface.co/baselabs/groundhog-1)
- [B8b][] — [https://huggingface.co/baselabs/bison-1](https://huggingface.co/baselabs/bison-1)
- [B9][] — [https://docs.baseten.co/development/model/overview](https://docs.baseten.co/development/model/overview)
- [B10][] — [https://labs.baseten.co/articles/mammoth-1-and-elephant-1-frontier-decision-models](https://labs.baseten.co/articles/mammoth-1-and-elephant-1-frontier-decision-models)
- [O1][] — [https://developers.openai.com/api/docs/guides/decisions](https://developers.openai.com/api/docs/guides/decisions)
- [O2][] — [https://developers.openai.com/api/reference/resources/decisions/methods/create](https://developers.openai.com/api/reference/resources/decisions/methods/create)
- [O3][] — [https://developers.openai.com/api/docs/models/gpt-6-luna](https://developers.openai.com/api/docs/models/gpt-6-luna)
- [O4][] — [https://developers.openai.com/api/docs/models/gpt-5.4-nano](https://developers.openai.com/api/docs/models/gpt-5.4-nano)
- [G1][] — [https://blog.google/innovation-and-ai/technology/developers-tools/gemini-decisions-api/](https://blog.google/innovation-and-ai/technology/developers-tools/gemini-decisions-api/)
- [G2][] — [https://aistudio.google.com/docs/decisions-api](https://aistudio.google.com/docs/decisions-api)
- [G3][] — [https://aistudio.google.com/docs/models/gemini-decision-1](https://aistudio.google.com/docs/models/gemini-decision-1)
- [G4][] — [https://aistudio.google.com/docs/pricing](https://aistudio.google.com/docs/pricing)
- [G5][] — [https://ai.google.dev/gemini-api/docs/generate-content/structured-output](https://ai.google.dev/gemini-api/docs/generate-content/structured-output)
- [A1][] — [https://platform.claude.com/docs/en/build-with-claude/structured-outputs](https://platform.claude.com/docs/en/build-with-claude/structured-outputs)
- [A2][] — [https://platform.claude.com/docs/en/models/haiku-5-5/overview](https://platform.claude.com/docs/en/models/haiku-5-5/overview)

### Model/runtime primary sources

- [D1][] — [https://huggingface.co/vllm-sr/d3](https://huggingface.co/vllm-sr/d3)
- [D2][] — [https://huggingface.co/vllm-sr/d3-edge](https://huggingface.co/vllm-sr/d3-edge)
- [D3][] — [https://huggingface.co/vllm-sr/d3-lite](https://huggingface.co/vllm-sr/d3-lite)
- [D4][] — [https://huggingface.co/vllm-sr/d3-nano](https://huggingface.co/vllm-sr/d3-nano)
- [D5][] — [https://huggingface.co/vllm-sr/d3-mini](https://huggingface.co/vllm-sr/d3-mini)
- [D6][] — [https://huggingface.co/vllm-sr/d3-flash](https://huggingface.co/vllm-sr/d3-flash)
- [D7][] — [https://huggingface.co/api/models/vllm-sr/d3/refs](https://huggingface.co/api/models/vllm-sr/d3/refs)
- [D8][] — [https://huggingface.co/api/models/vllm-sr/d3-edge/refs](https://huggingface.co/api/models/vllm-sr/d3-edge/refs)
- [L1][] — [https://huggingface.co/convaiinnovations/laya](https://huggingface.co/convaiinnovations/laya)
- [L2][] — [https://pypi.org/project/laya/](https://pypi.org/project/laya/)
- [V1][] — [https://docs.vllm.ai/en/latest/models/structured_decisions_models/](https://docs.vllm.ai/en/latest/models/structured_decisions_models/)
- [V2][] — [https://docs.ollama.com/capabilities/decision](https://docs.ollama.com/capabilities/decision)
- [E1][] — [https://docs.knowledgator.com/docs/frameworks/gliclass/pretrained-models/](https://docs.knowledgator.com/docs/frameworks/gliclass/pretrained-models/)
- [E2][] — [https://huggingface.co/knowledgator/gliclass-instruct-edge-v1.0](https://huggingface.co/knowledgator/gliclass-instruct-edge-v1.0)
- [E3][] — [https://huggingface.co/MoritzLaurer/ModernBERT-base-zeroshot-v2.0](https://huggingface.co/MoritzLaurer/ModernBERT-base-zeroshot-v2.0)
- [E4][] — [https://huggingface.co/MoritzLaurer/mDeBERTa-v3-base-mnli-xnli](https://huggingface.co/MoritzLaurer/mDeBERTa-v3-base-mnli-xnli)
- [E5][] — [https://huggingface.co/urchade/gliner_multi-v2.1](https://huggingface.co/urchade/gliner_multi-v2.1)
- [E6][] — [https://huggingface.co/knowledgator/gliclass-multilang-mini](https://huggingface.co/knowledgator/gliclass-multilang-mini)
- [E7][] — [https://huggingface.co/answerdotai/ModernBERT-base](https://huggingface.co/answerdotai/ModernBERT-base)
- [E8][] — [https://huggingface.co/jhu-clsp/ettin-encoder-32m](https://huggingface.co/jhu-clsp/ettin-encoder-32m)
- [E9][] — [https://huggingface.co/BAAI/bge-reranker-v2-m3](https://huggingface.co/BAAI/bge-reranker-v2-m3)
- [Q1][] — [https://huggingface.co/LiquidAI/d1-2B](https://huggingface.co/LiquidAI/d1-2B)
- [Q2][] — [https://huggingface.co/Qwen/Qwen3.5-0.8B](https://huggingface.co/Qwen/Qwen3.5-0.8B)
- [Q3][] — [https://huggingface.co/LiquidAI/d1-3B](https://huggingface.co/LiquidAI/d1-3B)
- [Q4][] — [https://huggingface.co/LiquidAI/d1-2B/blob/main/LICENSE](https://huggingface.co/LiquidAI/d1-2B/blob/main/LICENSE)
- [F1][] — [https://huggingface.co/fastino/GLiDE](https://huggingface.co/fastino/GLiDE)
- [F2][] — [https://docs.fastino.ai/inference/systemone](https://docs.fastino.ai/inference/systemone)
- [F3][] — [https://huggingface.co/fastino/GLiDE/blob/main/LICENSE](https://huggingface.co/fastino/GLiDE/blob/main/LICENSE)

### Repository evidence

- [R1][] — [../../src-tauri/crates/decision-contract/src/lib.rs](../../src-tauri/crates/decision-contract/src/lib.rs)
- [R2][] — [../../src-tauri/src/decision/mod.rs](../../src-tauri/src/decision/mod.rs)
- [R3][] — [../../src-tauri/src/decision/route.rs](../../src-tauri/src/decision/route.rs)
- [R4][] — [system-one-eval-2026-10-18.md](system-one-eval-2026-10-18.md)
- [R5][] — [../../src-tauri/src/models/mod.rs](../../src-tauri/src/models/mod.rs)
- [R6][] — [../designs/provider-architecture.md](../designs/provider-architecture.md)
- [R7][] — [d3-native-latency-2026-10-18.md](d3-native-latency-2026-10-18.md)
- [R8][] — [../designs/d3-native-runtime-2026-10-18.md](../designs/d3-native-runtime-2026-10-18.md)
- [R9][] — [../designs/clef-flash-native-2026-10-18.md](../designs/clef-flash-native-2026-10-18.md)
- [R10][] — [../../src-tauri/src/credentials/mod.rs](../../src-tauri/src/credentials/mod.rs)

Policy rationale: [ADR-0049](../adr/0049-decision-route-layer.md). Native engine modules: [`decision-engine`](../../src-tauri/crates/decision-engine/src/lib.rs), [`laya`](../../src-tauri/crates/decision-engine/src/laya/mod.rs), [`d3`](../../src-tauri/crates/decision-engine/src/d3/mod.rs), [`clef`](../../src-tauri/crates/decision-engine/src/clef/mod.rs). Registry additions belong in [`provider-registry`](../../src-tauri/crates/provider-registry/src/lib.rs), not frontend provider branches.

[J1]: https://docs.typesafe.ai/models.md
[J2]: https://docs.typesafe.ai/api/system-one.md
[J3]: https://docs.typesafe.ai/changelog.md
[C1]: https://developers.cloudflare.com/workers-ai/models/clef/
[C2]: https://developers.cloudflare.com/workers-ai/models/clef-flash/
[C3]: https://developers.cloudflare.com/workers-ai/models/clef-omni/
[C4]: https://developers.cloudflare.com/ai/models/typesafe/jev/
[C5]: https://blog.cloudflare.com/clef-decision-models/
[C6]: https://huggingface.co/Cloudflare/clef-omni
[C7]: https://developers.cloudflare.com/workers-ai/models/distilbert-sst-2-int8/
[C8]: https://developers.cloudflare.com/workers-ai/models/bge-reranker-base/
[C9]: https://developers.cloudflare.com/workers-ai/models/
[X1]: https://blog.cloudflare.com/open-clairic/
[X2]: https://huggingface.co/Cloudflare/clairic
[X3]: https://huggingface.co/Cloudflare/clairic-flash
[X4]: https://developers.cloudflare.com/workers-ai/models/clairic/
[X5]: https://developers.cloudflare.com/workers-ai/models/clairic-flash/
[B1]: https://docs.baseten.co/inference/model-apis/decision-models
[B2]: https://docs.baseten.co/reference/inference-api/system-one
[B3]: https://www.baseten.co/pricing/
[B4]: https://huggingface.co/baselabs/elephant-1
[B5]: https://huggingface.co/baselabs/mammoth-1
[B6]: https://huggingface.co/baselabs/joey-1
[B7]: https://www.baseten.co/library/d3/
[B8]: https://www.baseten.co/library/d3-mini/
[B8a]: https://huggingface.co/baselabs/groundhog-1
[B8b]: https://huggingface.co/baselabs/bison-1
[B9]: https://docs.baseten.co/development/model/overview
[B10]: https://labs.baseten.co/articles/mammoth-1-and-elephant-1-frontier-decision-models
[O1]: https://developers.openai.com/api/docs/guides/decisions
[O2]: https://developers.openai.com/api/reference/resources/decisions/methods/create
[O3]: https://developers.openai.com/api/docs/models/gpt-6-luna
[O4]: https://developers.openai.com/api/docs/models/gpt-5.4-nano
[G1]: https://blog.google/innovation-and-ai/technology/developers-tools/gemini-decisions-api/
[G2]: https://aistudio.google.com/docs/decisions-api
[G3]: https://aistudio.google.com/docs/models/gemini-decision-1
[G4]: https://aistudio.google.com/docs/pricing
[G5]: https://ai.google.dev/gemini-api/docs/generate-content/structured-output
[A1]: https://platform.claude.com/docs/en/build-with-claude/structured-outputs
[A2]: https://platform.claude.com/docs/en/models/haiku-5-5/overview
[D1]: https://huggingface.co/vllm-sr/d3
[D2]: https://huggingface.co/vllm-sr/d3-edge
[D3]: https://huggingface.co/vllm-sr/d3-lite
[D4]: https://huggingface.co/vllm-sr/d3-nano
[D5]: https://huggingface.co/vllm-sr/d3-mini
[D6]: https://huggingface.co/vllm-sr/d3-flash
[D7]: https://huggingface.co/api/models/vllm-sr/d3/refs
[D8]: https://huggingface.co/api/models/vllm-sr/d3-edge/refs
[L1]: https://huggingface.co/convaiinnovations/laya
[L2]: https://pypi.org/project/laya/
[V1]: https://docs.vllm.ai/en/latest/models/structured_decisions_models/
[V2]: https://docs.ollama.com/capabilities/decision
[E1]: https://docs.knowledgator.com/docs/frameworks/gliclass/pretrained-models/
[E2]: https://huggingface.co/knowledgator/gliclass-instruct-edge-v1.0
[E3]: https://huggingface.co/MoritzLaurer/ModernBERT-base-zeroshot-v2.0
[E4]: https://huggingface.co/MoritzLaurer/mDeBERTa-v3-base-mnli-xnli
[E5]: https://huggingface.co/urchade/gliner_multi-v2.1
[E6]: https://huggingface.co/knowledgator/gliclass-multilang-mini
[E7]: https://huggingface.co/answerdotai/ModernBERT-base
[E8]: https://huggingface.co/jhu-clsp/ettin-encoder-32m
[E9]: https://huggingface.co/BAAI/bge-reranker-v2-m3
[Q1]: https://huggingface.co/LiquidAI/d1-2B
[Q2]: https://huggingface.co/Qwen/Qwen3.5-0.8B
[Q3]: https://huggingface.co/LiquidAI/d1-3B
[Q4]: https://huggingface.co/LiquidAI/d1-2B/blob/main/LICENSE
[F1]: https://huggingface.co/fastino/GLiDE
[F2]: https://docs.fastino.ai/inference/systemone
[F3]: https://huggingface.co/fastino/GLiDE/blob/main/LICENSE
[R1]: ../../src-tauri/crates/decision-contract/src/lib.rs
[R2]: ../../src-tauri/src/decision/mod.rs
[R3]: ../../src-tauri/src/decision/route.rs
[R4]: system-one-eval-2026-10-18.md
[R5]: ../../src-tauri/src/models/mod.rs
[R6]: ../designs/provider-architecture.md
[R7]: d3-native-latency-2026-10-18.md
[R8]: ../designs/d3-native-runtime-2026-10-18.md
[R9]: ../designs/clef-flash-native-2026-10-18.md
[R10]: ../../src-tauri/src/credentials/mod.rs
