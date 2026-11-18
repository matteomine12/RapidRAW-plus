"""Colour transfer targets: move a photo's colour and tone distribution toward a reference's.

This is what "take the colours of this photo" means to a user: the reference's palette and mood end up on the
target. The answer is computed with iterative distribution transfer (Pitié et al.): in OKLab, repeatedly match
the 1-D distributions of the two pixel clouds along random directions. The model, which can only output a smooth
3D LUT, learns a clean version of it.
"""

import torch

from .color import linear_to_oklab, linear_to_srgb, oklab_to_linear, srgb_to_linear

ITERATIONS = 12
SAMPLES = 8192


def _random_rotations(batch: int, device) -> torch.Tensor:
    """Random 2-D rotations of the (a, b) plane, (B, 2, 2)."""
    angle = torch.rand(batch, device=device) * torch.pi
    c, s = torch.cos(angle), torch.sin(angle)
    return torch.stack([torch.stack([c, -s], -1), torch.stack([s, c], -1)], -2)


def _match(source: torch.Tensor, reference: torch.Tensor) -> torch.Tensor:
    """Per batch and column, the reference value at each source value's quantile. (B, N, C), (B, M, C)."""
    ranks = source.argsort(1).argsort(1).float() / max(source.shape[1] - 1, 1)
    ref_sorted = reference.sort(1).values
    idx = (ranks * (reference.shape[1] - 1)).round().long()
    return torch.gather(ref_sorted, 1, idx)


def distribution_transfer(target: torch.Tensor, reference: torch.Tensor) -> torch.Tensor:
    """OKLab pixels (B, N, 3) moved to the reference's distribution (B, M, 3): lightness by quantile matching,
    colour (a, b) by iterative 2-D distribution transfer. Kept apart so lightness never leaks into hue (a
    black-and-white reference gives exactly neutral colour)."""
    lightness = _match(target[..., :1], reference[..., :1])
    ab, ref_ab = target[..., 1:], reference[..., 1:]
    for _ in range(ITERATIONS):
        rot = _random_rotations(target.shape[0], target.device)
        projected = ab @ rot
        matched = _match(projected, ref_ab @ rot)
        ab = ab + (matched - projected) @ rot.transpose(1, 2)
    return torch.cat([lightness, ab], -1)


def transfer_answer(
    target: torch.Tensor, reference: torch.Tensor, colour: float = 1.0, tone: float = 0.6
) -> torch.Tensor:
    """`target`, `reference` (B, 3, H, W) sRGB → target with the reference's colours (`colour` of the way) and
    tones (`tone` of the way), sRGB."""
    b, _, h, w = target.shape
    pixels = linear_to_oklab(srgb_to_linear(target)).flatten(2).transpose(1, 2)
    ref = linear_to_oklab(srgb_to_linear(reference)).flatten(2).transpose(1, 2)
    pick = torch.randint(0, ref.shape[1], (b, min(SAMPLES, ref.shape[1]), 1), device=ref.device)
    ref = torch.gather(ref, 1, pick.expand(-1, -1, 3))
    moved = distribution_transfer(pixels, ref)
    mixed = torch.cat(
        [
            pixels[..., :1] + (moved[..., :1] - pixels[..., :1]) * tone,
            pixels[..., 1:] + (moved[..., 1:] - pixels[..., 1:]) * colour,
        ],
        -1,
    )
    return linear_to_srgb(oklab_to_linear(mixed.transpose(1, 2).reshape(b, 3, h, w)))
