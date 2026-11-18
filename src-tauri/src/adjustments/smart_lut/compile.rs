//! Turns a look and the statistics of the photo it is applied to into four 3D LUTs (whole image, skin,
//! sky, vegetation). The LUTs are indexed by V-Log of the photo's scene light normalised so its median
//! sits at 0.18, and return V-Log of the light the display transform should receive.

use rayon::prelude::*;

use super::color::{
    DisplayMode, angle_diff, display_gray, display_inverse, linear_to_oklab, linear_to_srgb, luma,
    oklab_to_linear, srgb_to_linear, vlog_decode, vlog_encode,
};
use super::look::{SmartLook, ToneSpec};
use super::stats::{BandStats, ImageStats, RegionStats, band_weights};

pub const LUT_SIZE: usize = 33;
pub const LUT_SLOTS: usize = 4;
/// How far an image look's tones go toward the reference (the rest is a standard rendering).
const TONE_MATCH: f32 = 0.8;
const MIN_SLOPE: f32 = 0.3;
const MAX_SLOPE: f32 = 2.5;
/// The photo's own cast is neutralised only up to this much (a/L, b/L).
const MAX_CAST_REMOVAL: f32 = 0.08;

fn clamp_vector(v: [f32; 2], limit: f32) -> [f32; 2] {
    let n = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if n > limit { [v[0] * limit / n, v[1] * limit / n] } else { v }
}

/// Four LUT_SIZE³ RGB tables stacked along blue: slot `s` holds z in `s*size .. (s+1)*size`.
pub struct SmartLutData {
    pub size: usize,
    pub data: Vec<f32>,
}

/// Monotone cubic in log2 space.
struct ToneCurve {
    xs: Vec<f32>,
    ys: Vec<f32>,
    ms: Vec<f32>,
    low_slope: f32,
    high_slope: f32,
}

impl ToneCurve {
    fn new(mut points: Vec<(f32, f32)>) -> Self {
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f32, f32)> = Vec::new();
        for p in points {
            match merged.last_mut() {
                Some(last) if p.0 - last.0 < 0.08 => {
                    last.1 = 0.5 * (last.1 + p.1);
                }
                _ => merged.push(p),
            }
        }
        if merged.len() < 2 {
            let x = merged.first().map_or(0.0, |p| p.0);
            let y = merged.first().map_or(0.0, |p| p.1);
            merged = vec![(x - 1.0, y - 1.0), (x + 1.0, y + 1.0)];
        }
        // rebuild ys from the median outward with bounded slopes
        let n = merged.len();
        let mid = merged
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.0.abs().total_cmp(&b.1.0.abs()))
            .map_or(0, |(i, _)| i);
        let xs: Vec<f32> = merged.iter().map(|p| p.0).collect();
        let raw: Vec<f32> = merged.iter().map(|p| p.1).collect();
        let mut slopes: Vec<f32> = (0..n - 1)
            .map(|i| ((raw[i + 1] - raw[i]) / (xs[i + 1] - xs[i])).clamp(MIN_SLOPE, MAX_SLOPE))
            .collect();
        if slopes.len() > 2 {
            let s = slopes.clone();
            for i in 0..s.len() {
                let a = s[i.saturating_sub(1)];
                let c = s[(i + 1).min(s.len() - 1)];
                slopes[i] = 0.25 * a + 0.5 * s[i] + 0.25 * c;
            }
        }
        let mut ys = vec![0.0f32; n];
        ys[mid] = raw[mid];
        for i in mid + 1..n {
            ys[i] = ys[i - 1] + slopes[i - 1] * (xs[i] - xs[i - 1]);
        }
        for i in (0..mid).rev() {
            ys[i] = ys[i + 1] - slopes[i] * (xs[i + 1] - xs[i]);
        }
        // Fritsch–Carlson tangents
        let mut ms = vec![0.0f32; n];
        for i in 0..n {
            ms[i] = if i == 0 {
                slopes[0]
            } else if i == n - 1 {
                slopes[n - 2]
            } else {
                let (a, b) = (slopes[i - 1], slopes[i]);
                if a * b <= 0.0 { 0.0 } else { 2.0 * a * b / (a + b) }
            };
        }
        let low_slope = slopes[0].clamp(0.6, 1.4);
        let high_slope = slopes[n - 2].clamp(0.2, 1.0);
        ToneCurve {
            xs,
            ys,
            ms,
            low_slope,
            high_slope,
        }
    }

    fn eval(&self, x: f32) -> f32 {
        let n = self.xs.len();
        if x <= self.xs[0] {
            return self.ys[0] + (x - self.xs[0]) * self.low_slope;
        }
        if x >= self.xs[n - 1] {
            return self.ys[n - 1] + (x - self.xs[n - 1]) * self.high_slope;
        }
        let i = self.xs.partition_point(|&v| v <= x).saturating_sub(1).min(n - 2);
        let h = self.xs[i + 1] - self.xs[i];
        let t = (x - self.xs[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * self.ys[i]
            + (t3 - 2.0 * t2 + t) * h * self.ms[i]
            + (-2.0 * t3 + 3.0 * t2) * self.ys[i + 1]
            + (t3 - t2) * h * self.ms[i + 1]
    }
}

/// A pleasing standard rendering of scene light (display-encoded), used as the neutral tone target.
fn standard_display(x: f32) -> f32 {
    let x = (x * 0.75).max(0.0);
    let aces = (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14);
    linear_to_srgb(aces.clamp(0.0, 1.0))
}

fn tone_curve(look: &SmartLook, target: &ImageStats, mode: DisplayMode) -> ToneCurve {
    let log_o = |display: f32| display_inverse(mode, display.clamp(0.0, 1.0)).max(1.0e-6).log2();
    let points = target
        .log_quantiles
        .iter()
        .enumerate()
        .map(|(i, &t)| {
            let neutral = log_o(standard_display(0.18 * t.exp2()));
            let y = match &look.tone {
                ToneSpec::Quantiles { display } => {
                    let wanted = log_o(display.get(i).copied().unwrap_or(0.5));
                    neutral + (wanted - neutral) * TONE_MATCH
                }
                ToneSpec::Parametric {
                    key_shift,
                    contrast,
                } => log_o(standard_display(0.18 * (key_shift + contrast * t).exp2())),
            };
            (t, y)
        })
        .collect();
    ToneCurve::new(points)
}

#[derive(Clone, Copy)]
struct Slot<'a> {
    reference: &'a [BandStats; 3],
    target: &'a [BandStats; 3],
    /// Stops added to this region's tones.
    lum_offset: f32,
    hue_pull: f32,
    /// 0 = the look's colour, 1 = the photo's own colour (skin protection).
    protect: f32,
    /// How much of the look's tint whites and blacks are spared (0..1).
    end_protect: f32,
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn chroma_factor(look: &SmartLook, reference: &BandStats, target: &BandStats) -> f32 {
    if look.monochrome {
        return 0.0;
    }
    match look.saturation {
        Some(s) => s,
        None => (reference.spread / target.spread.max(0.02)).clamp(0.0, 2.5),
    }
}

struct Prepared {
    palette: Vec<(f32, f32)>,
}

fn entry(
    n: [f32; 3],
    look: &SmartLook,
    slot: &Slot,
    tone: &ToneCurve,
    prepared: &Prepared,
) -> [f32; 3] {
    let y = luma(n);
    if y < 1.0e-7 {
        return n;
    }
    let x = (y / 0.18).log2();
    let y_out = (tone.eval(x) + slot.lum_offset).exp2();

    let lab = linear_to_oklab(n);
    let l = lab[0].max(1.0e-4);
    let c = [lab[1] / l, lab[2] / l];
    let w = band_weights(x);
    // paper white and deep black keep most of their neutrality
    let ends = smoothstep(1.5, 3.2, x).max(smoothstep(-3.5, -5.5, x));
    let tint = 1.0 - ends * slot.end_protect;
    let mut out = [0.0f32; 2];
    for b in 0..3 {
        let s = chroma_factor(look, &slot.reference[b], &slot.target[b]);
        let rm = slot.reference[b].mean;
        let rm = [rm[0] * tint, rm[1] * tint];
        let tm = clamp_vector(slot.target[b].mean, MAX_CAST_REMOVAL);
        out[0] += w[b] * (rm[0] + (c[0] - tm[0]) * s);
        out[1] += w[b] * (rm[1] + (c[1] - tm[1]) * s);
    }

    let chroma = (out[0] * out[0] + out[1] * out[1]).sqrt();
    if slot.hue_pull > 0.0 && chroma > 0.02 && !prepared.palette.is_empty() {
        let h = out[1].atan2(out[0]);
        let (mut num, mut den) = (0.0f32, 0.35f32);
        for &(hue, weight) in &prepared.palette {
            let d = angle_diff(hue, h);
            let g = weight * (-(d / 0.8).powi(2)).exp();
            num += g * d;
            den += g;
        }
        let gate = ((chroma - 0.02) / 0.06).clamp(0.0, 1.0);
        let shift = (num / den * slot.hue_pull * gate).clamp(-0.6, 0.6);
        let (sin, cos) = shift.sin_cos();
        out = [out[0] * cos - out[1] * sin, out[0] * sin + out[1] * cos];
    }

    out[0] += (c[0] - out[0]) * slot.protect;
    out[1] += (c[1] - out[1]) * slot.protect;
    let chroma = (out[0] * out[0] + out[1] * out[1]).sqrt();
    if chroma > 0.9 {
        out = [out[0] * 0.9 / chroma, out[1] * 0.9 / chroma];
    }

    let rgb = oklab_to_linear([l, out[0] * l, out[1] * l]);
    let y_rgb = luma(rgb);
    if y_rgb < 1.0e-7 {
        return [y_out; 3];
    }
    let k = y_out / y_rgb;
    [rgb[0] * k, rgb[1] * k, rgb[2] * k]
}

fn region_slot<'a>(
    look: &'a SmartLook,
    target: &'a ImageStats,
    global: Slot<'a>,
    reference: Option<&'a RegionStats>,
    region: Option<&'a RegionStats>,
    is_skin: bool,
) -> Slot<'a> {
    match (reference, region) {
        (Some(r), Some(t)) => Slot {
            reference: &r.bands,
            target: &t.bands,
            lum_offset: ((r.rel_key - t.rel_key) * 0.8).clamp(-1.5, 1.5),
            hue_pull: if is_skin { 0.0 } else { look.hue_pull * 0.5 },
            protect: 0.0,
            end_protect: global.end_protect,
        },
        (None, Some(t)) if is_skin && !look.monochrome => Slot {
            // the reference has no people: keep skin close to its own colour
            reference: &look.bands,
            target: &t.bands,
            lum_offset: 0.0,
            hue_pull: 0.0,
            protect: 0.65,
            end_protect: global.end_protect,
        },
        _ => {
            let _ = target;
            global
        }
    }
}

pub fn compile(look: &SmartLook, target: &ImageStats, mode: DisplayMode) -> SmartLutData {
    let tone = tone_curve(look, target, mode);
    let prepared = Prepared {
        palette: look
            .palette
            .iter()
            .filter_map(|p| {
                let lab = linear_to_oklab([
                    srgb_to_linear(p.rgb[0]),
                    srgb_to_linear(p.rgb[1]),
                    srgb_to_linear(p.rgb[2]),
                ]);
                let l = lab[0].max(0.05);
                let (a, b) = (lab[1] / l, lab[2] / l);
                ((a * a + b * b).sqrt() > 0.04).then(|| (b.atan2(a), p.weight.max(0.05)))
            })
            .collect(),
    };
    let global = Slot {
        reference: &look.bands,
        target: &target.bands,
        lum_offset: 0.0,
        hue_pull: look.hue_pull,
        protect: 0.0,
        end_protect: if look.saturation.is_some() { 0.8 } else { 0.3 },
    };
    let slots = [
        global,
        region_slot(
            look,
            target,
            global,
            look.regions.skin.as_ref(),
            target.regions.skin.as_ref(),
            true,
        ),
        region_slot(
            look,
            target,
            global,
            look.regions.sky.as_ref(),
            target.regions.sky.as_ref(),
            false,
        ),
        region_slot(
            look,
            target,
            global,
            look.regions.vegetation.as_ref(),
            target.regions.vegetation.as_ref(),
            false,
        ),
    ];

    let size = LUT_SIZE;
    let axis: Vec<f32> = (0..size)
        .map(|i| vlog_decode(i as f32 / (size - 1) as f32))
        .collect();
    let per_slot = size * size * size * 3;
    let mut data = vec![0.0f32; per_slot * LUT_SLOTS];
    data.par_chunks_mut(size * 3)
        .enumerate()
        .for_each(|(row, chunk)| {
            // rows run over (slot, b, g); r varies inside the row
            let slot = row / (size * size);
            let b = (row / size) % size;
            let g = row % size;
            for r in 0..size {
                let out = entry(
                    [axis[r], axis[g], axis[b]],
                    look,
                    &slots[slot],
                    &tone,
                    &prepared,
                );
                chunk[r * 3] = vlog_encode(out[0]);
                chunk[r * 3 + 1] = vlog_encode(out[1]);
                chunk[r * 3 + 2] = vlog_encode(out[2]);
            }
        });
    SmartLutData { size, data }
}

/// Display value a neutral grey of normalised scene light `x` gets through the look (for tests/UI).
#[allow(dead_code)]
pub fn preview_gray(look: &SmartLook, target: &ImageStats, mode: DisplayMode, x: f32) -> f32 {
    let tone = tone_curve(look, target, mode);
    display_gray(mode, tone.eval((x / 0.18).log2()).exp2())
}

#[cfg(test)]
mod tests {
    use super::super::look::{from_colors, from_reference};
    use super::super::stats::{Encoding, Regions, analyze};
    use super::*;
    use image::{DynamicImage, Rgb, RgbImage};

    fn gradient(f: impl Fn(u32, u32) -> Rgb<u8>) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(96, 96, f))
    }

    fn colourful() -> DynamicImage {
        gradient(|x, y| Rgb([(x * 2 + 20) as u8, (y * 2 + 10) as u8, ((x + y) + 30) as u8]))
    }

    #[test]
    fn a_black_and_white_reference_gives_grey_output() {
        let reference = gradient(|x, y| {
            let v = ((x + y) * 1) as u8;
            Rgb([v, v, v])
        });
        let look = from_reference("bw".into(), &analyze(&reference, Encoding::Srgb, &Regions::default(), true));
        assert!(look.monochrome);
        let target = analyze(&colourful(), Encoding::Srgb, &Regions::default(), false);
        let lut = compile(&look, &target, DisplayMode::BasicSrgb);
        let mid = (LUT_SIZE / 2) * 3 + (LUT_SIZE / 3) * LUT_SIZE * 3 + (LUT_SIZE / 2) * LUT_SIZE * LUT_SIZE * 3;
        let rgb = [vlog_decode(lut.data[mid]), vlog_decode(lut.data[mid + 1]), vlog_decode(lut.data[mid + 2])];
        assert!((rgb[0] - rgb[1]).abs() < 0.01 * rgb[1].max(0.01) + 1.0e-3, "{:?}", rgb);
        assert!((rgb[2] - rgb[1]).abs() < 0.01 * rgb[1].max(0.01) + 1.0e-3, "{:?}", rgb);
    }

    #[test]
    fn a_dark_reference_darkens_and_a_bright_one_brightens() {
        let target = analyze(&colourful(), Encoding::Srgb, &Regions::default(), false);
        let dark = gradient(|x, y| {
            let v = ((x + y) / 4) as u8;
            Rgb([v, v, v])
        });
        let bright = gradient(|x, y| {
            let v = (150 + (x + y) / 2) as u8;
            Rgb([v, v, v])
        });
        let dark = from_reference("d".into(), &analyze(&dark, Encoding::Srgb, &Regions::default(), false));
        let bright = from_reference("b".into(), &analyze(&bright, Encoding::Srgb, &Regions::default(), false));
        let mode = DisplayMode::BasicSrgb;
        assert!(preview_gray(&dark, &target, mode, 0.18) < preview_gray(&bright, &target, mode, 0.18));
    }

    #[test]
    fn tone_curves_are_monotone() {
        let target = analyze(&colourful(), Encoding::Srgb, &Regions::default(), false);
        let look = from_colors("c".into(), &[[0.8, 0.4, 0.2], [0.1, 0.3, 0.4]]).unwrap();
        for mode in [DisplayMode::BasicRaw, DisplayMode::Agx] {
            let curve = tone_curve(&look, &target, mode);
            let mut last = f32::MIN;
            for i in -80..60 {
                let v = curve.eval(i as f32 * 0.1);
                assert!(v >= last - 1.0e-4);
                last = v;
            }
        }
    }
}

/// Renders `SMART_TARGET` through a look made from `SMART_REF` (a picture, or `#rrggbb[,#rrggbb…]`) on the
/// CPU, the way the shader does for a JPEG, into `SMART_OUT`. For eyeballing looks:
/// `SMART_REF=a.jpg SMART_TARGET=b.jpg SMART_OUT=out.png cargo test --lib visual_check -- --ignored`
#[cfg(test)]
mod visual {
    use super::super::look::{from_colors, from_reference};
    use super::super::stats::{Encoding, Regions, analyze};
    use super::*;

    fn sample(lut: &SmartLutData, uv: [f32; 3]) -> [f32; 3] {
        let n = lut.size;
        let p = uv.map(|v| v.clamp(0.0, 1.0) * (n - 1) as f32);
        let i = p.map(|v| (v.floor() as usize).min(n - 2));
        let f = [p[0] - i[0] as f32, p[1] - i[1] as f32, p[2] - i[2] as f32];
        let at = |r: usize, g: usize, b: usize, c: usize| lut.data[((b * n + g) * n + r) * 3 + c];
        std::array::from_fn(|c| {
            let mut acc = 0.0;
            for (dr, wr) in [(0, 1.0 - f[0]), (1, f[0])] {
                for (dg, wg) in [(0, 1.0 - f[1]), (1, f[1])] {
                    for (db, wb) in [(0, 1.0 - f[2]), (1, f[2])] {
                        acc += wr * wg * wb * at(i[0] + dr, i[1] + dg, i[2] + db, c);
                    }
                }
            }
            acc
        })
    }

    #[test]
    #[ignore]
    fn visual_check() {
        let (Ok(reference), Ok(target), Ok(out)) = (
            std::env::var("SMART_REF"),
            std::env::var("SMART_TARGET"),
            std::env::var("SMART_OUT"),
        ) else {
            return;
        };
        let look = if reference.starts_with('#') {
            let colors: Vec<[f32; 3]> = reference
                .split(',')
                .map(|h| {
                    let v = u32::from_str_radix(h.trim_start_matches('#'), 16).unwrap();
                    [(v >> 16) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0]
                })
                .collect();
            from_colors("c".into(), &colors).unwrap()
        } else {
            let img = image::open(&reference).unwrap();
            from_reference("r".into(), &analyze(&img, Encoding::Srgb, &Regions::default(), true))
        };
        let img = image::open(&target).unwrap().to_rgb32f();
        let dyn_img = image::DynamicImage::ImageRgb32F(img.clone());
        let stats = analyze(&dyn_img, Encoding::Srgb, &Regions::default(), false);
        let lut = match std::env::var("SMART_MODEL") {
            // the neural engine, the way the app runs it
            Ok(model) => {
                let lib = concat!(env!("CARGO_MANIFEST_DIR"), "/resources/libonnxruntime.so");
                let _ = ort::init_from(lib).commit();
                let mut session = ort::session::Session::builder().unwrap().commit_from_file(model).unwrap();
                let reference = super::super::neural::thumbnail(&image::open(&reference).unwrap());
                let target = super::super::neural::thumbnail(&dyn_img);
                let raw = super::super::neural::infer(&mut session, &reference, &target).unwrap();
                let neural = super::super::neural::NeuralLut::new(raw, &dyn_img);
                super::super::neural::to_engine(&neural, DisplayMode::BasicSrgb)
            }
            Err(_) => compile(&look, &stats, DisplayMode::BasicSrgb),
        };
        let mut ys: Vec<f32> = img
            .pixels()
            .map(|p| luma([srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2])]))
            .collect();
        ys.sort_by(f32::total_cmp);
        let median = ys[ys.len() / 2].max(1.0e-6);
        let amount = |k: &str| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(1.0f32);
        let (ca, la) = (amount("SMART_COLOR"), amount("SMART_LUM"));
        let mut result = image::RgbImage::new(img.width(), img.height());
        for (p, o) in img.pixels().zip(result.pixels_mut()) {
            let src = [srgb_to_linear(p[0]), srgb_to_linear(p[1]), srgb_to_linear(p[2])];
            let n = src.map(|v| vlog_encode(v * 0.18 / median));
            let look_rgb = sample(&lut, n).map(vlog_decode);
            let (b, l) = (linear_to_oklab(src), linear_to_oklab(look_rgb));
            let lo = b[0] + (l[0] - b[0]) * la;
            let cb = [b[1] / b[0].max(1.0e-4), b[2] / b[0].max(1.0e-4)];
            let cl = [l[1] / l[0].max(1.0e-4), l[2] / l[0].max(1.0e-4)];
            let c = [cb[0] + (cl[0] - cb[0]) * ca, cb[1] + (cl[1] - cb[1]) * ca];
            let rgb = oklab_to_linear([lo, c[0] * lo, c[1] * lo]);
            *o = image::Rgb(rgb.map(|v| (linear_to_srgb(v) * 255.0).round() as u8));
        }
        result.save(out).unwrap();
    }
}
