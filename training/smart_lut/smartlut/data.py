"""Photos for training: random crops of every JPEG/PNG under a folder, with a fixed held-out split."""

from pathlib import Path
import random

import torch
from PIL import Image
from torch.utils.data import Dataset

EXTENSIONS = {".jpg", ".jpeg", ".png", ".tif", ".tiff", ".webp"}
HOLDOUT = 300


def list_photos(root: str | Path) -> list[Path]:
    return sorted(p for p in Path(root).rglob("*") if p.suffix.lower() in EXTENSIONS)


def split(root: str | Path) -> tuple[list[Path], list[Path]]:
    """(train, held-out): the held-out photos are never trained on."""
    photos = list_photos(root)
    rng = random.Random(1234)
    shuffled = photos[:]
    rng.shuffle(shuffled)
    return shuffled[HOLDOUT:], shuffled[:HOLDOUT]


def load(path: Path, size: int | None = None) -> torch.Tensor:
    img = Image.open(path).convert("RGB")
    if size:
        img.thumbnail((size, size), Image.BICUBIC)
    return torch.frombuffer(bytearray(img.tobytes()), dtype=torch.uint8).view(img.height, img.width, 3).permute(2, 0, 1).float() / 255


class Photos(Dataset):
    def __init__(self, paths: list[Path], size: int = 256):
        self.paths = paths
        self.size = size

    def __len__(self):
        return len(self.paths)

    def __getitem__(self, i):
        img = Image.open(self.paths[i]).convert("RGB")
        w, h = img.size
        side = int(min(w, h) * random.uniform(0.55, 1.0))
        x, y = random.randint(0, w - side), random.randint(0, h - side)
        img = img.crop((x, y, x + side, y + side)).resize((self.size, self.size), Image.BICUBIC)
        if random.random() < 0.5:
            img = img.transpose(Image.FLIP_LEFT_RIGHT)
        t = torch.frombuffer(bytearray(img.tobytes()), dtype=torch.uint8).view(self.size, self.size, 3)
        return t.permute(2, 0, 1).float() / 255
