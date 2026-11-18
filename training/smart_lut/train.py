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
from smartlut.styles import apply_grade, apply_look, random_grades, random_looks
from smartlut.transfer import transfer_answer
from smartlut.luts import apply_random_luts, load_luts, split_luts


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
    ap.add_argument(
        "--objective",
        choices=["strong", "luts", "transfer", "style"],
        default="strong",
        help="strong: real LUTs pushed up to 150%% and chained, strong colour grades (tints, duotones) and "
        "random looks (v4); luts: real film/creative LUTs on photo pairs (v3); transfer: take the reference's colour "
        "distribution (v2, abandoned); style: random synthetic looks (v1)",
    )
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
    if args.objective in ("luts", "strong"):
        names, luts = load_luts(Path(args.data) / "luts")
        train_luts, held_names, _ = split_luts(names, luts)
        train_luts = train_luts.to(device)
        print(f"{len(train_luts)} training LUTs, {len(held_names)} held out")
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
            others = photos.roll(1, 0)
            if args.objective == "strong":
                b = photos.shape[0]
                ref_l, gt_l = apply_random_luts(photos, others, train_luts, 0.8, 1.5, combine=0.25)
                grade = random_grades(b, device)
                ref_g, gt_g = apply_grade(others, grade), apply_grade(photos, grade)
                looks = random_looks(b).to(device)
                ref_s, gt_s = apply_look(others, looks), apply_look(photos, looks)
                kind = torch.rand(b, 1, 1, 1, device=device)
                use_l, use_g = (kind < 0.45).float(), ((kind >= 0.45) & (kind < 0.8)).float()
                use_s = 1 - use_l - use_g
                reference = ref_l * use_l + ref_g * use_g + ref_s * use_s
                gt = gt_l * use_l + gt_g * use_g + gt_s * use_s
            elif args.objective == "luts":
                reference, gt = apply_random_luts(photos, others, train_luts)
            elif args.objective == "style":
                looks = random_looks(photos.shape[0]).to(device)
                reference = apply_look(others, looks)
                gt = apply_look(photos, looks)
            else:
                # references: real photos, some with a random look (black and white, toning, grading…)
                looks = random_looks(photos.shape[0]).to(device)
                styled = (torch.rand(photos.shape[0], 1, 1, 1, device=device) < 0.4).float()
                reference = styled * apply_look(others, looks) + (1 - styled) * others
                gt = transfer_answer(photos, reference)
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
