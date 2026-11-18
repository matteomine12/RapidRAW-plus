"""Export a trained LutNet to ONNX for the app (ort crate).

    .venv/bin/python export_onnx.py --name v1      → runs/v1/smart_lut_v1.onnx

Inputs: `reference` and `target`, (1, 3, 256, 256) sRGB 0..1. Output: `lut`, (33, 33, 33, 3) indexed
[b][g][r][channel], the same order as the app's SmartLutData (sRGB in, sRGB out)."""

import argparse
from pathlib import Path

import numpy as np
import onnxruntime as ort
import torch

from smartlut.model import LutNet


class Export(torch.nn.Module):
    def __init__(self, model):
        super().__init__()
        self.model = model

    def forward(self, reference, target):
        return self.model(reference, target)[0].permute(1, 2, 3, 0).clamp(0, 1)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", default="v1")
    args = ap.parse_args()
    run = Path("runs") / args.name
    model = LutNet()
    model.load_state_dict(torch.load(run / "model.pt", map_location="cpu")["model"])
    model.eval()
    wrapper = Export(model)
    ref, tgt = torch.rand(1, 3, 256, 256), torch.rand(1, 3, 256, 256)
    out_path = run / f"smart_lut_{args.name}.onnx"
    torch.onnx.export(wrapper, (ref, tgt), out_path, input_names=["reference", "target"], output_names=["lut"],
                      opset_version=17, dynamo=False)
    with torch.no_grad():
        expected = wrapper(ref, tgt).numpy()
    got = ort.InferenceSession(str(out_path)).run(None, {"reference": ref.numpy(), "target": tgt.numpy()})[0]
    print(f"{out_path}  max |torch - onnx| = {np.abs(expected - got).max():.2e}")


if __name__ == "__main__":
    main()
