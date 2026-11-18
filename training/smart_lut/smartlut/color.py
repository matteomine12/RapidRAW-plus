"""Colour maths on torch tensors shaped (..., 3, H, W) or (..., 3): sRGB transfer and OKLab."""

import torch

LUMA = (0.2126, 0.7152, 0.0722)

_M1 = torch.tensor([
    [0.4122214708, 0.5363325363, 0.0514459929],
    [0.2119034982, 0.6806995451, 0.1073969566],
    [0.0883024619, 0.2817188376, 0.6299787005],
])
_M2 = torch.tensor([
    [0.2104542553, 0.7936177850, -0.0040720468],
    [1.9779984951, -2.4285922050, 0.4505937099],
    [0.0259040371, 0.7827717662, -0.8086757660],
])
_M2_INV = torch.linalg.inv(_M2)
_M1_INV = torch.linalg.inv(_M1)


def srgb_to_linear(x: torch.Tensor) -> torch.Tensor:
    x = x.clamp(0, 1)
    return torch.where(x <= 0.04045, x / 12.92, ((x + 0.055) / 1.055) ** 2.4)


def linear_to_srgb(x: torch.Tensor) -> torch.Tensor:
    x = x.clamp(0, 1)
    return torch.where(x <= 0.0031308, x * 12.92, 1.055 * x.clamp_min(1e-12) ** (1 / 2.4) - 0.055)


def _apply(m: torch.Tensor, x: torch.Tensor, dim: int) -> torch.Tensor:
    m = m.to(x.device, x.dtype)
    x = x.movedim(dim, -1)
    return (x @ m.T).movedim(-1, dim)


def linear_to_oklab(x: torch.Tensor, dim: int = -3) -> torch.Tensor:
    lms = _apply(_M1, x.clamp_min(0), dim)
    return _apply(_M2, torch.sign(lms) * lms.abs().clamp_min(1e-12) ** (1 / 3), dim)


def oklab_to_linear(lab: torch.Tensor, dim: int = -3) -> torch.Tensor:
    lms = _apply(_M2_INV, lab, dim) ** 3
    return _apply(_M1_INV, lms, dim).clamp_min(0)


def luma(x: torch.Tensor, dim: int = -3) -> torch.Tensor:
    w = torch.tensor(LUMA, device=x.device, dtype=x.dtype)
    return (x.movedim(dim, -1) * w).sum(-1)
