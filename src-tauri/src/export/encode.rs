use crate::export::settings::{ExportSettings, TiffBitDepth};
use crate::metadata::tiff_metadata::{TiffSamples, encode_tiff_with_metadata};
use crate::metadata::{exif_processing, export_metadata};
use image::codecs::jpeg::{JpegEncoder, PixelDensity};
use image::codecs::png::PngEncoder;
use image::{DynamicImage, GenericImageView, ImageEncoder};
use jxl_encoder::{
    LosslessConfig, LossyConfig, PixelLayout,
    api::{calibrated_jxl_quality, quality_to_distance},
};
use std::borrow::Cow;
#[cfg(not(target_os = "android"))]
use std::fs;
use std::io::Cursor;
use std::path::Path;

pub fn set_timestamps_from_exif(src: &Path, dst: &Path) {
    let capture_dt = exif_processing::get_creation_date_from_path(src);

    if capture_dt.timestamp() <= 0 {
        let now = filetime::FileTime::now();
        if let Err(e) = filetime::set_file_times(dst, now, now) {
            log::warn!("Could not set timestamps on '{}': {}", dst.display(), e);
        }
        return;
    }

    let ft = filetime::FileTime::from_unix_time(
        capture_dt.timestamp(),
        capture_dt.timestamp_subsec_nanos(),
    );
    if let Err(e) = filetime::set_file_times(dst, ft, ft) {
        log::warn!("Could not set timestamps on '{}': {}", dst.display(), e);
    }
}

pub fn save_image_with_metadata(
    image: &DynamicImage,
    output_path: &std::path::Path,
    source_path_str: &str,
    export_xmp: &export_metadata::ExportXmp,
    export_settings: &ExportSettings,
) -> Result<(), String> {
    let extension = output_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let tiff_metadata =
        if export_settings.keep_metadata && matches!(extension.as_str(), "tif" | "tiff") {
            export_metadata::collect_metadata_from_source(
                source_path_str,
                export_settings.strip_gps,
                Some(image.dimensions()),
            )
        } else {
            None
        };

    let mut image_bytes =
        encode_image_to_bytes(image, &extension, export_settings, tiff_metadata.as_ref())?;

    export_metadata::write_image_with_metadata(
        &mut image_bytes,
        source_path_str,
        &extension,
        export_settings.keep_metadata,
        export_settings.strip_gps,
        export_xmp,
        export_settings.dpi,
    )?;

    #[cfg(target_os = "android")]
    {
        let file_name = output_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "Missing Android export file name".to_string())?;
        crate::platform::android::save_image_bytes_to_android_gallery(
            file_name,
            mime_type_for_extension(&extension),
            &image_bytes,
        )?;
    }

    #[cfg(not(target_os = "android"))]
    {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create folder '{}': {}", parent.display(), e))?;
        }
        fs::write(output_path, image_bytes)
            .map_err(|e| format!("Failed to write file to '{}': {}", output_path.display(), e))?;
    }

    Ok(())
}

/// Writes an Ultra HDR JPEG: `sdr` is the base image, `hdr` its linear float rendition.
pub fn save_ultra_hdr(
    sdr: &DynamicImage,
    hdr: &DynamicImage,
    output_path: &std::path::Path,
    source_path_str: &str,
    export_settings: &ExportSettings,
) -> Result<(), String> {
    let source = source_path_str.to_string();
    let (keep, strip_gps, dpi) = (
        export_settings.keep_metadata,
        export_settings.strip_gps,
        export_settings.dpi,
    );
    let write_metadata = move |bytes: &mut Vec<u8>| {
        export_metadata::write_image_with_metadata(
            bytes,
            &source,
            "jpg",
            keep,
            strip_gps,
            &export_metadata::ExportXmp::default(),
            dpi,
        )
    };
    let image_bytes = crate::export::hdr_encode::encode_ultra_hdr(
        sdr,
        hdr,
        export_settings.jpeg_quality,
        Some(&write_metadata),
    )?;

    #[cfg(target_os = "android")]
    {
        let file_name = output_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "Missing Android export file name".to_string())?;
        crate::platform::android::save_image_bytes_to_android_gallery(
            file_name,
            "image/jpeg",
            &image_bytes,
        )?;
    }

    #[cfg(not(target_os = "android"))]
    {
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create folder '{}': {}", parent.display(), e))?;
        }
        fs::write(output_path, image_bytes)
            .map_err(|e| format!("Failed to write file to '{}': {}", output_path.display(), e))?;
    }
    Ok(())
}

#[cfg(target_os = "android")]
pub fn mime_type_for_extension(extension: &str) -> &'static str {
    match extension {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        "tif" | "tiff" => "image/tiff",
        "jxl" => "image/jxl",
        _ => "application/octet-stream",
    }
}

pub(crate) const SRGB_ICC_PROFILE: &[u8] = include_bytes!("../../icc/sRGB-v2-magic.icc");

pub fn encode_image_to_bytes(
    image: &DynamicImage,
    output_format: &str,
    export_settings: &ExportSettings,
    tiff_metadata: Option<&little_exif::metadata::Metadata>,
) -> Result<Vec<u8>, String> {
    let jpeg_quality = export_settings.jpeg_quality;
    let mut image_bytes = Vec::new();
    let mut cursor = Cursor::new(&mut image_bytes);

    match output_format.to_lowercase().as_str() {
        "jxl" => {
            if matches!(image, DynamicImage::ImageRgba32F(_)) {
                return encode_hdr_jxl(image, jpeg_quality);
            }
            let (width, height) = image.dimensions();
            let has_alpha = image.color().has_alpha();

            let jxl_data = if jpeg_quality == 100 {
                let config = LosslessConfig::new().with_tree_learning(false);
                if has_alpha {
                    let rgba = image.to_rgba8();
                    config
                        .encode(rgba.as_raw(), width, height, PixelLayout::Rgba8)
                        .map_err(|e| format!("Failed to encode lossless JXL: {}", e))?
                } else {
                    let rgb = image.to_rgb8();
                    config
                        .encode(rgb.as_raw(), width, height, PixelLayout::Rgb8)
                        .map_err(|e| format!("Failed to encode lossless JXL: {}", e))?
                }
            } else {
                let jxl_quality = calibrated_jxl_quality(jpeg_quality as f32);
                let distance = quality_to_distance(jxl_quality);

                if has_alpha {
                    let rgba = image.to_rgba8();
                    LossyConfig::new(distance)
                        .encode(rgba.as_raw(), width, height, PixelLayout::Rgba8)
                        .map_err(|e| format!("Failed to encode lossy JXL: {}", e))?
                } else {
                    let rgb = image.to_rgb8();
                    LossyConfig::new(distance)
                        .encode(rgb.as_raw(), width, height, PixelLayout::Rgb8)
                        .map_err(|e| format!("Failed to encode lossy JXL: {}", e))?
                }
            };

            return Ok(jxl_data);
        }
        "webp" => {
            let encoder = webp::Encoder::from_image(image)
                .map_err(|_| "Failed to create WebP encoder".to_string())?;
            let webp_mem = encoder.encode(jpeg_quality as f32);
            return Ok(webp_mem.to_vec());
        }
        "jpg" | "jpeg" => {
            let rgb_image = image
                .as_rgb8()
                .map_or_else(|| Cow::Owned(image.to_rgb8()), Cow::Borrowed);
            let mut encoder = JpegEncoder::new_with_quality(&mut cursor, jpeg_quality);
            encoder
                .set_icc_profile(SRGB_ICC_PROFILE.to_vec())
                .map_err(|e| e.to_string())?;
            if let Some(dpi) = export_settings.dpi {
                encoder.set_pixel_density(PixelDensity::dpi(dpi));
            }
            rgb_image
                .write_with_encoder(encoder)
                .map_err(|e| e.to_string())?;
        }
        "png" => {
            if matches!(image, DynamicImage::ImageRgba32F(_)) {
                return crate::export::hdr_encode::encode_hdr_png(
                    image,
                    export_settings.hdr_transfer,
                    export_settings.dpi,
                );
            }
            let converted;
            let image_to_encode = if image.as_rgb32f().is_some() {
                converted = DynamicImage::ImageRgb16(image.to_rgb16());
                &converted
            } else {
                image
            };

            let mut encoder = PngEncoder::new(&mut cursor);
            encoder
                .set_icc_profile(SRGB_ICC_PROFILE.to_vec())
                .map_err(|e| e.to_string())?;
            image_to_encode
                .write_with_encoder(encoder)
                .map_err(|e| e.to_string())?;
            if let Some(dpi) = export_settings.dpi {
                insert_png_phys_chunk(&mut image_bytes, dpi);
            }
        }
        "tif" | "tiff" => {
            // an HDR edit is saved as 32-bit float, linear light with 1.0 at SDR white (no profile: sRGB
            // would mislabel linear values)
            if matches!(image, DynamicImage::ImageRgba32F(_)) {
                return encode_tiff_with_metadata(
                    image,
                    TiffSamples::ThirtyTwoFloat,
                    tiff_metadata,
                    None,
                    export_settings.dpi,
                );
            }
            let samples = match export_settings.tiff_bit_depth {
                TiffBitDepth::Eight => TiffSamples::Eight,
                TiffBitDepth::Sixteen => TiffSamples::Sixteen,
            };
            return encode_tiff_with_metadata(
                image,
                samples,
                tiff_metadata,
                Some(SRGB_ICC_PROFILE),
                export_settings.dpi,
            );
        }
        "avif" => {
            if matches!(image, DynamicImage::ImageRgba32F(_)) {
                return crate::export::hdr_encode::encode_hdr_avif(
                    image,
                    export_settings.hdr_transfer,
                    jpeg_quality,
                );
            }
            image
                .write_to(&mut cursor, image::ImageFormat::Avif)
                .map_err(|e| e.to_string())?;
        }
        _ => return Err(format!("Unsupported file format: {}", output_format)),
    };
    Ok(image_bytes)
}

/// HDR JPEG XL: linear float RGB where 1.0 is the brightest pixel, with the intensity target set so that
/// SDR white (value 1.0 of the HDR render) stays at 203 nits.
fn encode_hdr_jxl(image: &DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    use jxl_encoder::ImageMetadata;
    const SDR_WHITE_NITS: f32 = 203.0;

    let (width, height) = image.dimensions();
    let rgb = image.to_rgb32f();
    let peak = rgb
        .as_raw()
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(1.0f32, f32::max)
        .min(50.0);
    let scale = 1.0 / peak;
    let mut bytes = Vec::with_capacity(rgb.as_raw().len() * 4);
    for value in rgb.as_raw() {
        let value = if value.is_finite() { (value * scale).clamp(0.0, 1.0) } else { 0.0 };
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    let metadata = ImageMetadata::new()
        .with_intensity_target((SDR_WHITE_NITS * peak).clamp(255.0, 10_000.0));
    let distance = if quality >= 100 {
        0.3
    } else {
        quality_to_distance(calibrated_jxl_quality(quality as f32))
    };
    let config = LossyConfig::new(distance);
    config
        .encode_request(width, height, PixelLayout::RgbLinearF32)
        .with_metadata(&metadata)
        .encode(&bytes)
        .map_err(|e| format!("Failed to encode HDR JXL: {}", e))
}

fn insert_png_phys_chunk(image_bytes: &mut Vec<u8>, dpi: u16) {
    let pixels_per_meter = (f64::from(dpi) / 0.0254).round() as u32;
    let mut data = Vec::with_capacity(9);
    data.extend_from_slice(&pixels_per_meter.to_be_bytes());
    data.extend_from_slice(&pixels_per_meter.to_be_bytes());
    data.push(1);
    export_metadata::insert_png_chunk(image_bytes, b"pHYs", &data);
}

#[cfg(test)]
mod hdr_tests {
    use super::*;

    #[test]
    fn hdr_jxl_encodes_values_above_sdr_white() {
        let mut img = image::Rgba32FImage::new(16, 16);
        for (i, px) in img.pixels_mut().enumerate() {
            let v = 0.25 + (i % 16) as f32 * 0.4;
            *px = image::Rgba([v, v * 0.9, v * 0.8, 1.0]);
        }
        let bytes = encode_hdr_jxl(&DynamicImage::ImageRgba32F(img), 90).expect("HDR JXL encodes");
        assert!(bytes.len() > 20);
        assert!(bytes.starts_with(&[0xFF, 0x0A]) || bytes.starts_with(&[0x00, 0x00, 0x00, 0x0C]));
    }
}
