//! The neural Smart LUT engine (model trained in training/smart_lut): reference thumbnail + the open photo's
//! thumbnail → an sRGB→sRGB 3D LUT, converted into the analytic engine's domain so the shader and the
//! colour/luminance amounts work unchanged. Kept beside the analytic engine for comparison.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use image::{DynamicImage, GenericImageView, RgbImage, imageops::FilterType};
use ndarray::Array4;
use ort::session::Session;
use ort::value::Tensor;
use rayon::prelude::*;

use super::color::{
    DisplayMode, display_gray, display_inverse, linear_to_srgb, luma, srgb_to_linear, vlog_decode, vlog_encode,
};
use super::compile::{LUT_SIZE, LUT_SLOTS, SmartLutData};

pub const THUMB: u32 = 256;
const NEURAL_SIZE: usize = 33;

/// The model's raw output, (33, 33, 33, 3) indexed [b][g][r][c], sRGB in → sRGB out.
pub type RawLut = Vec<f32>;

/// A model LUT and how bright the photo it was made for looks (median display luma of its thumbnail):
/// the LUT works on the photo as displayed, while the engine works on it normalised to a standard key.
pub struct NeuralLut {
    pub lut: RawLut,
    pub target_median: f32,
}

impl NeuralLut {
    /// `whole` is the whole photo (any size): its median brightness must match the one the engine
    /// normalises by, so it is not the centre-cropped thumbnail the model saw.
    pub fn new(lut: RawLut, whole: &DynamicImage) -> Self {
        let small = whole.resize(256, 256, FilterType::Triangle).to_rgb8();
        let mut ys: Vec<f32> = small
            .pixels()
            .map(|p| luma(p.0.map(|v| srgb_to_linear(v as f32 / 255.0))))
            .collect();
        ys.sort_by(f32::total_cmp);
        NeuralLut {
            lut,
            target_median: linear_to_srgb(ys[ys.len() / 2]).clamp(0.02, 0.98),
        }
    }
}

fn raw_cache() -> &'static Mutex<HashMap<String, Arc<NeuralLut>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<NeuralLut>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn session_slot() -> &'static Mutex<Option<Session>> {
    static SLOT: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Key of a neural LUT: one look on one analysed photo.
pub fn key(look_id: &str, analysis_id: &str) -> String {
    format!("{}:{}", look_id, analysis_id)
}

pub fn cached(key: &str) -> Option<Arc<NeuralLut>> {
    raw_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(key)
        .cloned()
}

/// Centre square of `image`, THUMB×THUMB, as the model was trained on.
pub fn thumbnail(image: &DynamicImage) -> RgbImage {
    let (w, h) = image.dimensions();
    let side = w.min(h).max(1);
    image
        .crop_imm((w - side) / 2, (h - side) / 2, side, side)
        .resize_exact(THUMB, THUMB, FilterType::Triangle)
        .to_rgb8()
}

fn tensor(img: &RgbImage) -> Array4<f32> {
    let mut t = Array4::<f32>::zeros((1, 3, THUMB as usize, THUMB as usize));
    for (x, y, p) in img.enumerate_pixels() {
        for c in 0..3 {
            t[[0, c, y as usize, x as usize]] = p[c] as f32 / 255.0;
        }
    }
    t
}

/// Runs the model and caches its LUT under `key`.
pub fn run(
    app_handle: &tauri::AppHandle,
    key: &str,
    reference: &RgbImage,
    target: &DynamicImage,
) -> Result<(), String> {
    let whole = target;
    let target = &thumbnail(whole);
    let mut slot = session_slot().lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = Some(crate::ai::models::build_smart_lut_session(app_handle).map_err(|e| e.to_string())?);
    }
    let lut = infer(slot.as_mut().unwrap(), reference, target)?;
    raw_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.to_string(), Arc::new(NeuralLut::new(lut, whole)));
    Ok(())
}

/// One run of the model: the raw sRGB LUT.
pub fn infer(session: &mut Session, reference: &RgbImage, target: &RgbImage) -> Result<RawLut, String> {
    let outputs = session
        .run(ort::inputs![
            "reference" => Tensor::from_array(tensor(reference)).map_err(|e| e.to_string())?,
            "target" => Tensor::from_array(tensor(target)).map_err(|e| e.to_string())?,
        ])
        .map_err(|e| e.to_string())?;
    let lut = outputs["lut"]
        .try_extract_array::<f32>()
        .map_err(|e| e.to_string())?
        .iter()
        .copied()
        .collect::<Vec<f32>>();
    if lut.len() != NEURAL_SIZE * NEURAL_SIZE * NEURAL_SIZE * 3 {
        return Err(format!("Unexpected Smart LUT model output ({} values)", lut.len()));
    }
    Ok(lut)
}

fn sample(raw: &RawLut, rgb: [f32; 3]) -> [f32; 3] {
    let n = NEURAL_SIZE;
    let p = rgb.map(|v| v.clamp(0.0, 1.0) * (n - 1) as f32);
    let i = p.map(|v| (v.floor() as usize).min(n - 2));
    let f = [p[0] - i[0] as f32, p[1] - i[1] as f32, p[2] - i[2] as f32];
    let at = |r: usize, g: usize, b: usize, c: usize| raw[((b * n + g) * n + r) * 3 + c];
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

/// The model's sRGB LUT in the engine's domain. A normalised scene value `n` is shown the way the default
/// render shows the photo (display transform at the exposure that puts the median where the thumbnail has
/// it), passed through the model's LUT, and brought back to scene light. An identity LUT therefore gives the
/// default render back. All four slots (whole image and regions) get the same table: the model has no regions.
pub fn to_engine(neural: &NeuralLut, mode: DisplayMode) -> SmartLutData {
    let scale = display_inverse(mode, neural.target_median) / 0.18;
    let size = LUT_SIZE;
    let axis: Vec<f32> = (0..size)
        .map(|i| vlog_decode(i as f32 / (size - 1) as f32))
        .collect();
    let per_slot = size * size * size * 3;
    let mut slot = vec![0.0f32; per_slot];
    slot.par_chunks_mut(size * 3).enumerate().for_each(|(row, chunk)| {
        let (b, g) = (row / size, row % size);
        for r in 0..size {
            let display = [axis[r], axis[g], axis[b]].map(|n| display_gray(mode, n * scale));
            let out = sample(&neural.lut, display);
            for c in 0..3 {
                chunk[r * 3 + c] = vlog_encode(display_inverse(mode, out[c]));
            }
        }
    });
    let mut data = Vec::with_capacity(per_slot * LUT_SLOTS);
    for _ in 0..LUT_SLOTS {
        data.extend_from_slice(&slot);
    }
    SmartLutData { size, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_identity_model_gives_the_standard_rendering() {
        let n = NEURAL_SIZE;
        let mut raw = vec![0.0f32; n * n * n * 3];
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let i = ((b * n + g) * n + r) * 3;
                    raw[i] = r as f32 / (n - 1) as f32;
                    raw[i + 1] = g as f32 / (n - 1) as f32;
                    raw[i + 2] = b as f32 / (n - 1) as f32;
                }
            }
        }
        let neural = NeuralLut { lut: raw, target_median: 0.46 };
        let lut = to_engine(&neural, DisplayMode::BasicSrgb);
        // an identity model returns the default render: n scaled so the median shows at 0.46
        let scale = display_inverse(DisplayMode::BasicSrgb, 0.46) / 0.18;
        for (i, &x) in [6usize, 12, 16].iter().enumerate() {
            let at = ((x * LUT_SIZE + x) * LUT_SIZE + x) * 3;
            let n = vlog_decode(x as f32 / (LUT_SIZE - 1) as f32);
            let o = vlog_decode(lut.data[at]);
            assert!((o - n * scale).abs() < 0.02 * (n * scale) + 1.0e-3, "{i}: {o} vs {}", n * scale);
        }
    }
}
