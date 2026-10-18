"""One-off ONNX export, based on receptron/laya/export/export_onnx.py.

python export_bundle.py CHECKPOINT_DIR OUTPUT_DIR
Python is a development dependency only. Runtime inference uses Rust ort.
"""
import importlib
import json
import pathlib
import shutil
import sys

import torch
from safetensors.torch import load_file

checkpoint, output = map(lambda s: pathlib.Path(s).resolve(), sys.argv[1:3])
common = importlib.import_module("laya.common")
config = json.loads((checkpoint / "rl_agent_config.json").read_text())
model = common.build_model(config, encoder_dir=str(checkpoint / "encoder"), pretrained=False)
model.load_state_dict(load_file(str(checkpoint / "model.safetensors")), strict=True)
model.eval()
model.encoder.config.reference_compile = False

class Wrapper(torch.nn.Module):
    def __init__(self, model):
        super().__init__()
        self.model = model

    def forward(self, input_ids, attention_mask, marker_pos, marker_mask, qtype):
        logits, act = self.model(input_ids, attention_mask, marker_pos, marker_mask, qtype)
        return logits, torch.softmax(act.float(), -1)

example = (torch.randint(5, 1000, (2, 40)), torch.ones(2, 40, dtype=torch.long),
           torch.tensor([[3, 9, 15, 21], [3, 9, 0, 0]]),
           torch.tensor([[True, True, True, True], [True, True, False, False]]), torch.tensor([0, 2]))
example[1][1, 30:] = 0
output.mkdir(parents=True, exist_ok=True)
wrapper = Wrapper(model)
batch, seq, options = torch.export.Dim("batch"), torch.export.Dim("seq", min=8), torch.export.Dim("options", min=2)
program = torch.onnx.export(wrapper, example, opset_version=18, dynamo=True, optimize=True,
    input_names=["input_ids", "attention_mask", "marker_pos", "marker_mask", "qtype"], output_names=["logits", "act_probs"],
    dynamic_shapes={"input_ids": {0: batch, 1: seq}, "attention_mask": {0: batch, 1: seq},
                    "marker_pos": {0: batch, 1: options}, "marker_mask": {0: batch, 1: options}, "qtype": {0: batch}})
program.save(str(output / "laya.onnx"), external_data=True)
shutil.copytree(checkpoint / "tokenizer", output / "tokenizer", dirs_exist_ok=True)
(output / "laya_config.json").write_text(json.dumps({key: config[key] for key in ("max_len", "head_max_len", "temperature", "temperature_by_options")}, indent=2))
print(f"Exported {output}")
