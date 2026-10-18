"""One-off fixture generation. Never imported or invoked by the Rust runtime.

Requires laya==0.3.20, torch, transformers, safetensors and numpy.
Download the convaiinnovations/laya checkpoint at revision
7b928d828b7b0e022f929d9bd2e44165aa270148 before running.
Run: python generate_reference.py REFERENCE_DIRECTORY OUTPUT_JSON [--multilingual]
"""
import importlib
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1]).resolve()
laya = importlib.import_module("laya")

multilingual = "--multilingual" in sys.argv[3:]
agent = laya.Agent(str(root / "multilingual" if multilingual else root), device="cpu", fast=False, compile=False)
questions = {
    "intent": {"type": "choice", "instructions": "Classify the customer's request.", "criteria": {"refund": "Return money", "shipping": "Track a delivery", "support": "Technical assistance"}},
    "urgency": {"type": "score", "instructions": "How urgent is the request?", "criteria": ["low", "medium", "high"]},
    "refund_requested": {"type": "noul", "instructions": "The customer explicitly requests a refund.", "criteria": None},
}
cases = []
states = ["包裹一直没有送达，请退还我的付款。", {"customer": "Приложение постоянно вылетает, но я могу подождать до следующей недели."}] if multilingual else ["The parcel never arrived. Please refund my payment.", {"customer": "My app keeps crashing, but I can wait until next week."}]
for state in states:
    for qs in [{"intent": questions["intent"]}, questions]:
        result = agent.system_one(state, qs)
        cases.append({"request": {"state": state, "questions": qs}, "reference": result})
output = {"source": "laya.Agent.system_one", "laya_version": laya.__version__, "checkpoint": "convaiinnovations/laya@7b928d828b7b0e022f929d9bd2e44165aa270148" + ("/multilingual" if multilingual else ""), "device": "cpu", "dtype": "float32", "tolerance": 0.001, "cases": cases}
destination = pathlib.Path(sys.argv[2])
destination.parent.mkdir(parents=True, exist_ok=True)
destination.write_text(json.dumps(output, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
print(json.dumps(output, ensure_ascii=False))
