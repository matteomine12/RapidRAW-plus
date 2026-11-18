//! Hooks the Smart LUT into a render: compiles (and caches) the LUTs for the edit's look and analysis,
//! appends the region weights after the user's mask layers and fills the shader uniforms.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use base64::{Engine as _, engine::general_purpose};
use image::GrayImage;
use serde_json::Value;

use super::color::DisplayMode;
use super::compile::{SmartLutData, compile};
use super::look::SmartLook;
use super::neural;
use super::stats::ImageStats;
use crate::masks::transform::{TransformParams, transform_full_mask};
use crate::render::adjustment_utils::is_tool_active;
use crate::render::uniforms::{AllAdjustments, MAX_MASKS};

pub const REGION_NAMES: [&str; 3] = ["skin", "sky", "vegetation"];
const COMPILED_CACHE: usize = 4;
const MASK_CACHE: usize = 8;

type Cache<T> = Mutex<Vec<(u64, Arc<T>)>>;

fn compiled_cache() -> &'static Cache<SmartLutData> {
    static CACHE: OnceLock<Cache<SmartLutData>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn decoded_cache() -> &'static Cache<GrayImage> {
    static CACHE: OnceLock<Cache<GrayImage>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn placed_cache() -> &'static Cache<GrayImage> {
    static CACHE: OnceLock<Cache<GrayImage>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

fn cached<T>(cache: &Cache<T>, key: u64, limit: usize, make: impl FnOnce() -> Option<T>) -> Option<Arc<T>> {
    if let Some((_, hit)) = cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(k, _)| *k == key)
    {
        return Some(Arc::clone(hit));
    }
    let value = Arc::new(make()?);
    let mut entries = cache.lock().unwrap_or_else(|e| e.into_inner());
    if entries.len() >= limit {
        entries.remove(0);
    }
    entries.push((key, Arc::clone(&value)));
    Some(value)
}

fn hash_of(parts: &[&dyn Fn(&mut DefaultHasher)]) -> u64 {
    let mut h = DefaultHasher::new();
    for p in parts {
        p(&mut h);
    }
    h.finish()
}

/// The Smart LUT part of an edit, when it is on and has been analysed for this photo.
pub fn is_active(js: &Value) -> bool {
    is_tool_active(js, "smartLut")
        && js["smartLut"]["look"].is_object()
        && js["smartLut"]["analysis"]["stats"].is_object()
}

fn decode_mask(data_url: &str) -> Option<GrayImage> {
    let b64 = data_url.find(',').map_or(data_url, |i| &data_url[i + 1..]);
    let bytes = general_purpose::STANDARD.decode(b64).ok()?;
    Some(image::load_from_memory(&bytes).ok()?.to_luma8())
}

fn place_mask(params: &Value, width: u32, height: u32, scale: f32, crop_offset: (f32, f32)) -> Option<GrayImage> {
    let data = params["maskDataBase64"].as_str()?;
    let data_key = hash_of(&[&|h| data.hash(h)]);
    let tf = TransformParams {
        rotation: params["rotation"].as_f64().unwrap_or(0.0) as f32,
        flip_horizontal: params["flipHorizontal"].as_bool().unwrap_or(false),
        flip_vertical: params["flipVertical"].as_bool().unwrap_or(false),
        orientation_steps: params["orientationSteps"].as_u64().unwrap_or(0) as u8,
        width,
        height,
        scale,
        crop_offset,
    };
    let placed_key = hash_of(&[
        &|h| data_key.hash(h),
        &|h| (width, height, scale.to_bits(), crop_offset.0.to_bits(), crop_offset.1.to_bits()).hash(h),
        &|h| (tf.rotation.to_bits(), tf.flip_horizontal, tf.flip_vertical, tf.orientation_steps).hash(h),
    ]);
    let placed = cached(placed_cache(), placed_key, MASK_CACHE, || {
        let full = cached(decoded_cache(), data_key, MASK_CACHE, || decode_mask(data))?;
        Some(transform_full_mask(&full, &tf, false))
    })?;
    Some((*placed).clone())
}

/// What a render needs from the Smart LUT.
pub struct Attached {
    pub lut: Arc<SmartLutData>,
    /// Changes whenever the LUT or the region layers change (for mask texture caching).
    pub key: u64,
}

/// Sets up the Smart LUT for a render of `width`×`height` (mask space `scale`, `crop_offset`): compiles
/// the LUTs, appends the region layers to `masks` and fills the uniforms. `None` when the edit has none.
pub fn attach(
    js: &Value,
    adjustments: &mut AllAdjustments,
    masks: &mut Vec<GrayImage>,
    width: u32,
    height: u32,
    scale: f32,
    crop_offset: (f32, f32),
) -> Option<Attached> {
    adjustments.global.smart_lut_enabled = 0;
    if !is_active(js) {
        return None;
    }
    let smart = &js["smartLut"];
    let mode = DisplayMode::from_uniforms(
        adjustments.global.is_raw_image == 1,
        adjustments.global.tonemapper_mode,
    );
    let look_value = &smart["look"];
    let stats_value = &smart["analysis"]["stats"];
    let lut_key = hash_of(&[
        &|h| look_value.to_string().hash(h),
        &|h| stats_value.to_string().hash(h),
        &|h| mode.hash(h),
    ]);
    // the neural engine, when chosen and already run for this look on this photo
    let neural = (smart["engine"].as_str() == Some("neural"))
        .then(|| {
            neural::cached(&neural::key(
                look_value["id"].as_str()?,
                smart["analysis"]["id"].as_str()?,
            ))
        })
        .flatten();
    let lut_key = match &neural {
        Some(raw) => hash_of(&[&|h| lut_key.hash(h), &|h| (Arc::as_ptr(raw) as usize).hash(h)]),
        None => lut_key,
    };
    let lut = cached(compiled_cache(), lut_key, COMPILED_CACHE, || match &neural {
        Some(raw) => Some(neural::to_engine(raw, mode)),
        None => {
            let look: SmartLook = serde_json::from_value(look_value.clone()).ok()?;
            let stats: ImageStats = serde_json::from_value(stats_value.clone()).ok()?;
            Some(compile(&look, &stats, mode))
        }
    })?;

    let amount = |key: &str| (smart[key].as_f64().unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 2.0);
    let g = &mut adjustments.global;
    g.smart_lut_enabled = 1;
    g.smart_color = amount("color");
    g.smart_luminance = amount("luminance");
    g.smart_regions = 0;
    g.smart_region_start = masks.len() as u32;

    let regions = &smart["analysis"]["regions"];
    let wanted: Vec<(usize, &Value)> = REGION_NAMES
        .iter()
        .enumerate()
        .filter(|(_, name)| smart["analysis"]["stats"]["regions"][**name].is_object())
        .filter_map(|(i, name)| {
            let params = &regions[*name];
            params["maskDataBase64"].as_str().filter(|s| !s.is_empty()).map(|_| (i, params))
        })
        .collect();
    let mut region_key = 0u64;
    if neural.is_none() && !wanted.is_empty() && masks.len() + wanted.len() <= MAX_MASKS {
        for (bit, params) in wanted {
            let mask = place_mask(params, width, height, scale, crop_offset)
                .unwrap_or_else(|| GrayImage::new(width, height));
            region_key = hash_of(&[
                &|h| region_key.hash(h),
                &|h| params["maskDataBase64"].as_str().unwrap_or_default().hash(h),
                &|h| bit.hash(h),
            ]);
            masks.push(mask);
            adjustments.global.smart_regions |= 1 << bit;
        }
    }
    Some(Attached {
        key: hash_of(&[
            &|h| lut_key.hash(h),
            &|h| region_key.hash(h),
            &|h| (width, height, scale.to_bits(), crop_offset.0.to_bits(), crop_offset.1.to_bits()).hash(h),
        ]),
        lut,
    })
}

/// `attach` for call sites that only need the LUT.
pub fn attach_lut(
    js: &Value,
    adjustments: &mut AllAdjustments,
    masks: &mut Vec<GrayImage>,
    width: u32,
    height: u32,
    scale: f32,
    crop_offset: (f32, f32),
) -> Option<Arc<SmartLutData>> {
    attach(js, adjustments, masks, width, height, scale, crop_offset).map(|a| a.lut)
}
