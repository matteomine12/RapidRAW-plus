//! Image statistics the Smart LUT is built from: where the tones sit (luma quantiles), what colour each
//! tonal band carries (mean and spread of OKLab chroma relative to lightness), the same per region
//! (skin, sky, vegetation) and a small palette.

use image::{DynamicImage, GenericImageView, GrayImage, imageops::FilterType};
use serde::{Deserialize, Serialize};

use super::color::{linear_to_oklab, linear_to_srgb, luma, oklab_to_linear, srgb_to_linear};

/// Luma quantiles sampled for the tone transfer.
pub const QUANTILES: [f32; 17] = [
    0.01, 0.03, 0.06, 0.10, 0.15, 0.20, 0.30, 0.40, 0.50, 0.60, 0.70, 0.80, 0.85, 0.90, 0.94, 0.97,
    0.99,
];
const ANALYSIS_EDGE: u32 = 360;
/// Lightness under which a pixel's chroma is too noisy to say anything about its colour.
const L_FLOOR: f32 = 0.06;
/// A region must cover this much of the frame to count.
const MIN_COVERAGE: f32 = 0.006;
/// Chroma (a/L, b/L) around which a pixel stops counting as neutral when reading a colour cast.
const NEUTRAL_CHROMA: f32 = 0.1;

/// Weights of the shadow, midtone and highlight bands for a pixel `x` stops from the median.
pub fn band_weights(x: f32) -> [f32; 3] {
    let t = ((x + 2.5) / 5.0).clamp(0.0, 1.0);
    [(1.0 - t) * (1.0 - t), 2.0 * t * (1.0 - t), t * t]
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BandStats {
    /// Mean of (a/L, b/L): the band's colour cast.
    pub mean: [f32; 2],
    /// Mean distance of (a/L, b/L) from that mean: how colourful the band is.
    pub spread: f32,
    /// Share of the image's weight in this band.
    pub weight: f32,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RegionStats {
    pub coverage: f32,
    /// log2 of the region's median luma over the whole image's.
    pub rel_key: f32,
    pub bands: [BandStats; 3],
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Regions<T> {
    pub skin: T,
    pub sky: T,
    pub vegetation: T,
}

impl<T> Regions<T> {
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Regions<U> {
        Regions {
            skin: f(self.skin),
            sky: f(self.sky),
            vegetation: f(self.vegetation),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PaletteColor {
    /// Display sRGB, 0..1.
    pub rgb: [f32; 3],
    pub weight: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ImageStats {
    /// log2(luma / median) at each of `QUANTILES` (scene light).
    pub log_quantiles: Vec<f32>,
    /// sRGB-encoded luma at each of `QUANTILES` (how the image looks on screen).
    pub display_quantiles: Vec<f32>,
    pub bands: [BandStats; 3],
    /// Mean |(a/L, b/L)|: overall colourfulness.
    pub chroma: f32,
    pub regions: Regions<Option<RegionStats>>,
    #[serde(default)]
    pub palette: Vec<PaletteColor>,
}

/// How the pixels of an image encode light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// A rendered picture (JPEG, PNG…): sRGB.
    Srgb,
    /// The editor's CPU preview of a RAW (gamma 2.38 and a contrast S).
    RawPreview,
}

fn decode_value(v: f32, encoding: Encoding) -> f32 {
    match encoding {
        Encoding::Srgb => srgb_to_linear(v),
        Encoding::RawPreview => (((v - 0.5) / 1.28) + 0.5).clamp(0.0, 1.0).powf(2.38),
    }
}

struct Sample {
    y: f32,
    lab: [f32; 3],
}

fn samples(image: &DynamicImage, encoding: Encoding) -> (Vec<Sample>, u32, u32) {
    let (w, h) = image.dimensions();
    let scale = (ANALYSIS_EDGE as f32 / w.max(h) as f32).min(1.0);
    let (sw, sh) = (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    );
    let small = image.resize_exact(sw, sh, FilterType::Triangle).to_rgb32f();
    let out = small
        .pixels()
        .map(|p| {
            let rgb = [
                decode_value(p[0], encoding),
                decode_value(p[1], encoding),
                decode_value(p[2], encoding),
            ];
            Sample {
                y: luma(rgb).max(1.0e-6),
                lab: linear_to_oklab(rgb),
            }
        })
        .collect();
    (out, sw, sh)
}

fn resize_mask(mask: &GrayImage, w: u32, h: u32) -> Vec<f32> {
    image::imageops::resize(mask, w, h, FilterType::Triangle)
        .pixels()
        .map(|p| p[0] as f32 / 255.0)
        .collect()
}

fn weighted_quantile(values: &mut [(f32, f32)], q: f32) -> f32 {
    values.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f32 = values.iter().map(|v| v.1).sum();
    if total <= 0.0 || values.is_empty() {
        return 0.0;
    }
    let target = q * total;
    let mut acc = 0.0;
    for v in values.iter() {
        acc += v.1;
        if acc >= target {
            return v.0;
        }
    }
    values.last().map(|v| v.0).unwrap_or(0.0)
}

fn chroma_vector(lab: [f32; 3]) -> Option<[f32; 2]> {
    (lab[0] > L_FLOOR).then(|| [lab[1] / lab[0], lab[2] / lab[0]])
}

fn band_stats(samples: &[Sample], weights: &[f32], log_median: f32) -> [BandStats; 3] {
    let mut sums = [[0.0f32; 4]; 3];
    let mut total = 0.0f32;
    for (s, &w) in samples.iter().zip(weights) {
        let Some(c) = chroma_vector(s.lab) else {
            continue;
        };
        let bw = band_weights(s.y.log2() - log_median);
        // the cast is read from the near-neutral pixels: a field of green grass is not a green cast
        let neutral = 1.0 / (1.0 + (c[0] * c[0] + c[1] * c[1]) / (NEUTRAL_CHROMA * NEUTRAL_CHROMA));
        for b in 0..3 {
            let k = bw[b] * w;
            sums[b][0] += c[0] * k * neutral;
            sums[b][1] += c[1] * k * neutral;
            sums[b][2] += k * neutral;
            sums[b][3] += k;
        }
        total += w;
    }
    let means: Vec<[f32; 2]> = sums
        .iter()
        .map(|s| {
            if s[2] > 1.0e-6 {
                [s[0] / s[2], s[1] / s[2]]
            } else {
                [0.0, 0.0]
            }
        })
        .collect();
    let mut spread = [[0.0f32; 2]; 3];
    for (s, &w) in samples.iter().zip(weights) {
        let Some(c) = chroma_vector(s.lab) else {
            continue;
        };
        let bw = band_weights(s.y.log2() - log_median);
        for b in 0..3 {
            let k = bw[b] * w;
            let d = ((c[0] - means[b][0]).powi(2) + (c[1] - means[b][1]).powi(2)).sqrt();
            spread[b][0] += d * k;
            spread[b][1] += k;
        }
    }
    std::array::from_fn(|b| BandStats {
        mean: means[b],
        spread: if spread[b][1] > 1.0e-6 {
            spread[b][0] / spread[b][1]
        } else {
            0.0
        },
        weight: if total > 0.0 { sums[b][3] / total } else { 0.0 },
    })
}

fn palette(samples: &[Sample], k: usize) -> Vec<PaletteColor> {
    let points: Vec<([f32; 3], f32)> = samples
        .iter()
        .step_by((samples.len() / 6000).max(1))
        .filter(|s| s.lab[0] > 0.03)
        .map(|s| {
            let chroma = (s.lab[1] * s.lab[1] + s.lab[2] * s.lab[2]).sqrt();
            (s.lab, 0.25 + chroma * 10.0)
        })
        .collect();
    if points.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<&([f32; 3], f32)> = points.iter().collect();
    sorted.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));
    let mut centers: Vec<[f32; 3]> = (0..k)
        .map(|i| sorted[((i as f32 + 0.5) / k as f32 * sorted.len() as f32) as usize % sorted.len()].0)
        .collect();
    let mut weights = vec![0.0f32; k];
    for _ in 0..10 {
        let mut acc = vec![[0.0f32; 4]; k];
        for (p, w) in &points {
            let best = (0..k)
                .min_by(|&a, &b| dist2(p, &centers[a]).total_cmp(&dist2(p, &centers[b])))
                .unwrap_or(0);
            acc[best][0] += p[0] * w;
            acc[best][1] += p[1] * w;
            acc[best][2] += p[2] * w;
            acc[best][3] += w;
        }
        for i in 0..k {
            if acc[i][3] > 0.0 {
                centers[i] = [acc[i][0] / acc[i][3], acc[i][1] / acc[i][3], acc[i][2] / acc[i][3]];
            }
            weights[i] = acc[i][3];
        }
    }
    let total: f32 = weights.iter().sum::<f32>().max(1.0e-6);
    let mut out: Vec<PaletteColor> = centers
        .iter()
        .zip(&weights)
        .filter(|(_, w)| **w > 0.0)
        .map(|(c, w)| {
            let rgb = oklab_to_linear(*c);
            PaletteColor {
                rgb: [linear_to_srgb(rgb[0]), linear_to_srgb(rgb[1]), linear_to_srgb(rgb[2])],
                weight: w / total,
            }
        })
        .collect();
    out.sort_by(|a, b| luma(a.rgb).total_cmp(&luma(b.rgb)));
    out
}

fn dist2(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    // lightness counts less than hue, so the palette separates colours rather than tones
    0.35 * (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// Statistics of `image` (encoded as `encoding`), with optional region masks of any size.
pub fn analyze(
    image: &DynamicImage,
    encoding: Encoding,
    masks: &Regions<Option<GrayImage>>,
    with_palette: bool,
) -> ImageStats {
    let (samples, w, h) = samples(image, encoding);
    let ones = vec![1.0f32; samples.len()];

    let mut ys: Vec<(f32, f32)> = samples.iter().map(|s| (s.y, 1.0)).collect();
    let median = weighted_quantile(&mut ys, 0.5).max(1.0e-6);
    let log_median = median.log2();
    let log_quantiles = QUANTILES
        .iter()
        .map(|&q| (weighted_quantile(&mut ys, q).max(1.0e-6) / median).log2())
        .collect();
    let display_quantiles = QUANTILES
        .iter()
        .map(|&q| linear_to_srgb(weighted_quantile(&mut ys, q)))
        .collect();

    let chroma = {
        let (mut sum, mut n) = (0.0f32, 0.0f32);
        for s in &samples {
            if let Some(c) = chroma_vector(s.lab) {
                sum += (c[0] * c[0] + c[1] * c[1]).sqrt();
                n += 1.0;
            }
        }
        if n > 0.0 { sum / n } else { 0.0 }
    };

    let region = |mask: &Option<GrayImage>| -> Option<RegionStats> {
        let weights = resize_mask(mask.as_ref()?, w, h);
        let coverage = weights.iter().sum::<f32>() / weights.len().max(1) as f32;
        if coverage < MIN_COVERAGE {
            return None;
        }
        let mut ys: Vec<(f32, f32)> = samples
            .iter()
            .zip(&weights)
            .map(|(s, &wt)| (s.y, wt))
            .collect();
        let region_median = weighted_quantile(&mut ys, 0.5).max(1.0e-6);
        Some(RegionStats {
            coverage,
            rel_key: (region_median / median).log2(),
            bands: band_stats(&samples, &weights, log_median),
        })
    };

    ImageStats {
        log_quantiles,
        display_quantiles,
        bands: band_stats(&samples, &ones, log_median),
        chroma,
        regions: Regions {
            skin: region(&masks.skin),
            sky: region(&masks.sky),
            vegetation: region(&masks.vegetation),
        },
        palette: if with_palette { palette(&samples, 5) } else { Vec::new() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    #[test]
    fn a_grey_picture_has_no_chroma() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 64, |x, _| {
            let v = (x * 4) as u8;
            Rgb([v, v, v])
        }));
        let stats = analyze(&img, Encoding::Srgb, &Regions::default(), true);
        assert!(stats.chroma < 0.01, "{}", stats.chroma);
        assert!(stats.bands.iter().all(|b| b.spread < 0.01));
        assert!(stats.log_quantiles.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn a_warm_picture_has_a_warm_cast() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(64, 64, |x, y| {
            let v = ((x + y) * 2) as u8;
            Rgb([v.saturating_add(40), v.saturating_add(10), v / 2])
        }));
        let stats = analyze(&img, Encoding::Srgb, &Regions::default(), false);
        // warm = positive b (yellow) in OKLab
        assert!(stats.bands[1].mean[1] > 0.05, "{:?}", stats.bands);
    }
}
