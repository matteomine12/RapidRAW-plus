//! The Smart LUT recipe: what a look wants (tones, colour per tonal band, per region, palette), extracted
//! from a reference picture or built from one or more colours. It is image-independent; `compile` turns
//! it into LUTs for a given photo.

use serde::{Deserialize, Serialize};

use super::color::{linear_to_oklab, srgb_to_linear};
use super::stats::{BandStats, ImageStats, PaletteColor, RegionStats, Regions};

pub const LOOK_VERSION: u32 = 1;
/// Below this mean |(a/L, b/L)| a reference counts as black and white.
const MONOCHROME_CHROMA: f32 = 0.018;
/// Below this a colour is a grey.
const GREY_CHROMA: f32 = 0.03;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LookKind {
    Image,
    Color,
    Palette,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ToneSpec {
    /// Match the reference's on-screen luma distribution.
    #[serde(rename_all = "camelCase")]
    Quantiles { display: Vec<f32> },
    /// A standard rendering shifted by `key_shift` stops with `contrast` around the median.
    #[serde(rename_all = "camelCase")]
    Parametric { key_shift: f32, contrast: f32 },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SmartLook {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub kind: LookKind,
    pub created_at: i64,
    pub palette: Vec<PaletteColor>,
    pub monochrome: bool,
    pub tone: ToneSpec,
    pub bands: [BandStats; 3],
    /// Fixed chroma factor (looks built from colours); `None` = ratio of the band spreads.
    pub saturation: Option<f32>,
    pub regions: Regions<Option<RegionStats>>,
    /// How strongly hues are pulled toward the palette (0..1).
    pub hue_pull: f32,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// A look extracted from a reference picture's statistics.
pub fn from_reference(name: String, stats: &ImageStats) -> SmartLook {
    let monochrome = stats.chroma < MONOCHROME_CHROMA;
    let (bands, regions) = if monochrome {
        let zero = |mut b: BandStats| {
            b.mean = [0.0, 0.0];
            b.spread = 0.0;
            b
        };
        (
            stats.bands.map(zero),
            stats.regions.clone().map(|r| {
                r.map(|mut r| {
                    r.bands = r.bands.map(zero);
                    r
                })
            }),
        )
    } else {
        (stats.bands, stats.regions.clone())
    };
    SmartLook {
        version: LOOK_VERSION,
        id: new_id(),
        name,
        kind: LookKind::Image,
        created_at: now_ms(),
        palette: stats.palette.clone(),
        monochrome,
        tone: ToneSpec::Quantiles {
            display: stats.display_quantiles.clone(),
        },
        bands,
        saturation: None,
        regions,
        hue_pull: if monochrome {
            0.0
        } else {
            (stats.chroma * 4.0).clamp(0.2, 0.6)
        },
    }
}

/// A look built from one colour or a palette (display sRGB, 0..1).
pub fn from_colors(name: String, colors: &[[f32; 3]]) -> Option<SmartLook> {
    if colors.is_empty() {
        return None;
    }
    let labs: Vec<[f32; 3]> = colors
        .iter()
        .map(|c| linear_to_oklab([srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2])]))
        .collect();
    let rel = |lab: &[f32; 3]| -> [f32; 2] {
        let l = lab[0].max(0.05);
        [lab[1] / l, lab[2] / l]
    };
    let norm = |c: [f32; 2]| (c[0] * c[0] + c[1] * c[1]).sqrt();
    let mean_l = labs.iter().map(|l| l[0]).sum::<f32>() / labs.len() as f32;
    let min_l = labs.iter().map(|l| l[0]).fold(f32::MAX, f32::min);
    let max_l = labs.iter().map(|l| l[0]).fold(f32::MIN, f32::max);
    let chromas: Vec<f32> = labs.iter().map(|l| norm(rel(l))).collect();
    let mean_chroma = chromas.iter().sum::<f32>() / chromas.len() as f32;
    let monochrome = chromas.iter().all(|&c| c < GREY_CHROMA);

    let mut order: Vec<usize> = (0..labs.len()).collect();
    order.sort_by(|&a, &b| labs[a][0].total_cmp(&labs[b][0]));
    let darkest = rel(&labs[order[0]]);
    let lightest = rel(&labs[*order.last().unwrap()]);
    let average = {
        let (mut a, mut b) = (0.0, 0.0);
        for l in &labs {
            let c = rel(l);
            a += c[0];
            b += c[1];
        }
        [a / labs.len() as f32, b / labs.len() as f32]
    };
    let scale = |c: [f32; 2], k: f32| [c[0] * k, c[1] * k];
    let single = colors.len() == 1;
    let band = |mean: [f32; 2]| BandStats {
        mean: if monochrome { [0.0, 0.0] } else { mean },
        spread: 0.0,
        weight: 1.0 / 3.0,
    };
    let bands = if single {
        [band(scale(average, 0.3)), band(scale(average, 0.22)), band(scale(average, 0.12))]
    } else {
        [band(scale(darkest, 0.35)), band(scale(average, 0.25)), band(scale(lightest, 0.2))]
    };

    let kind = if single { LookKind::Color } else { LookKind::Palette };
    Some(SmartLook {
        version: LOOK_VERSION,
        id: new_id(),
        name,
        kind,
        created_at: now_ms(),
        palette: colors
            .iter()
            .map(|c| PaletteColor {
                rgb: *c,
                weight: 1.0 / colors.len() as f32,
            })
            .collect(),
        monochrome,
        tone: ToneSpec::Parametric {
            key_shift: ((mean_l - 0.62) * if single { 1.5 } else { 2.0 }).clamp(-0.8, 0.8),
            contrast: if monochrome {
                1.15
            } else {
                (1.0 + (max_l - min_l) * 0.3).clamp(1.0, 1.25)
            },
        },
        bands,
        saturation: Some(if monochrome {
            0.0
        } else {
            (0.9 + mean_chroma * 0.6).clamp(0.8, 1.15)
        }),
        regions: Regions::default(),
        hue_pull: if monochrome {
            0.0
        } else if single {
            0.25
        } else {
            0.45
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grey_colour_makes_a_black_and_white_look() {
        let look = from_colors("g".into(), &[[0.5, 0.5, 0.5]]).unwrap();
        assert!(look.monochrome);
        assert_eq!(look.saturation, Some(0.0));
    }

    #[test]
    fn a_teal_colour_tints_toward_teal() {
        let look = from_colors("t".into(), &[[0.1, 0.5, 0.55]]).unwrap();
        assert!(!look.monochrome);
        // teal: negative a (green) and negative b (blue)
        assert!(look.bands[0].mean[0] < 0.0 && look.bands[0].mean[1] < 0.0);
    }
}
