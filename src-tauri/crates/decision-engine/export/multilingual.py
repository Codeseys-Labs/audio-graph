"""Development-only export of the pinned multilingual checkpoint. No Python at runtime.

Install export/requirements.txt, download the five source files at the revision
in multilingual-manifest.json, then run:
  python multilingual.py CHECKPOINT_DIR OUTPUT_DIR
  python multilingual.py CHECKPOINT_DIR OUTPUT_DIR --verify-only

The English runtime uses the published receptron bundle, not this exporter.
"""
import argparse
import hashlib
import importlib
import importlib.metadata
import json
import pathlib
import platform
import shutil


MANIFEST = pathlib.Path(__file__).with_name("multilingual-manifest.json")


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def verify_files(directory, expected):
    for name, digest in expected.items():
        if sha256(directory / name) != digest:
            raise SystemExit(f"SHA-256 mismatch: {name}")


def strip_export_metadata(message):
    # Stack traces contain checkout paths; they must not change the graph digest.
    for field, value in list(message.ListFields()):
        if field.name in ("doc_string", "metadata_props"):
            message.ClearField(field.name)
        elif field.message_type is not None:
            children = value if field.is_repeated else [value]
            for child in children:
                strip_export_metadata(child)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("checkpoint", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("--verify-only", action="store_true")
    parser.add_argument("--record-output-hashes", action="store_true", help="Record new output hashes for review instead of enforcing the committed ones")
    args = parser.parse_args()
    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    verify_files(args.checkpoint, manifest["inputs"])
    if args.verify_only:
        verify_files(args.output, manifest["outputs"])
        print("Pinned input and output SHA-256 verification passed")
        return
    if platform.python_version() != manifest["python"]:
        raise SystemExit(f"Use Python {manifest['python']} for the recorded export")
    for package, expected in manifest["packages"].items():
        if importlib.metadata.version(package) != expected:
            raise SystemExit(f"Use {package}=={expected} for the recorded export")

    torch = importlib.import_module("torch")
    onnx = importlib.import_module("onnx")
    common = importlib.import_module("laya.common")
    load_file = importlib.import_module("safetensors.torch").load_file
    torch.manual_seed(0)
    config = json.loads((args.checkpoint / "rl_agent_config.json").read_text(encoding="utf-8"))
    model = common.build_model(config, encoder_dir=str(args.checkpoint / "encoder"), pretrained=False)
    model.load_state_dict(load_file(str(args.checkpoint / "model.safetensors")), strict=True)
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
    args.output.mkdir(parents=True, exist_ok=True)
    batch, seq, options = torch.export.Dim("batch"), torch.export.Dim("seq", min=8), torch.export.Dim("options", min=2)
    program = torch.onnx.export(Wrapper(model).eval(), example, opset_version=18, dynamo=True, optimize=True,
        input_names=["input_ids", "attention_mask", "marker_pos", "marker_mask", "qtype"], output_names=["logits", "act_probs"],
        dynamic_shapes={"input_ids": {0: batch, 1: seq}, "attention_mask": {0: batch, 1: seq},
                        "marker_pos": {0: batch, 1: options}, "marker_mask": {0: batch, 1: options}, "qtype": {0: batch}})
    graph = args.output / "laya.onnx"
    program.save(str(graph), external_data=True)
    normalized = onnx.load(str(graph), load_external_data=False)
    strip_export_metadata(normalized)
    onnx.save(normalized, str(graph))
    shutil.copytree(args.checkpoint / "tokenizer", args.output / "tokenizer", dirs_exist_ok=True)
    (args.output / "laya_config.json").write_text(json.dumps({key: config[key] for key in ("max_len", "head_max_len", "temperature", "temperature_by_options")}, indent=2), encoding="utf-8")
    observed = {name: sha256(args.output / name) for name in manifest["outputs"]}
    provenance = {**manifest, "outputs": observed}
    (args.output / "export-provenance.json").write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(provenance, indent=2))
    if not args.record_output_hashes:
        verify_files(args.output, manifest["outputs"])
    print(f"Exported {args.output}")


if __name__ == "__main__":
    main()
