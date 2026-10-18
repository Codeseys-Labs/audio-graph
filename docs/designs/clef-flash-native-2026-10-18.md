# Clef-flash native runtime (audio-graph-4bc6)

Date: 2026-10-18. Seed: `audio-graph-4bc6` (S1-T5), epic `audio-graph-6dfb`.
Code: `src-tauri/crates/decision-engine/src/clef/` (feature `decision-clef`), on the shared Qwen3.5 encoder
`src-tauri/crates/decision-engine/src/qwen35/` from the d3 work (`docs/designs/d3-native-runtime-2026-10-18.md`).

## Decision

Run Clef-flash on the same candle 0.10.2 stack as d3: the `qwen35::Encoder` in causal mode, plus a candle
port of the checkpoint's `JointSchemaHead`. No new runtime, no conversion, no LoRA code: the release ships
the rank-256 LoRA already merged into the backbone weights (`model.safetensors.index.json` has no LoRA
tensors). Default precision on GPU is BF16 (about 14 GB of weights on the device); FP32 numerics are
available on the same BF16-stored weights at the same memory cost by upcasting each weight for its
matmul, which is how the parity run below reaches the FP32 reference on a 32 GB card.

Measured on the committed fixture (9 requests, 23 questions, 204 to 1,930 tokens):

- FP32 compute on CUDA matches the FP32 reference to at most 6.0e-7 per probability, against the 1e-3
  target; picks match on 23 of 23 questions. Prompt token ids, question spans, option spans and option
  order are identical to `encode_record` for every request.
- BF16 on CUDA differs from the published BF16 path by at most 6.4e-3; picks match on 23 of 23. The
  published BF16 path itself differs from the FP32 reference by up to 4.8e-3.

## What the checkpoint computes

Sources: `Cloudflare/clef-flash@8b2e5fd17c09fd49bc2880b805d5323ec7ff4ff3` (`joint_schema_model.py`,
`joint_head_config.json`, `joint_head.safetensors`, `config.json`, model card), and vLLM's
`ClefFlashStrategy` / `JointSchemaStrategy` (`vllm/entrypoints/openai/decisions/strategies.py`, main,
read 2026-10-18), which mirrors `encode_record` and takes the same prompt.

### Backbone

`Qwen3_5ForConditionalGeneration` (Qwen3.5-9B): 32 layers in `LLLF` order (24 Gated DeltaNet, 8 full
attention), hidden 4096, MLP 12288, 16 query / 4 KV heads of dim 256, linear attention 16 key / 32 value
heads of dim 128, untied `lm_head`. Text requests run the stock text model (`model.language_model`) with
`use_cache=False`: full attention is **causal** here, unlike d3. The head reads `last_hidden_state`, the
final-norm states of every position.

### Prompt

Every piece is tokenized on its own (no special-token insertion) and the ids are concatenated, so token
boundaries never cross a piece:

```text
<|im_start|>system\n{SYSTEM_PROMPT}<|im_end|>\n<|im_start|>user\nSTATE:\n
{state}
\n\nSCHEMA FIELDS:\n
per question i (request order):
  \nFIELD {i}\nID: {id}\nTYPE: {noul|choice|score}\nINSTRUCTION:   <- trailing space
  {instructions, or the question id when empty}                    <- question span
  \nALLOWED OPTIONS:\n
  per option j: OPTION {j}:  {"description":...,"option_id":...}    <- option span
                \n
  END FIELD\n
\n<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\nJOINT SCHEMA DECISIONS:
```

- `SYSTEM_PROMPT`: "Read the complete state and schema. Decide every field jointly. Each answer must be
  exactly one of that field's allowed options."
- State: strings verbatim (an empty state is empty, not a placeholder); any other JSON value as
  `json.dumps(ensure_ascii=False, separators=(",", ":"), sort_keys=True)`.
- Options: noul lists `true` then `false` with default descriptions "The proposition is true or the answer
  is yes." / "The proposition is false or the answer is no." unless the request gives them; choice lists
  its keys **sorted** (code point order), description omitted when null; score lists levels as
  `option_id` `"0".."n-1"` with the level text as description. Option semantics are compact sorted JSON.
- Budget: 16,384 tokens in total. The state is truncated from the end to fit; a schema that alone exceeds
  the budget is an error (vLLM: 400).
- vLLM rejects the request-level `instructions` field (fixed prompt); per-question instructions are part
  of the prompt.

### Joint schema head (122 M parameters, `width` 1024, 16 heads)

1. `hidden_norm` (LayerNorm 4096) over all positions; `memory = memory_projection(h)` (L x 1024);
   `global = h[-1]`; question vector = mean of `h` over the question span; option context = mean over the
   option span; option lexical vector = mean of the `lm_head` rows of the option's token ids.
2. Option queries = `option_context_projection(context) + option_lexical_projection(lexical) +
   option_question_projection(question)`, refined by 2 evidence-routing layers (pre-norm cross-attention
   from options to `memory`, then a GELU MLP).
3. Fields = `question_projection(question) + option_summary_norm(softmax-weighted option summary) +
   global_projection(global) + type_embedding(type)`, then 4 `TransformerDecoderLayer`s (norm-first,
   GELU): self-attention across the request's questions (this is the "joint" part) and cross-attention to
   `memory`; then `field_norm`.
4. Per option: `prior = exp(min(prior_logit_scale, ln 100)) * cos(lexical, question + global)`;
   `joint = exp(min(joint_logit_scale, ln 100)) * cos(field, option_norm(option)) +
   residual_scorer([field, option, field * option, |field - option|])`;
   `logit = prior + sigmoid(residual_gate) * joint`. Probabilities are the FP32 softmax per question.

One forward pass scores every option of every question. `systemone()` rounds probabilities to 4 decimals;
the native engine returns them unrounded, with `confidence` as the probability of the reported answer
(the orchestrator's native-engine decision).

## Implementation

- `qwen35` gains `LoadOptions::weights` (storage dtype separate from compute dtype) and a projection that
  upcasts its weight to the activation dtype per matmul. d3 keeps storage = compute; its parity is
  unchanged (re-run on CUDA, same maxima). The module and `pyjson` now build under `decision-d3` or
  `decision-clef`.
- `pyjson`: Python `json.dumps` text shared by d3 (default separators) and Clef (compact, sorted keys).
- `clef/format.rs`: `encode` (prompt, spans, option order, truncation) and `answer` (contract answer in
  request order).
- `clef/head.rs`: the head on candle ops (torch `MultiheadAttention` / `TransformerDecoderLayer` semantics,
  LayerNorm eps 1e-5, exact GELU); span means accumulate in FP32 like torch's BF16 mean.
- `clef/mod.rs`: `ClefModel::load` / `score`: the `lm_head` table stays on the host in BF16 (only the
  option-token rows are gathered), like the embedding table, so neither 2 GB vocabulary matrix uses VRAM.

### Fixture

`tools/generate_clef_reference.py`, run once (Python 3.11, torch 2.11.0+cu128, transformers 5.17.0, RTX
5090). `official` is `load_release_model` + `ClefModel` in BF16. `fp32` runs transformers' own
`Qwen3_5DecoderLayer`s one at a time in float32 (a float32 9B model does not fit next to anything else),
then the head in float32. Harness check: the same streamed path in BF16 reproduces `official` exactly
(max difference 0 on all 9 requests). Requests cover multi-question schemas, a JSON state with nested
objects and a float, an empty state, empty instructions, a 1,930-token transcript, 20 choice options,
French and Japanese text, a JSON list state with quotes and a newline, a single-option choice, and
2-, 3-, 5- and 10-level scores.

### Parity

`tests/clef_parity.rs` (`AUDIOGRAPH_CLEF_DIR`, `AUDIOGRAPH_CLEF_EP=cuda-f32|cuda|cpu`):

| Mode | Reference | max abs diff | Picks |
| --- | --- | ---: | --- |
| CUDA, FP32 compute, BF16 weights | `fp32` | 5.96e-7 | 23/23 |
| CUDA, BF16 | `official` | 6.44e-3 | 23/23 |

Warm latency through the engine is in the Phase B section below.

## Memory and quantization

| Component | Size | Where |
| --- | ---: | --- |
| Text layers, BF16 | 13.8 GB (6.9 B params) | GPU |
| Joint head | 0.24 GB BF16 / 0.49 GB FP32 | GPU |
| Embedding table, BF16 | 2.0 GB | host RAM |
| `lm_head`, BF16 | 2.0 GB | host RAM |
| Vision tower (0.4 B) | not loaded | mmap only |

The model card's "about 18 GB" counts the vocabulary matrices and the vision tower, which this runtime
keeps off the GPU or never reads. The 5090 (32 GB) runs BF16 with room for 16k-token prompts. A 16 GB
card is borderline for BF16 (13.8 GB of weights plus activations); 8 to 12 GB cards need quantized
weights. Community recipes serve Clef-flash in FP8 on 16 and 24 GB GPUs (`turlockmike/clef-flash-local`,
`cryguy/clef-flash-vllm-fp8`). The candle path would be its quantized matmul (Q8_0 about 7.4 GB, Q4K about
4 GB for the layers) with parity measured against `official`; that is follow-up seed `audio-graph-5678`.

## Phase B (as built)

- `ClefEngine` (`clef/mod.rs`) implements `DecisionEngine`; `clef::register(&mut EngineRegistry)` adds it
  through the registry API, and `create_decision_engine` in `src-tauri/src/models/mod.rs` calls it under
  the app's `decision-clef` feature. Without the feature the skeleton registry answers `clef-flash` with
  `EngineUnavailable { feature: "decision-clef" }`.
- `egress()` is `Local`. Limits: 64 questions (vLLM's limit), 255 options per choice or score, state
  budget 16,384 tokens. `EngineLimits::validate` runs before any model file is read. A schema that alone
  exceeds the budget returns `CardinalityLimit`; a long state is truncated as in the reference and vLLM,
  and `usage.input_tokens` counts the prompt actually read.
- Precision: BF16 on GPU by default; `ClefEngine::with_precision(Precision::F32)` selects FP32 compute
  on the BF16 weights. CPU always runs FP32 compute (candle's CPU matmul has no BF16 kernel). DirectML is
  refused with `EngineUnavailable { feature: "cuda" }`, as for d3; device selection is shared
  (`qwen35::device_for`).
- Model files: `ModelDef` `clef-flash` in the existing model manager, `resolve/8b2e5fd1...` with the
  config, tokenizer, joint head and all four weight shards (the vision tower shares the shards, so the
  download is the full 19 GB even though only the text layers load).
- GPU smoke and latency (`examples/clef_bench.rs`, details in
  `docs/research/clef-flash-native-latency-2026-10-18.md`), warm p50 / p95 on the RTX 5090:

  | Path | 3 questions, 335 tokens | 4 questions, 1,930 tokens | Device memory |
  | --- | --- | --- | ---: |
  | BF16 | 57.6 / 60.0 ms | 279 / 284 ms | 14.2 GB |
  | FP32 compute | 176 / 179 ms | 769 / 785 ms | 15.1 GB |
  | Reference `systemone()`, BF16 | 160 / 183 ms | 385 / 423 ms | 18.5 GB peak |

## Risks

- Same platform limits as d3: no Vulkan backend (`audio-graph-26b4`), Metal untested (`audio-graph-377a`).
- CPU: FP32 compute on the BF16 weights needs about 18 GB of RAM; not measured (this machine had 14 to
  20 GB free during the session). Tracked with the CPU work in `audio-graph-1f5c`.
- Images and video are out of scope (text only), as for d3.
