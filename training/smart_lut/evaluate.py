"""Analytic engine (the app's current Smart LUT) vs the trained model, on the same pairs.

    .venv/bin/python evaluate.py --name v1                 held-out photos with random looks, scored
    .venv/bin/python evaluate.py --name v1 --pairs ref.jpg:photo.jpg other.jpg:photo2.jpg   real looks, side by side

Held-out mode: each pair gets a random look on photo A (the reference) and the same look on photo B (the
answer). Both engines turn plain B into B-with-the-look; the score is the mean OKLab distance (×100, about one
just-noticeable step) from the answer, lower is better. The analytic side runs the app's own Rust code
(`visual_check` in src-tauri/src/adjustments/smart_lut/compile.rs). Writes eval/<name>/summary.json and grids."""

import argparse
import json
import os
import subprocess
from pathlib import Path

import torch
from PIL import Image
from tqdm import tqdm

from smartlut.color import linear_to_oklab, srgb_to_linear
from smartlut.data import load, split
from smartlut.model import LutNet, apply_lut
from smartlut.styles import apply_look, random_looks

SRC_TAURI = Path(__file__).resolve().parents[2] / "src-tauri"
EVAL_SIZE = 512


def save(t: torch.Tensor, path: Path):
    arr = (t.clamp(0, 1) * 255).round().byte().permute(1, 2, 0).cpu().numpy()
    Image.fromarray(arr).save(path)


def analytic(reference: Path, target: Path, out: Path) -> torch.Tensor:
    # cargo runs in src-tauri, so the paths must be absolute
    env = dict(
        os.environ,
        SMART_REF=str(reference.resolve()),
        SMART_TARGET=str(target.resolve()),
        SMART_OUT=str(out.resolve()),
    )
    subprocess.run(
        ["cargo", "test", "--quiet", "--lib", "visual_check", "--", "--ignored"],
        cwd=SRC_TAURI, env=env, check=True, capture_output=True,
    )
    return load(out)


def neural(model, reference: torch.Tensor, target: torch.Tensor, device) -> torch.Tensor:
    with torch.no_grad():
        lut = model(reference[None].to(device), target[None].to(device))
        return apply_lut(target[None].to(device), lut)[0].cpu()


def distance(a: torch.Tensor, b: torch.Tensor) -> float:
    lab = lambda x: linear_to_oklab(srgb_to_linear(x))  # noqa: E731
    return float((lab(a) - lab(b)).norm(dim=0).mean() * 100)


def fit(t: torch.Tensor, h: int) -> torch.Tensor:
    return torch.nn.functional.interpolate(t[None], size=(h, int(t.shape[2] * h / t.shape[1])), mode="area")[0]


def grid(rows: list[list[torch.Tensor]], path: Path, h: int = 160):
    rows = [torch.cat([fit(t, h) for t in row], 2) for row in rows]
    width = max(r.shape[2] for r in rows)
    rows = [torch.nn.functional.pad(r, (0, width - r.shape[2])) for r in rows]
    save(torch.cat(rows, 1), path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", default="v1")
    ap.add_argument("--data", default="data")
    ap.add_argument("--count", type=int, default=100)
    ap.add_argument("--pairs", nargs="*", help="reference:target image pairs (real looks, no score)")
    args = ap.parse_args()

    device = "cuda" if torch.cuda.is_available() else "cpu"
    model = LutNet().to(device)
    model.load_state_dict(torch.load(Path("runs") / args.name / "model.pt", map_location=device)["model"])
    model.eval()
    out_dir = Path("eval") / args.name
    out_dir.mkdir(parents=True, exist_ok=True)

    if args.pairs:
        rows = []
        for i, pair in enumerate(args.pairs):
            ref_path, tgt_path = (Path(p).resolve() for p in pair.split(":"))
            ref, tgt = load(ref_path, EVAL_SIZE), load(tgt_path, EVAL_SIZE)
            save(tgt, out_dir / f"real{i}_target.png")
            a = analytic(ref_path, out_dir / f"real{i}_target.png", out_dir / f"real{i}_analytic.png")
            n = neural(model, ref, tgt, device)
            save(n, out_dir / f"real{i}_neural.png")
            rows.append([ref, tgt, a, n])
        grid(rows, out_dir / "real_pairs.jpg")
        print(f"reference | target | analytic | neural → {out_dir / 'real_pairs.jpg'}")
        return

    _, holdout = split(args.data)
    gen = torch.Generator().manual_seed(7)
    scores = {"identity": [], "analytic": [], "neural": []}
    rows = []
    for i in tqdm(range(args.count)):
        a_path, b_path = holdout[(2 * i) % len(holdout)], holdout[(2 * i + 1) % len(holdout)]
        a, b = load(a_path, EVAL_SIZE), load(b_path, EVAL_SIZE)
        look = random_looks(1, gen)
        reference = apply_look(a[None], look)[0]
        answer = apply_look(b[None], look)[0]
        ref_png, tgt_png = out_dir / f"{i:03d}_reference.png", out_dir / f"{i:03d}_target.png"
        save(reference, ref_png)
        save(b, tgt_png)
        an = analytic(ref_png, tgt_png, out_dir / f"{i:03d}_analytic.png")
        ne = neural(model, reference, b, device)
        scores["identity"].append(distance(b, answer))
        scores["analytic"].append(distance(an, answer))
        scores["neural"].append(distance(ne, answer))
        if i < 10:
            rows.append([reference, b, answer, an, ne])

    summary = {k: sum(v) / len(v) for k, v in scores.items()}
    summary["neural_better_on"] = sum(n < a for n, a in zip(scores["neural"], scores["analytic"])) / args.count
    json.dump({"summary": summary, "scores": scores}, open(out_dir / "summary.json", "w"), indent=2)
    grid(rows, out_dir / "heldout.jpg")
    print(json.dumps(summary, indent=2))
    print(f"reference | target | answer | analytic | neural → {out_dir / 'heldout.jpg'}")


if __name__ == "__main__":
    main()
