"""Generate the d3 parity fixture once from the Hugging Face reference (fixtures only, never at runtime).

    python gen_fixtures.py <snapshot-dir> <out.json> [--model-name d3-edge] [--device cuda]

Environment used for the committed fixtures: Python 3.11, torch 2.11.0+cu128, transformers==5.17.0, no
flash-linear-attention (so transformers' PyTorch Gated DeltaNet reference runs), RTX 5090.

Per question the fixture records:
- ``prompt`` / ``input_ids``: the exact text ``d3_format.render`` produces and its token ids;
- ``official``: probabilities from ``AutoModel.from_pretrained(..., trust_remote_code=True).system_one()``
  (BF16 backbone, FP32 readout, default batch_size=8, the published product path);
- ``official_b1``: the same with batch_size=1 (one question per forward, no left padding);
- ``fp32``: the same runtime code path with the backbone cast to float32 (TF32 off), the numerics target
  for an FP32 port;
- ``fp32_causal``: float32, but through the stock causal Qwen3.5 forward (evidence that the
  bidirectional full-attention mask changes the answers).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import torch

LONG_STATE = " ".join(
    [
        "Meeting notes, quarterly platform review.",
        "Attendees: Priya (infra lead), Marco (payments), Aiko (mobile), Dev (support), Lena (security).",
        "Priya opened with the incident summary: on Tuesday the primary database failed over twice,",
        "each time for about four minutes, because a storage controller firmware update was rolled out",
        "without the staged canary that the runbook requires. Checkout errors peaked at 18 percent.",
        "Marco said payments retried most failed charges automatically but roughly 2,300 customers saw a",
        "double authorization hold that released within 48 hours; support received 640 tickets about it.",
        "Dev asked for a macro that explains the hold and a dashboard of open tickets by category.",
        "Aiko reported that the new offline mode for the mobile app is in beta with 4 percent of users,",
        "crash-free sessions are at 99.2 percent, and the remaining blocker is a sync conflict when two",
        "devices edit the same shopping list. Lena raised that the firmware rollout bypassed change review",
        "and asked for an audit of every production change made without approval in the last 90 days.",
        "Decisions: the firmware runbook gets a mandatory canary gate; support gets the macro by Friday;",
        "the audit starts Monday and Lena owns it; offline mode stays at 4 percent until the sync conflict",
        "is fixed. Open question: whether to compensate the affected customers with a discount code.",
        "Marco is against a blanket discount and prefers targeted outreach to customers who contacted",
        "support. Priya will draft the public incident report and circulate it before Thursday noon.",
    ]
    * 2
)

CASES = [
    {
        "id": "card-support",
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
        "id": "code-kind",
        "state": "Write a Python function that merges two sorted lists.",
        "questions": {
            "kind": {
                "type": "choice",
                "instructions": "What kind of work is this?",
                "criteria": {"code": "Writing or fixing code", "math": "Mathematics", "chat": "Anything else"},
            },
            "reasoning": {"type": "noul", "instructions": "Does answering this need multi-step reasoning?"},
        },
    },
    {
        "id": "json-state",
        "state": {
            "speaker": "Aiko",
            "utterance": "Can we push the launch to next Tuesday? QA found two blockers.",
            "meeting": {"topic": "release planning", "minutes_in": 12},
            "tags": ["release", "risk"],
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
        "id": "default-instructions",
        "state": "The sky is clear and the sun is shining over the beach.",
        "questions": {
            "weather": {
                "type": "choice",
                "criteria": {"sunny": "Clear sky", "rainy": "Rain falling", "snowy": "Snow on the ground"},
            },
            "outdoors": {"type": "noul"},
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
            "discount": {
                "type": "noul",
                "instructions": "Was a blanket discount for all customers agreed?",
            },
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
                        "Montreal", "Washington", "New York", "Chicago", "Boston", "London",
                        "Manchester", "Paris", "Lyon", "Berlin", "Munich", "Madrid", "Barcelona",
                        "Rome", "Milan", "Tokyo", "Osaka", "Seoul",
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
                "criteria": {
                    "fr+ja": "French and Japanese",
                    "en": "English only",
                    "de+zh": "German and Chinese",
                },
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
        "id": "json-descriptions",
        "state": ["turn 1: hello", "turn 2: I need to reset my password", "turn 3: thanks"],
        "questions": {
            "category": {
                "type": "choice",
                "instructions": {"task": "classify", "field": "category"},
                "criteria": {
                    "account": {"examples": ["password reset", "login"]},
                    "orders": {"examples": ["tracking", "returns"]},
                },
            },
            "binary_score": {
                "type": "score",
                "instructions": "Was the issue resolved?",
                "criteria": ["No", "Yes"],
            },
        },
    },
    {
        "id": "ten-levels",
        "state": "The talk was clear, well paced and the demo worked on the first try, but it ran five minutes over.",
        "questions": {
            "rating": {
                "type": "score",
                "instructions": "Rate the talk.",
                "criteria": [str(i) for i in range(1, 11)],
            },
            "single": {
                "type": "choice",
                "instructions": "Pick the only option.",
                "criteria": {"only": "The single option"},
            },
        },
    },
]


def probabilities_for(rt, prepared):
    out = {}
    for key in prepared.runnable:
        probs = rt.probabilities([prepared.sequences[key]], [len(prepared.questions[key].keys)])[0]
        out[key] = probs
    return out


def causal_probabilities(rt, prepared):
    encoder = rt.encoder
    rt.encoder = None
    try:
        return probabilities_for(rt, prepared)
    finally:
        rt.encoder = encoder


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("snapshot")
    ap.add_argument("out")
    ap.add_argument("--model-name", default="d3-edge")
    ap.add_argument("--device", default="cuda:0")
    args = ap.parse_args()

    import transformers
    from transformers import AutoModel

    torch.backends.cuda.matmul.allow_tf32 = False
    torch.backends.cudnn.allow_tf32 = False

    model = AutoModel.from_pretrained(args.snapshot, trust_remote_code=True, device=args.device)
    rt = model.runtime
    assert rt.fast is None, rt.fast_skipped

    official = {c["id"]: model.system_one(state=c["state"], questions=c["questions"]) for c in CASES}
    batch = rt.batch_size
    rt.batch_size = 1
    official_b1 = {c["id"]: model.system_one(state=c["state"], questions=c["questions"]) for c in CASES}
    rt.batch_size = batch

    prepared = {c["id"]: rt.prepare(c["state"], c["questions"]) for c in CASES}

    rt.backbone.to(torch.float32)
    fp32 = {cid: probabilities_for(rt, p) for cid, p in prepared.items()}
    fp32_causal = {cid: causal_probabilities(rt, p) for cid, p in prepared.items()}

    def probs_of(answer, question):
        kind = answer["type"]
        if kind == "noul":
            return [1.0 - answer["noul"], answer["noul"]]
        if kind == "score":
            return [answer["probabilities"][str(i)] for i in range(len(question["criteria"]))]
        return list(answer["probabilities"].values())

    cases = []
    for case in CASES:
        cid = case["id"]
        p = prepared[cid]
        questions = []
        for key, question in case["questions"].items():
            normalized = p.questions[key]
            questions.append(
                {
                    "id": key,
                    "kind": normalized.kind,
                    "prompt": rt.text(case["state"], normalized.rendered),
                    "input_ids": p.sequences[key],
                    "official": probs_of(official[cid]["answers"][key], question),
                    "official_b1": probs_of(official_b1[cid]["answers"][key], question),
                    "fp32": fp32[cid][key],
                    "fp32_causal": fp32_causal[cid][key],
                }
            )
        cases.append(
            {
                "id": cid,
                "request": {"state": case["state"], "questions": case["questions"]},
                "official_response": official[cid],
                "questions": questions,
            }
        )

    doc = {
        "model": args.model_name,
        "snapshot": Path(args.snapshot).name,
        "generator": {
            "transformers": transformers.__version__,
            "torch": torch.__version__,
            "device": torch.cuda.get_device_name(0) if args.device.startswith("cuda") else "cpu",
            "kernels": rt.kernels,
            "temperature": rt.temperature,
            "attention_mode": rt.attention_mode,
        },
        "codes": rt.codes,
        "token_ids": rt.token_ids,
        "cases": cases,
    }
    Path(args.out).write_text(json.dumps(doc, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {args.out}: {sum(len(c['questions']) for c in cases)} questions", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
