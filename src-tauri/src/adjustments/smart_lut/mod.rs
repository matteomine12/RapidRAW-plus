//! Smart LUT: a look extracted from a reference picture, a colour or a palette, applied under the edit and
//! adapted to each photo — its tones, its colours per tonal band, and separately its skin, sky and vegetation.

pub mod color;
pub mod compile;
pub mod look;
pub mod render;
pub mod stats;

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose};
use image::{DynamicImage, GenericImageView, GrayImage, ImageFormat, imageops::FilterType};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use crate::ai::commands::{MaskTransform, ai_cache_key, smart_lut_region_masks};
use crate::app::state::AppState;
use crate::render::preview::get_cached_full_warped_image;
use look::SmartLook;
use stats::{Encoding, Regions, analyze};

/// Longest edge the reference picture is analysed at.
const REFERENCE_EDGE: u32 = 1536;
/// Region masks are softened over this fraction of the image's longest edge.
const MASK_SOFTNESS: f32 = 0.006;

fn looks_dir(app_handle: &AppHandle) -> Result<PathBuf, String> {
    let dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("smart_luts");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn look_path(dir: &Path, id: &str) -> Result<PathBuf, String> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("Invalid Smart LUT id".to_string());
    }
    Ok(dir.join(format!("{}.json", id)))
}

fn save_look(app_handle: &AppHandle, look: &SmartLook) -> Result<(), String> {
    let path = look_path(&looks_dir(app_handle)?, &look.id)?;
    let text = serde_json::to_string_pretty(look).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_smart_luts(app_handle: AppHandle) -> Result<Vec<SmartLook>, String> {
    let dir = looks_dir(&app_handle)?;
    let mut looks: Vec<SmartLook> = fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                return None;
            }
            serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()
        })
        .collect();
    looks.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(looks)
}

#[tauri::command]
pub fn delete_smart_lut(id: String, app_handle: AppHandle) -> Result<(), String> {
    let path = look_path(&looks_dir(&app_handle)?, &id)?;
    if path.exists() {
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn rename_smart_lut(id: String, name: String, app_handle: AppHandle) -> Result<SmartLook, String> {
    let path = look_path(&looks_dir(&app_handle)?, &id)?;
    let mut look: SmartLook =
        serde_json::from_str(&fs::read_to_string(&path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    look.name = name.trim().to_string();
    save_look(&app_handle, &look)?;
    Ok(look)
}

#[tauri::command]
pub fn create_smart_lut_from_colors(
    colors: Vec<[f32; 3]>,
    name: String,
    app_handle: AppHandle,
) -> Result<SmartLook, String> {
    let colors: Vec<[f32; 3]> = colors
        .into_iter()
        .take(8)
        .map(|c| c.map(|v| v.clamp(0.0, 1.0)))
        .collect();
    let look = look::from_colors(name.trim().to_string(), &colors).ok_or("No colours given")?;
    save_look(&app_handle, &look)?;
    Ok(look)
}

fn load_reference(path: &str) -> Result<DynamicImage, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let image = if crate::decode::formats::is_raw_file(path) {
        crate::decode::image_loader::safe_embedded_preview_fallback(&bytes, path)
            .ok_or("This RAW has no embedded preview to read the look from")?
    } else {
        crate::decode::image_loader::load_image_with_orientation(&bytes, None)
            .map_err(|e| e.to_string())?
    };
    let (w, h) = image.dimensions();
    let scale = REFERENCE_EDGE as f32 / w.max(h) as f32;
    Ok(if scale < 1.0 {
        DynamicImage::ImageRgb8(
            image
                .resize(
                    (w as f32 * scale).round() as u32,
                    (h as f32 * scale).round() as u32,
                    FilterType::Triangle,
                )
                .to_rgb8(),
        )
    } else {
        DynamicImage::ImageRgb8(image.to_rgb8())
    })
}

/// Softens a region mask so the looks blend without seams (blurred at a reduced size).
fn soften(mask: &GrayImage) -> GrayImage {
    let (w, h) = mask.dimensions();
    let edge = w.max(h) as f32;
    let scale = (768.0 / edge).min(1.0);
    let (sw, sh) = (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
    let small = image::imageops::resize(mask, sw, sh, FilterType::Triangle);
    let sigma = (sw.max(sh) as f32 * MASK_SOFTNESS).max(0.8);
    let blurred = imageproc::filter::gaussian_blur_f32(&small, sigma);
    image::imageops::resize(&blurred, w, h, FilterType::Triangle)
}

#[tauri::command]
pub async fn create_smart_lut_from_image(
    path: String,
    name: Option<String>,
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<SmartLook, String> {
    let reference = load_reference(&path)?;
    let cache_key = format!("smart-lut-reference:{}", blake3::hash(path.as_bytes()).to_hex());
    let masks = smart_lut_region_masks(&app_handle, &state, &reference, &cache_key, 0).await?;
    let masks = masks.map(|m| m.map(|m| soften(&m)));
    let stats = analyze(&reference, Encoding::Srgb, &masks, true);
    let name = name
        .filter(|n| !n.trim().is_empty())
        .or_else(|| {
            Path::new(&path)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "Smart LUT".to_string());
    let look = look::from_reference(name, &stats);
    save_look(&app_handle, &look)?;
    Ok(look)
}

fn encode_png(mask: &GrayImage) -> Result<String, String> {
    let mut buf = Cursor::new(Vec::new());
    mask.write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "data:image/png;base64,{}",
        general_purpose::STANDARD.encode(buf.get_ref())
    ))
}

/// Reads the open photo for the Smart LUT: its statistics and its skin, sky and vegetation masks. The
/// result goes into the edit as `smartLut.analysis`.
#[tauri::command]
pub async fn analyze_smart_lut_target(
    js_adjustments: Value,
    path: String,
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<Value, String> {
    let loaded = state
        .original_image
        .lock()
        .unwrap()
        .clone()
        .ok_or("No image loaded")?;
    let warped = get_cached_full_warped_image(&state, &js_adjustments)?;
    let transform = MaskTransform::from_adjustments(&js_adjustments);
    let masks = smart_lut_region_masks(
        &app_handle,
        &state,
        &warped,
        &ai_cache_key(&path, &js_adjustments),
        transform.orientation_steps,
    )
    .await?;
    let masks = masks.map(|m| m.map(|m| soften(&m)));
    let encoding = if crate::adjustments::negative_conversion::effective_is_raw(
        loaded.is_raw,
        &js_adjustments,
    ) {
        Encoding::RawPreview
    } else {
        Encoding::Srgb
    };
    let stats = analyze(&warped, encoding, &masks, false);

    let present = [
        stats.regions.skin.is_some(),
        stats.regions.sky.is_some(),
        stats.regions.vegetation.is_some(),
    ];
    let encode = |mask: &Option<GrayImage>, keep: bool| -> Result<Value, String> {
        Ok(match mask {
            Some(mask) if keep => json!({
                "maskDataBase64": encode_png(mask)?,
                "rotation": transform.rotation,
                "flipHorizontal": transform.flip_horizontal,
                "flipVertical": transform.flip_vertical,
                "orientationSteps": transform.orientation_steps,
            }),
            _ => Value::Null,
        })
    };
    let Regions {
        skin,
        sky,
        vegetation,
    } = &masks;
    Ok(json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "stats": stats,
        "regions": {
            "skin": encode(skin, present[0])?,
            "sky": encode(sky, present[1])?,
            "vegetation": encode(vegetation, present[2])?,
        },
    }))
}
