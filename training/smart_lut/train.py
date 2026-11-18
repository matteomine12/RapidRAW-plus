"""Train LutNet on random looks.

    .venv/bin/python train.py --data data --iters 60000 --name v1

Each step takes a batch of photos, gives every pair (A, B) the same random look, and asks the model for a LUT
that turns plain B into B-with-the-look, given A-with-the-look as the reference."""

import argparse
import json
import math
import time
from pathlib import Path

import torch
from torch.utils.data import DataLoader
from tqdm import tqdm

from smartlut.color import linear_to_oklab, srgb_to_linear
from smartlut.data import Photos, split
from smartlut.model import LutNet, apply_lut, lut_regularisers
from smartlut.styles import apply_look, random_looks


def loss_fn(out, gt):
    lab = lambda x: linear_to_oklab(srgb_to_linear(x))  # noqa: E731
    return (lab(out) - lab(gt)).abs().mean() + 0.5 * (out - gt).abs().mean()


def endless(loader):
    while True:
        yield from loader


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", default="data")
    ap.add_argument("--name", default="v1")
    ap.add_argument("--iters", type=int, default=60000)
    ap.add_argument("--batch", type=int, default=24)
    ap.add_argument("--lr", type=float, default=3e-4)
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--resume", action="store_true")
    args = ap.parse_args()

    device = "cuda" if torch.cuda.is_available() else "cpu"
    run = Path("runs") / args.name
    run.mkdir(parents=True, exist_ok=True)
    train_paths, holdout = split(args.data)
    print(f"{len(train_paths)} training photos, {len(holdout)} held out, device {device}")

    loader = DataLoader(
        Photos(train_paths), batch_size=args.batch, shuffle=True, num_workers=args.workers,
        drop_last=True, pin_memory=True, persistent_workers=args.workers > 0,
    )
    model = LutNet().to(device)
    opt = torch.optim.AdamW(model.parameters(), lr=args.lr, weight_decay=1e-4)
    sched = torch.optim.lr_scheduler.LambdaLR(
        opt, lambda i: min(1.0, i / 1000) * 0.5 * (1 + math.cos(math.pi * min(i, args.iters) / args.iters))
    )
    start = 0
    ckpt = run / "model.pt"
    if args.resume and ckpt.exists():
        state = torch.load(ckpt, map_location=device)
        model.load_state_dict(state["model"])
        opt.load_state_dict(state["opt"])
        sched.load_state_dict(state["sched"])
        start = state["iter"]
    json.dump(vars(args), open(run / "args.json", "w"), indent=2)

    log = open(run / "log.txt", "a")
    batches = endless(loader)
    bar = tqdm(range(start, args.iters), initial=start, total=args.iters)
    t0, running = time.time(), None
    for it in bar:
        photos = next(batches).to(device, non_blocking=True)
        with torch.no_grad():
            looks = random_looks(photos.shape[0]).to(device)
            others = photos.roll(1, 0)
            reference = apply_look(others, looks)
            gt = apply_look(photos, looks)
        lut = model(reference, photos)
        out = apply_lut(photos, lut)
        tv, mono = lut_regularisers(lut)
        loss = loss_fn(out, gt) + 1e-4 * tv + 10 * mono
        opt.zero_grad(set_to_none=True)
        loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        opt.step()
        sched.step()

        running = loss.item() if running is None else 0.98 * running + 0.02 * loss.item()
        if it % 50 == 0:
            bar.set_postfix(loss=f"{running:.4f}", lr=f"{sched.get_last_lr()[0]:.1e}")
        if it % 1000 == 0 or it == args.iters - 1:
            print(f"{it}\t{running:.5f}\t{time.time() - t0:.0f}s", file=log, flush=True)
            torch.save({"model": model.state_dict(), "opt": opt.state_dict(), "sched": sched.state_dict(),
                        "iter": it + 1}, ckpt)
    print(f"done: {ckpt}")


if __name__ == "__main__":
    main()
