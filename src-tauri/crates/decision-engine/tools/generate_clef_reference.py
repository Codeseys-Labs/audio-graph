"""Generate the Clef-flash parity fixture once from the checkpoint's own Python code (fixtures only).

    python tools/generate_clef_reference.py <hf-snapshot-dir> fixtures/clef-flash.json

Environment used for the committed fixture: Python 3.11, torch 2.11.0+cu128, transformers==5.17.0, no
flash-linear-attention (transformers' PyTorch Gated DeltaNet runs), RTX 5090, Cloudflare/clef-flash
revision 8b2e5fd17c09fd49bc2880b805d5323ec7ff4ff3.

Per record the fixture holds the request, the token ids and spans ``joint_schema_model.encode_record``
produces, and per question (options in the model's internal order):

- ``official``: softmax of the logits from ``load_release_model`` + ``ClefModel`` (BF16 backbone and
  head, the published path; ``systemone`` rounds these to 4 decimals, the fixture keeps them unrounded);
- ``fp32``: the same computation in float32. A float32 9B backbone does not fit next to the BF16 one, so
  the text layers run one at a time (each layer loaded in float32, run, freed) with transformers' own
  ``Qwen3_5DecoderLayer``, rotary embedding, masks and final norm, then the joint head in float32. The
  same streamed path in BF16 is compared against ``official`` as a harness check (printed, not stored).
"""

from __future__ import annotations

import argparse
import gc
import importlib.util
import json
import sys
from pathlib import Path

import torch

LONG_STATE = {
    "meeting": "Quarterly platform review",
    "attendees": ["Priya", "Marco", "Aiko", "Dev", "Lena"],
    "transcript": " ".join(
        [
            "Priya opened with the incident summary: on Tuesday the primary database failed over twice,",
            "each time for about four minutes, because a storage controller firmware update was rolled out",
            "without the staged canary that the runbook requires. Checkout errors peaked at 18 percent.",
            "Marco said payments retried most failed charges automatically but roughly 2,300 customers saw a",
            "double authorization hold that released within 48 hours; support received 640 tickets about it.",
            "Dev asked for a macro that explains the hold and a dashboard of open tickets by category.",
            "Aiko reported that offline mode for the mobile app is in beta with 4 percent of users and the",
            "remaining blocker is a sync conflict when two devices edit the same shopping list. Lena raised",
            "that the firmware rollout bypassed change review and asked for an audit of every production",
            "change made without approval in the last 90 days. Decisions: the firmware runbook gets a",
            "mandatory canary gate; support gets the macro by Friday; the audit starts Monday and Lena owns",
            "it; offline mode stays at 4 percent until the sync conflict is fixed. Marco is against a blanket",
            "discount and prefers targeted outreach to customers who contacted support.",
        ]
        * 6
    ),
}

CASES = [
    {
        "id": "support-ticket",
        "state": "The order arrived damaged yesterday. The customer has a receipt and asks for a replacement today.",
        "questions": {
            "route": {
                "type": "choice",
                "instructions": "Which team should handle this request?",
                "criteria": {
                    "returns": "Refunds, replacements and damaged deliveries",
                    "billing": "Payments, invoices and charges",
                    "technical": "Product setup and faults",
                },
            },
            "receipt": {"type": "noul", "instructions": "Does the customer have a receipt?"},
            "urgency": {
                "type": "score",
                "instructions": "How urgent is this request?",
                "criteria": ["Routine", "Soon", "Today"],
            },
        },
    },
    {
        "id": "json-utterance",
        "state": {
            "speaker": "Aiko",
            "utterance": "Can we push the launch to next Tuesday? QA found two blockers.",
            "meeting": {"topic": "release planning", "minutes_in": 12, "ratio": 0.25},
            "tags": ["release", "risk"],
            "flagged": False,
        },
        "questions": {
            "intent": {
                "type": "choice",
                "instructions": "What is the speaker doing?",
                "criteria": {
                    "request": "Asking for a change or action",
                    "inform": "Sharing information only",
                    "decide": "Announcing a decision",
                    "social": None,
                },
            },
            "action_item": {
                "type": "noul",
                "instructions": "Should this become an action item?",
                "criteria": {"true": "Someone must follow up", "false": "No follow-up needed"},
            },
            "risk": {
                "type": "score",
                "instructions": "How risky is the release right now?",
                "criteria": ["None", "Low", "Medium", "High", "Critical"],
            },
        },
    },
    {
        "id": "empty-state",
        "state": "",
        "questions": {
            "weekday": {
                "type": "choice",
                "instructions": "Which day comes after Monday?",
                "criteria": {"Sunday": None, "Tuesday": None, "Friday": None},
            },
            "only_true": {
                "type": "noul",
                "instructions": "Is water wet?",
                "criteria": {"true": "Yes, it is"},
            },
        },
    },
    {
        "id": "missing-instructions",
        "state": "The sky is clear and the sun is shining over the beach.",
        "questions": {
            "weather": {
                "type": "choice",
                "instructions": "",
                "criteria": {"sunny": "Clear sky", "rainy": "Rain falling", "snowy": "Snow on the ground"},
            },
            "outdoors": {"type": "noul", "instructions": ""},
        },
    },
    {
        "id": "long-meeting",
        "state": LONG_STATE,
        "questions": {
            "owner": {
                "type": "choice",
                "instructions": "Who owns the change audit?",
                "criteria": {"Priya": None, "Marco": None, "Aiko": None, "Dev": None, "Lena": None},
            },
            "discount": {"type": "noul", "instructions": "Was a blanket discount for all customers agreed?"},
            "severity": {
                "type": "score",
                "instructions": "How severe was the incident for customers?",
                "criteria": ["Negligible", "Minor", "Moderate", "Major", "Severe"],
            },
            "topic": {
                "type": "choice",
                "instructions": "What was the main topic of the meeting?",
                "criteria": {
                    "incident": "A production incident and its follow-ups",
                    "hiring": "Hiring and team changes",
                    "roadmap": "Long-term product roadmap",
                    "budget": "Budget and spending",
                },
            },
        },
    },
    {
        "id": "many-options",
        "state": "The capital of Australia is a planned city chosen as a compromise between Sydney and Melbourne.",
        "questions": {
            "city": {
                "type": "choice",
                "instructions": "Which city is described?",
                "criteria": {
                    name: None
                    for name in [
                        "Sydney", "Melbourne", "Brisbane", "Perth", "Adelaide", "Hobart", "Darwin",
                        "Canberra", "Auckland", "Wellington", "Ottawa", "Toronto", "Vancouver",
                        "Montreal", "Washington", "New York", "Chicago", "Boston", "London", "Paris",
                    ]
                },
            }
        },
    },
    {
        "id": "unicode",
        "state": "Le client écrit : « Ma commande n'est jamais arrivée, je veux être remboursé. » 顧客は返金を求めています。",
        "questions": {
            "language": {
                "type": "choice",
                "instructions": "Which languages appear in the state?",
                "criteria": {"fr+ja": "French and Japanese", "en": "English only", "de+zh": "German and Chinese"},
            },
            "refund": {"type": "noul", "instructions": "Does the customer want a refund?"},
            "sentiment": {
                "type": "score",
                "instructions": "How upset is the customer?",
                "criteria": ["Calm", "Annoyed", "Angry"],
            },
        },
    },
    {
        "id": "json-list-state",
        "state": ["turn 1: hello", "turn 2: I need to reset my password", "turn 3: thanks \"bye\"\n"],
        "questions": {
            "category": {
                "type": "choice",
                "instructions": "Classify the conversation.",
                "criteria": {"orders": "Tracking or returns", "account": "Password reset or login", "zz-other": None},
            },
            "resolved": {"type": "score", "instructions": "Was the issue resolved?", "criteria": ["No", "Yes"]},
            "single": {"type": "choice", "instructions": "Pick the only option.", "criteria": {"only": None}},
        },
    },
    {
        "id": "ten-levels",
        "state": "The talk was clear, well paced and the demo worked on the first try, but it ran five minutes over.",
        "questions": {
            "rating": {"type": "score", "instructions": "Rate the talk.", "criteria": [str(i) for i in range(1, 11)]},
            "overtime": {"type": "noul", "instructions": "Did the talk run over time?"},
        },
    },
]


def load_module(path: Path):
    spec = importlib.util.spec_from_file_location("joint_schema_model", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules["joint_schema_model"] = module
    spec.loader.exec_module(module)
    return module


def probabilities(logits):
    return [q.float().softmax(-1).tolist() for q in logits]


@torch.inference_mode()
def streamed_hidden(root: Path, text_config, input_ids: list[int], dtype: torch.dtype, device: str):
    """Final-norm hidden states of the text model, one decoder layer resident at a time."""
    from safetensors import safe_open
    from transformers.masking_utils import create_causal_mask, create_recurrent_attention_mask
    from transformers.models.qwen3_5.modeling_qwen3_5 import (
        Qwen3_5DecoderLayer,
        Qwen3_5RMSNorm,
        Qwen3_5TextRotaryEmbedding,
    )

    index = json.loads((root / "model.safetensors.index.json").read_text())["weight_map"]
    handles = {}

    def tensor(name):
        file = index[name]
        if file not in handles:
            handles[file] = safe_open(str(root / file), framework="pt", device="cpu")
        return handles[file].get_tensor(name)

    prefix = "model.language_model."
    ids = torch.tensor([input_ids], dtype=torch.long)
    embed = tensor(prefix + "embed_tokens.weight")
    hidden = embed[ids].to(device=device, dtype=dtype)
    del embed
    length = ids.shape[1]
    position_ids = torch.arange(length, device=device).view(1, 1, -1).expand(4, 1, -1)
    text_positions, position_ids = position_ids[0], position_ids[1:]
    mask_kwargs = {
        "config": text_config,
        "inputs_embeds": hidden,
        "attention_mask": torch.ones((1, length), dtype=torch.long, device=device),
        "past_key_values": None,
        "position_ids": text_positions,
    }
    masks = {
        "full_attention": create_causal_mask(**mask_kwargs),
        "linear_attention": create_recurrent_attention_mask(**mask_kwargs),
    }
    rotary = Qwen3_5TextRotaryEmbedding(config=text_config).to(device)
    rope = rotary(hidden, position_ids)
    for i in range(text_config.num_hidden_layers):
        layer = Qwen3_5DecoderLayer(text_config, i)
        layer_prefix = f"{prefix}layers.{i}."
        state = {k[len(layer_prefix):]: tensor(k).to(dtype) for k in index if k.startswith(layer_prefix)}
        layer.load_state_dict(state, strict=True)
        layer = layer.to(device=device, dtype=dtype).eval()
        hidden = layer(
            hidden,
            position_embeddings=rope,
            attention_mask=masks[text_config.layer_types[i]],
            position_ids=text_positions,
            past_key_values=None,
            use_cache=False,
        )
        del layer, state
        if device.startswith("cuda"):
            torch.cuda.empty_cache()
    norm = Qwen3_5RMSNorm(text_config.hidden_size, eps=text_config.rms_norm_eps)
    norm.load_state_dict({"weight": tensor(prefix + "norm.weight").to(dtype)})
    return norm.to(device=device, dtype=dtype)(hidden)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("snapshot")
    ap.add_argument("out")
    ap.add_argument("--device", default="cuda:0")
    args = ap.parse_args()
    root = Path(args.snapshot)

    import transformers
    from safetensors.torch import load_file
    from transformers import AutoConfig

    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False
    clef = load_module(root / "joint_schema_model.py")

    model, processor = clef.load_release_model(root, device=args.device, dtype=torch.bfloat16)
    tokenizer = processor.tokenizer
    encoded = {}
    official = {}
    responses = {}
    for case in CASES:
        request = {"model": "Cloudflare/clef-flash", "state": case["state"], "questions": case["questions"]}
        record = clef.encode_record(tokenizer, request, processor=processor)
        batch = clef.collate_records([record], tokenizer.pad_token_id, torch.device(args.device))
        with torch.inference_mode():
            official[case["id"]] = probabilities(model(batch)[0])
            responses[case["id"]] = clef.systemone(model, processor, request)
        encoded[case["id"]] = record
    lm_head = model.language_model.get_output_embeddings().weight.detach().to("cpu", torch.float32)
    del model
    gc.collect()
    torch.cuda.empty_cache()

    text_config = AutoConfig.from_pretrained(root).text_config
    text_config._attn_implementation = "sdpa"
    head_config = json.loads((root / "joint_head_config.json").read_text())
    head_weights = load_file(str(root / "joint_head.safetensors"))

    def run_head(record, hidden, dtype):
        head = clef.JointSchemaHead(**head_config)
        head.load_state_dict(head_weights, strict=True)
        head = head.to(device=args.device, dtype=dtype).eval()
        ids = torch.tensor([record.input_ids], device=args.device)
        mask = torch.ones_like(ids)
        with torch.inference_mode():
            logits = head(hidden, ids, mask, [record], lm_head.to(args.device, dtype))
        return probabilities(logits[0])

    fp32 = {}
    for case in CASES:
        record = encoded[case["id"]]
        bf16_hidden = streamed_hidden(root, text_config, list(record.input_ids), torch.bfloat16, args.device)
        streamed = run_head(record, bf16_hidden, torch.bfloat16)
        harness = max(
            abs(a - b) for x, y in zip(streamed, official[case["id"]]) for a, b in zip(x, y)
        )
        hidden = streamed_hidden(root, text_config, list(record.input_ids), torch.float32, args.device)
        fp32[case["id"]] = run_head(record, hidden, torch.float32)
        print(f"{case['id']}: {len(record.input_ids)} tokens; streamed bf16 vs official max|d| {harness:.3e}", file=sys.stderr)

    cases = []
    for case in CASES:
        record = encoded[case["id"]]
        cases.append(
            {
                "id": case["id"],
                "request": {"state": case["state"], "questions": case["questions"]},
                "input_ids": list(record.input_ids),
                "official_response": responses[case["id"]],
                "questions": [
                    {
                        "id": q.question_id,
                        "question_span": list(q.question_span),
                        "option_spans": [list(s) for s in q.option_spans],
                        "option_ids": list(q.option_ids),
                        "official": official[case["id"]][i],
                        "fp32": fp32[case["id"]][i],
                    }
                    for i, q in enumerate(record.questions)
                ],
            }
        )
    doc = {
        "model": "Cloudflare/clef-flash",
        "revision": root.name,
        "generator": {
            "transformers": transformers.__version__,
            "torch": torch.__version__,
            "device": torch.cuda.get_device_name(0) if args.device.startswith("cuda") else "cpu",
        },
        "cases": cases,
    }
    Path(args.out).write_text(json.dumps(doc, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {args.out}: {sum(len(c['questions']) for c in cases)} questions", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
