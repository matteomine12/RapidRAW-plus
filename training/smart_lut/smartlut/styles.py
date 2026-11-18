"""Random looks for self-supervised training.

A look is a set of parameters applied the way the app's Smart LUT thinks about a look: tones relative to the
photo's own median (key shift, contrast, faded blacks, a soft shoulder), colour per tonal band in OKLab
(chroma scale and tint, black and white with optional toning), and per-hue rotation and saturation. The same
look applied to two different photos gives the training pair: the model sees the look on photo A and must
reproduce it on photo B.
"""

from dataclasses import dataclass
import math

import torch

from .color import linear_to_oklab, linear_to_srgb, luma, oklab_to_linear, srgb_to_linear

HUE_CENTRES = torch.tensor([i * math.pi / 3 for i in range(6)])


@dataclass
class Look:
    key_shift: torch.Tensor  # (B,) stops
    contrast: torch.Tensor  # (B,)
    fade: torch.Tensor  # (B,) black lift, display
    shoulder: torch.Tensor  # (B,) 0 = hard, 1 = soft highlights
    chroma: torch.Tensor  # (B, 3) per band, 0 = black and white
    tint: torch.Tensor  # (B, 3, 2) per band (a/L, b/L)
    hue_shift: torch.Tensor  # (B, 6) radians per hue sector
    hue_sat: torch.Tensor  # (B, 6) chroma factor per hue sector

    def to(self, device):
        return Look(**{k: v.to(device) for k, v in self.__dict__.items()})


def random_looks(batch: int, generator: torch.Generator | None = None) -> Look:
    g = generator

    def n(*shape):
        return torch.randn(*shape, generator=g)

    def u(*shape):
        return torch.rand(*shape, generator=g)

    mono = (u(batch) < 0.12).float()
    chroma = (n(batch, 3) * 0.3).exp().clamp(0.0, 1.8) * (1 - mono[:, None])
    # occasionally a strongly desaturated look rather than full black and white
    chroma = torch.where((u(batch, 1) < 0.1), chroma * 0.35, chroma)
    tint = n(batch, 3, 2) * 0.045
    # split toning: shadows and highlights pulled to complementary hues
    split = (u(batch) < 0.35).float()[:, None]
    tint[:, 2] = tint[:, 2] * (1 - split) - tint[:, 0] * split * 0.8
    tint = tint * torch.where(mono[:, None, None] > 0, 0.6, 1.0)
    hue_on = (u(batch, 1) < 0.6).float()
    return Look(
        key_shift=(n(batch) * 0.5).clamp(-1.2, 1.2),
        contrast=(n(batch) * 0.25).exp().clamp(0.6, 1.7),
        fade=(u(batch) < 0.3).float() * u(batch) * 0.07,
        shoulder=u(batch),
        chroma=chroma,
        tint=tint,
        hue_shift=(n(batch, 6) * math.radians(15)).clamp(-math.radians(35), math.radians(35)) * hue_on,
        hue_sat=(n(batch, 6) * 0.2).exp() * hue_on + (1 - hue_on),
    )


def band_weights(x: torch.Tensor) -> torch.Tensor:
    """Shadow, midtone, highlight weights for `x` stops from the median (same as the app). (..., 3)"""
    t = ((x + 2.5) / 5.0).clamp(0, 1)
    return torch.stack([(1 - t) ** 2, 2 * t * (1 - t), t ** 2], -1)


def image_median(lin: torch.Tensor) -> torch.Tensor:
    small = torch.nn.functional.interpolate(lin, size=(64, 64), mode="area")
    return luma(small).flatten(1).median(1).values.clamp_min(1e-5)


def _display(y: torch.Tensor, shoulder: torch.Tensor) -> torch.Tensor:
    """Scene luma → display luma (linear): an ACES-like curve, softer as `shoulder` grows."""
    x = y * 0.75
    aces = (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)
    reinhard = y / (1 + y)
    return (aces + (reinhard * 1.35 - aces) * shoulder * 0.5).clamp(0, 1)


def apply_look(img: torch.Tensor, look: Look) -> torch.Tensor:
    """`img` (B, 3, H, W) sRGB 0..1 → the photo with `look`, sRGB 0..1."""
    lin = srgb_to_linear(img)
    y = luma(lin).clamp_min(1e-6)
    med = image_median(lin)[:, None, None]
    x = torch.log2(y / med)

    k = lambda v: v[:, None, None]  # noqa: E731
    y_scene = 0.18 * torch.exp2(k(look.key_shift) + k(look.contrast) * x)
    y_disp = _display(y_scene, k(look.shoulder))
    y_disp = k(look.fade) + (1 - k(look.fade)) * y_disp
    lin = lin * (y_disp / y)[:, None]

    lab = linear_to_oklab(lin)
    l = lab[:, :1].clamp_min(1e-4)
    c = lab[:, 1:] / l
    w = band_weights(x)  # (B, H, W, 3)
    scale = (w * look.chroma[:, None, None, :]).sum(-1)[:, None]
    tint = torch.einsum("bhwk,bkc->bchw", w, look.tint)
    c = c * scale + tint

    h = torch.atan2(c[:, 1], c[:, 0])
    centres = HUE_CENTRES.to(img.device)
    d = torch.atan2(torch.sin(h[..., None] - centres), torch.cos(h[..., None] - centres))
    hw = torch.exp(2.5 * (torch.cos(d) - 1))
    hw = hw / hw.sum(-1, keepdim=True)
    shift = (hw * look.hue_shift[:, None, None, :]).sum(-1)
    sat = (hw * look.hue_sat[:, None, None, :]).sum(-1)
    cos, sin = torch.cos(shift), torch.sin(shift)
    c = torch.stack([c[:, 0] * cos - c[:, 1] * sin, c[:, 0] * sin + c[:, 1] * cos], 1) * sat[:, None]
    norm = c.norm(dim=1, keepdim=True)
    c = c * (0.9 / norm.clamp_min(0.9))

    out = oklab_to_linear(torch.cat([l, c * l], 1))
    return linear_to_srgb(out)


def random_grades(batch: int, device) -> dict:
    """Strong colour grades: 1–3 colour stops from shadows to highlights (one stop = a tint of one colour,
    two = duotone / split toning, three = tritone), how much of the photo's own colour survives, and how much
    of the grade is applied."""
    stops = torch.randint(1, 4, (batch,), device=device)
    hue = torch.rand(batch, 3, device=device) * 2 * math.pi
    # neighbouring stops are often related hues (analogous) and sometimes complementary
    spread = torch.where(torch.rand(batch, 1, device=device) < 0.5, 0.6, math.pi) * torch.randn(batch, 1, device=device).sign()
    hue[:, 1:] = hue[:, :1] + spread * torch.arange(1, 3, device=device)[None] * torch.rand(batch, 1, device=device)
    chroma = 0.06 + torch.rand(batch, 3, device=device) * 0.22
    return {
        "stops": stops,
        "hue": hue,
        "chroma": chroma,
        "keep": torch.rand(batch, device=device) * 0.6,  # share of the photo's own chroma kept
        "strength": 0.6 + torch.rand(batch, device=device) * 0.4,
        "contrast": (torch.randn(batch, device=device) * 0.2).exp(),
    }


def apply_grade(img: torch.Tensor, grade: dict) -> torch.Tensor:
    """`img` (B, 3, H, W) sRGB → graded sRGB (in OKLab: lightness kept with a contrast tweak, colour from a
    gradient over the stops by lightness)."""
    lab = linear_to_oklab(srgb_to_linear(img))
    l = lab[:, :1]
    k = lambda v: v[:, None, None, None]  # noqa: E731
    l_new = (0.5 + (l - 0.5) * k(grade["contrast"])).clamp(0, 1)
    ab_stops = torch.stack([torch.cos(grade["hue"]), torch.sin(grade["hue"])], -1) * grade["chroma"][..., None]
    n = grade["stops"].float()
    # position along the gradient: 0 (shadows) .. stops-1 (highlights)
    pos = (l_new.clamp(0, 1) * (k(n) - 1)).clamp_min(0)
    i0 = pos.floor().long().clamp(max=2)
    i1 = (i0 + 1).clamp(max=2)
    i1 = torch.minimum(i1, k(grade["stops"]) - 1)
    f = pos - pos.floor()
    gather = lambda idx: torch.gather(  # noqa: E731
        ab_stops[:, None, None, :, :].expand(-1, l.shape[2], l.shape[3], -1, -1),
        3,
        idx[:, 0, :, :, None, None].expand(-1, -1, -1, 1, 2),
    )[:, :, :, 0].permute(0, 3, 1, 2)
    ab_grade = gather(i0) * (1 - f) + gather(i1) * f
    # the tint fades in deep blacks and paper white, like real grades
    fade = (l_new / 0.08).clamp(0, 1) * ((1 - l_new) / 0.08).clamp(0, 1)
    ab = lab[:, 1:] * k(grade["keep"]) + ab_grade * fade
    graded = torch.cat([l_new, ab], 1)
    out = lab + (graded - lab) * k(grade["strength"])
    return linear_to_srgb(oklab_to_linear(out))
