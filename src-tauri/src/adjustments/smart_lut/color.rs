//! Colour maths shared by the Smart LUT analysis and compiler: sRGB transfer, OKLab, the V-Log encoding
//! the LUT is indexed with, and a CPU copy of the shader's display transform for neutral greys.

pub const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

pub fn luma(c: [f32; 3]) -> f32 {
    c[0] * LUMA[0] + c[1] * LUMA[1] + c[2] * LUMA[2]
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Linear sRGB → OKLab (Björn Ottosson). Works on scene values above 1 too.
pub fn linear_to_oklab(c: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = [c[0].max(0.0), c[1].max(0.0), c[2].max(0.0)];
    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

pub fn oklab_to_linear(lab: [f32; 3]) -> [f32; 3] {
    let l = lab[0] + 0.396_337_78 * lab[1] + 0.215_803_76 * lab[2];
    let m = lab[0] - 0.105_561_346 * lab[1] - 0.063_854_17 * lab[2];
    let s = lab[0] - 0.089_484_18 * lab[1] - 1.291_485_5 * lab[2];
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        (4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s).max(0.0),
        (-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s).max(0.0),
        (-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s).max(0.0),
    ]
}

/// The shader's `linear_to_vlog`, used to index the Smart LUT (covers linear 0 … ~46).
pub fn vlog_encode(x: f32) -> f32 {
    let x = x.max(0.0);
    if x <= 0.01 {
        5.6 * x + 0.125
    } else {
        0.241_514 * (x + 0.008_73).log10() + 0.598_206
    }
}

pub fn vlog_decode(y: f32) -> f32 {
    if y <= 0.181 {
        ((y - 0.125) / 5.6).max(0.0)
    } else {
        (10f32.powf((y - 0.598_206) / 0.241_514) - 0.008_73).max(0.0)
    }
}

/// Which display transform the shader will run after the look.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DisplayMode {
    BasicRaw,
    BasicSrgb,
    Agx,
}

impl DisplayMode {
    pub fn from_uniforms(is_raw: bool, tonemapper_mode: u32) -> Self {
        if tonemapper_mode == 1 {
            DisplayMode::Agx
        } else if is_raw {
            DisplayMode::BasicRaw
        } else {
            DisplayMode::BasicSrgb
        }
    }
}

fn agx_sigmoid(x: f32, power: f32) -> f32 {
    x / (1.0 + x.powf(power)).powf(1.0 / power)
}

fn agx_scaled_sigmoid(x: f32, scale: f32, slope: f32, power: f32, tx: f32, ty: f32) -> f32 {
    scale * agx_sigmoid(slope * (x - tx) / scale, power) + ty
}

fn agx_curve(x: f32) -> f32 {
    const SLOPE: f32 = 2.3843;
    const TX: f32 = 0.606_060_6;
    const TY: f32 = 0.43446;
    let r = if x < TX {
        agx_scaled_sigmoid(x, -1.0359, SLOPE, 1.5, TX, TY)
    } else if x <= TX {
        SLOPE * x - 1.0112
    } else {
        agx_scaled_sigmoid(x, 1.3475, SLOPE, 1.5, TX, TY)
    };
    r.clamp(0.0, 1.0)
}

/// Display value the shader writes for a neutral scene grey `x` (linear), before curves.
pub fn display_gray(mode: DisplayMode, x: f32) -> f32 {
    match mode {
        DisplayMode::BasicSrgb => linear_to_srgb(x),
        DisplayMode::BasicRaw => {
            let s = linear_to_srgb(x).powf(1.0 / 1.1);
            let curve = s * s * (3.0 - 2.0 * s);
            s + (curve - s) * 0.75
        }
        DisplayMode::Agx => {
            let rel = (x / 0.18).max(1.0e-6);
            let encoded = ((rel.log2() + 15.2) / 20.2).clamp(0.0, 1.0);
            agx_curve(encoded).powf(2.4)
        }
    }
}

/// Scene grey that `display_gray` maps to `y` (bisection on log2 x).
pub fn display_inverse(mode: DisplayMode, y: f32) -> f32 {
    let (mut lo, mut hi) = (-16.0f32, 7.0f32);
    let top = display_gray(mode, hi.exp2());
    if y >= top {
        // the transform saturates: the smallest input that reaches white
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if display_gray(mode, mid.exp2()) >= top - 1.0e-4 {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        return hi.exp2();
    }
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if display_gray(mode, mid.exp2()) < y {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (0.5 * (lo + hi)).exp2()
}

/// Smallest signed difference between two angles in radians.
pub fn angle_diff(a: f32, b: f32) -> f32 {
    let mut d = a - b;
    while d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    while d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklab_round_trips() {
        for c in [[0.2, 0.5, 0.9], [1.5, 0.3, 0.1], [0.0, 0.0, 0.0], [0.18, 0.18, 0.18]] {
            let back = oklab_to_linear(linear_to_oklab(c));
            for i in 0..3 {
                assert!((back[i] - c[i]).abs() < 1.0e-3, "{:?} -> {:?}", c, back);
            }
        }
    }

    #[test]
    fn vlog_round_trips() {
        for x in [0.0, 0.005, 0.01, 0.18, 1.0, 10.0] {
            assert!((vlog_decode(vlog_encode(x)) - x).abs() < 1.0e-3 * (1.0 + x));
        }
    }

    #[test]
    fn display_inverse_inverts() {
        for mode in [DisplayMode::BasicRaw, DisplayMode::BasicSrgb, DisplayMode::Agx] {
            for y in [0.05, 0.2, 0.45, 0.8] {
                let x = display_inverse(mode, y);
                assert!((display_gray(mode, x) - y).abs() < 2.0e-3, "{:?} {}", mode, y);
            }
        }
    }
}
