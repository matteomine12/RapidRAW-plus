"""LutNet: reference photo + target photo → a 33³ 3D LUT (sRGB → sRGB) that puts the reference's look on the
target. Image-adaptive LUT design: a small CNN and colour histograms of both photos predict the weights of K
learned basis LUTs, added to the identity. The LUT can be dropped into the app's Smart LUT slots."""

import torch
import torch.nn as nn
import torch.nn.functional as F

LUT_SIZE = 33
BASIS = 16
HIST_BINS = 8


def identity_lut(n: int = LUT_SIZE) -> torch.Tensor:
    """(3, n, n, n) indexed [channel][b][g][r]."""
    r = torch.linspace(0, 1, n)
    b, g, rr = torch.meshgrid(r, r, r, indexing="ij")
    return torch.stack([rr, g, b])


def apply_lut(img: torch.Tensor, lut: torch.Tensor) -> torch.Tensor:
    """`img` (B, 3, H, W) in 0..1, `lut` (B, 3, n, n, n) → (B, 3, H, W)."""
    grid = (img.permute(0, 2, 3, 1) * 2 - 1).unsqueeze(1)  # (B, 1, H, W, 3) as (x=r, y=g, z=b)
    return F.grid_sample(lut, grid, mode="bilinear", padding_mode="border", align_corners=True).squeeze(2)


def colour_histogram(img: torch.Tensor, bins: int = HIST_BINS) -> torch.Tensor:
    """Soft joint RGB histogram, (B, bins³), sums to 1."""
    small = F.interpolate(img, size=(64, 64), mode="area").clamp(0, 1)
    idx = (small * (bins - 1)).round().long()
    flat = (idx[:, 0] * bins + idx[:, 1]) * bins + idx[:, 2]
    hist = torch.zeros(img.shape[0], bins**3, device=img.device)
    hist.scatter_add_(1, flat.flatten(1), torch.ones_like(flat.flatten(1), dtype=hist.dtype))
    return hist / hist.sum(1, keepdim=True)


def block(cin, cout):
    return nn.Sequential(
        nn.Conv2d(cin, cout, 3, stride=2, padding=1),
        nn.LeakyReLU(0.2),
        nn.InstanceNorm2d(cout, affine=True),
    )


class Encoder(nn.Module):
    def __init__(self):
        super().__init__()
        self.net = nn.Sequential(block(3, 16), block(16, 32), block(32, 64), block(64, 128), block(128, 128))

    def forward(self, x):
        x = F.interpolate(x, size=(256, 256), mode="bilinear", align_corners=False)
        f = self.net(x)
        return torch.cat([f.mean((2, 3)), f.amax((2, 3))], 1)  # (B, 256)


class LutNet(nn.Module):
    def __init__(self, basis: int = BASIS, n: int = LUT_SIZE):
        super().__init__()
        self.encoder = Encoder()
        feat = 256 * 2 + 2 * HIST_BINS**3
        self.head = nn.Sequential(nn.Linear(feat, 512), nn.LeakyReLU(0.2), nn.Dropout(0.1), nn.Linear(512, basis))
        self.basis = nn.Parameter(torch.randn(basis, 3, n, n, n) * 1e-3)
        self.register_buffer("identity", identity_lut(n))

    def forward(self, reference: torch.Tensor, target: torch.Tensor) -> torch.Tensor:
        """→ (B, 3, n, n, n) LUT."""
        f = torch.cat(
            [self.encoder(reference), self.encoder(target), colour_histogram(reference), colour_histogram(target)], 1
        )
        w = self.head(f)
        return self.identity + torch.einsum("bk,kcdhw->bcdhw", w, self.basis)


def lut_regularisers(lut: torch.Tensor) -> tuple[torch.Tensor, torch.Tensor]:
    """(smoothness, monotonicity) penalties."""
    dr = lut[..., 1:] - lut[..., :-1]
    dg = lut[..., 1:, :] - lut[..., :-1, :]
    db = lut[:, :, 1:] - lut[:, :, :-1]
    tv = dr.pow(2).mean() + dg.pow(2).mean() + db.pow(2).mean()
    mono = F.relu(-dr[:, 0]).mean() + F.relu(-dg[:, 1]).mean() + F.relu(-db[:, 2]).mean()
    return tv, mono
