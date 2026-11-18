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

use super::color::{DisplayMode, display_inverse, linear_to_srgb, vlog_decode, vlog_encode};
use super::compile::{LUT_SIZE, LUT_SLOTS, SmartLutData};

pub const THUMB: u32 = 256;
const NEURAL_SIZE: usize = 33;

/// The model's raw output, (33, 33, 33, 3) indexed [b][g][r][c], sRGB in → sRGB out.
pub type RawLut = Vec<f32>;

fn raw_cache() -> &'static Mutex<HashMap<String, Arc<RawLut>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<RawLut>>>> = OnceLock::new();
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

pub fn cached(key: &str) -> Option<Arc<RawLut>> {
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
    target: &RgbImage,
) -> Result<(), String> {
    let mut slot = session_slot().lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = Some(crate::ai::models::build_smart_lut_session(app_handle).map_err(|e| e.to_string())?);
    }
    let session = slot.as_mut().unwrap();
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
    raw_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.to_string(), Arc::new(lut));
    Ok(())
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

/// The same neutral rendering the analytic engine starts from (scene light → display, per channel).
fn standard_display(x: f32) -> f32 {
    let x = (x * 0.75).max(0.0);
    let aces = (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14);
    linear_to_srgb(aces.clamp(0.0, 1.0))
}

/// The model's sRGB LUT in the engine's domain: normalised scene light → standard display → model →
/// back to the scene light the shader's display transform turns into that picture. All four slots (whole
/// image and regions) get the same table: the model has no regions.
pub fn to_engine(raw: &RawLut, mode: DisplayMode) -> SmartLutData {
    let size = LUT_SIZE;
    let axis: Vec<f32> = (0..size)
        .map(|i| vlog_decode(i as f32 / (size - 1) as f32))
        .collect();
    let per_slot = size * size * size * 3;
    let mut slot = vec![0.0f32; per_slot];
    slot.par_chunks_mut(size * 3).enumerate().for_each(|(row, chunk)| {
        let (b, g) = (row / size, row % size);
        for r in 0..size {
            let display = [axis[r], axis[g], axis[b]].map(standard_display);
            let out = sample(raw, display);
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
        let lut = to_engine(&raw, DisplayMode::BasicSrgb);
        // a mid grey stays a grey
        let mid = LUT_SIZE / 2;
        let i = ((mid * LUT_SIZE + mid) * LUT_SIZE + mid) * 3;
        let rgb = [lut.data[i], lut.data[i + 1], lut.data[i + 2]].map(vlog_decode);
        assert!((rgb[0] - rgb[1]).abs() < 1.0e-3 && (rgb[2] - rgb[1]).abs() < 1.0e-3, "{rgb:?}");
    }
}
