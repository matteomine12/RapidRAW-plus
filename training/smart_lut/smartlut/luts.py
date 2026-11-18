"""Real looks: HaldCLUT film simulations (RawTherapee/G'MIC collection, CC BY-SA), loaded as 33³ LUTs.

Every 10th LUT (sorted by name) is held out from training, to check the model on looks it never saw.
"""

from pathlib import Path

import numpy as np
import torch
import torch.nn.functional as F
from PIL import Image

from .model import LUT_SIZE, apply_lut, identity_lut

CACHE = "luts33.pt"


def _hald_to_lut(path: Path, size: int = LUT_SIZE) -> torch.Tensor:
    img = np.asarray(Image.open(path).convert("RGB"), dtype=np.float32) / 255
    entries = img.reshape(-1, 3)
    n = round(entries.shape[0] ** (1 / 3))
    lut = torch.from_numpy(entries.reshape(n, n, n, 3)).permute(3, 0, 1, 2)  # (3, b, g, r)
    grid = identity_lut(size).permute(1, 2, 3, 0)[None] * 2 - 1  # sample at the 33³ grid points
    return F.grid_sample(lut[None], grid, mode="bilinear", align_corners=True)[0]


def load_luts(root: str | Path = "data/luts") -> tuple[list[str], torch.Tensor]:
    """(names, LUTs (K, 3, 33, 33, 33)), cached after the first load."""
    root = Path(root)
    cache = root / CACHE
    if cache.exists():
        data = torch.load(cache)
        return data["names"], data["luts"]
    paths = sorted(
        p for p in root.rglob("*.png") if "negative" not in p.name.lower() and Image.open(p).size == (1728, 1728)
    )
    names = [str(p.relative_to(root)) for p in paths]
    luts = torch.stack([_hald_to_lut(p) for p in paths])
    torch.save({"names": names, "luts": luts}, cache)
    return names, luts


def split_luts(names: list[str], luts: torch.Tensor):
    """(train LUTs, held-out names, held-out LUTs)."""
    held = torch.tensor([i % 10 == 0 for i in range(len(names))])
    return luts[~held], [n for n, h in zip(names, held) if h], luts[held]


def apply_random_luts(
    photos: torch.Tensor, others: torch.Tensor, luts: torch.Tensor, low: float = 0.6, high: float = 1.0,
    combine: float = 0.0,
) -> tuple[torch.Tensor, torch.Tensor]:
    """Gives each pair the same random LUT at a random strength (`low`–`high`, above 1 pushes the look further),
    sometimes (`combine`) chained with a second LUT: (reference = LUT on `others`, answer = LUT on `photos`)."""
    b = photos.shape[0]
    pick = luts[torch.randint(0, luts.shape[0], (b,))].to(photos.device)
    second = luts[torch.randint(0, luts.shape[0], (b,))].to(photos.device)
    chain = (torch.rand(b, 1, 1, 1, 1, device=photos.device) < combine).float()
    if combine > 0:
        # chaining: the second LUT sampled at the first one's output
        grid = pick.permute(0, 2, 3, 4, 1).reshape(b, 1, -1, 1, 3) * 2 - 1
        chained = F.grid_sample(second, grid, mode="bilinear", padding_mode="border", align_corners=True)
        chained = chained.reshape(b, 3, 33, 33, 33)
        pick = pick * (1 - chain) + chained * chain
    strength = torch.empty(b, 1, 1, 1, device=photos.device).uniform_(low, high)
    ref = others + (apply_lut(others, pick) - others) * strength
    answer = photos + (apply_lut(photos, pick) - photos) * strength
    return ref.clamp(0, 1), answer.clamp(0, 1)
