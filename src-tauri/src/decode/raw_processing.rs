use crate::decode::as_shot::AsShot;
use crate::decode::demosaic::Mosaic;
use crate::decode::develop::{self, Camera, Finish};
use crate::decode::white_balance::WhiteBalance;
use crate::render::geometry::apply_orientation;
use anyhow::{Result, anyhow};
use image::{DynamicImage, ImageBuffer, Rgb};
use rawler::{
    decoders::{Decoder, Orientation, RawDecodeParams},
    imgop::{
        Dim2, Point, Rect,
        develop::{DemosaicAlgorithm, Intermediate, ProcessingStep, RawDevelop},
        xyz::{FlatColorMatrix, Illuminant},
    },
    pixarray::Color2D,
    rawimage::{RawImage, RawPhotometricInterpretation},
    rawsource::RawSource,
};
use rayon::prelude::*;
use std::borrow::Cow;
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn metadata_orientation(decoder: &dyn Decoder, source: &RawSource) -> Result<Orientation> {
    let metadata = decoder.raw_metadata(source, &RawDecodeParams::default())?;
    Ok(metadata
        .exif
        .orientation
        .map(Orientation::from_u16)
        .unwrap_or(Orientation::Normal))
}

const HIGHLIGHT_HEADROOM: f32 = 1000.0;

thread_local! {
    static READING_PREFIX: Cell<bool> = const { Cell::new(false) };
    static PREFIX_WARNED: Cell<bool> = const { Cell::new(false) };
}

pub fn is_reading_prefix() -> bool {
    READING_PREFIX.with(Cell::get)
}

pub fn swallow_prefix_log(metadata: &log::Metadata) -> bool {
    if !metadata.target().starts_with("rawler") || !is_reading_prefix() {
        return false;
    }
    if metadata.level() <= log::Level::Warn {
        PREFIX_WARNED.with(|warned| warned.set(true));
    }
    true
}

pub fn read_from_prefix<T>(
    file_bytes: &[u8],
    read: impl Fn(&[u8], bool) -> Option<T>,
) -> Option<T> {
    let mut len = 256 * 1024;
    while len < file_bytes.len() && len <= 16 * 1024 * 1024 {
        READING_PREFIX.with(|reading| reading.set(true));
        PREFIX_WARNED.with(|warned| warned.set(false));
        let found = catch_unwind(AssertUnwindSafe(|| read(&file_bytes[..len], false)));
        READING_PREFIX.with(|reading| reading.set(false));
        if let Ok(Some(found)) = found
            && !PREFIX_WARNED.with(Cell::get)
        {
            return Some(found);
        }
        len *= 4;
    }
    read(file_bytes, true)
}

pub fn extract_embedded_preview(file_bytes: &[u8]) -> Option<DynamicImage> {
    read_from_prefix(file_bytes, |bytes, whole_file| {
        let source = RawSource::new_from_slice(bytes);
        let decoder = rawler::get_decoder(&source).ok()?;
        let orientation = match metadata_orientation(decoder.as_ref(), &source) {
            Ok(orientation) => orientation,
            Err(_) if whole_file => Orientation::Normal,
            Err(_) => return None,
        };
        let preview = decoder
            .full_image(&source, &RawDecodeParams::default())
            .ok()??;
        Some(apply_orientation(preview, orientation))
    })
}

fn is_linear_raw_format(raw_image: &RawImage) -> bool {
    matches!(
        raw_image.photometric,
        RawPhotometricInterpretation::LinearRaw
    )
}

#[inline]
fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(3.0)
    }
}

#[inline]
fn smootherstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn recover_clipped_pixel(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max_c = r.max(g).max(b);

    if max_c <= 0.50 {
        return (r, g, b);
    }

    let mut cur_r = r;
    let mut cur_g = g;
    let mut cur_b = b;

    let outer_blend = smootherstep(0.50, 1.5, max_c);

    let magenta = (cur_r.min(cur_b) - cur_g).max(0.0);
    if magenta > 0.0 {
        let target_g = cur_r.min(cur_b) * 0.80 + ((cur_r + cur_b) * 0.5) * 0.20;
        let correction = (target_g - cur_g).max(0.0);
        let magenta_weight = smoothstep(0.0, 0.25, magenta / max_c);
        cur_g += correction * outer_blend * magenta_weight;
    }

    let residual = (cur_r.min(cur_b) - cur_g).max(0.0);
    if residual > 0.0 {
        cur_g += residual * outer_blend;
    }

    let new_max = cur_r.max(cur_g).max(cur_b);
    let min_c = cur_r.min(cur_g).min(cur_b);

    let knee = smoothstep(0.50, 1.5, new_max);

    if knee > 0.0 {
        let neutrality = (min_c / new_max.max(1e-5)).clamp(0.0, 1.0);

        let core_burn = smoothstep(0.60, 3.0, new_max);

        let desat = (knee * (neutrality * 0.85 + core_burn * 0.15)).clamp(0.0, 1.0);
        let smooth_desat = desat * desat * (3.0 - 2.0 * desat);

        let neutral_value = min_c + (new_max - min_c) * 1.0;

        cur_r = cur_r * (1.0 - smooth_desat) + neutral_value * smooth_desat;
        cur_g = cur_g * (1.0 - smooth_desat) + neutral_value * smooth_desat;
        cur_b = cur_b * (1.0 - smooth_desat) + neutral_value * smooth_desat;
    }

    (cur_r, cur_g, cur_b)
}

fn primary_color_matrix(raw_image: &RawImage) -> Option<&FlatColorMatrix> {
    let matrices = &raw_image.color_matrix;
    matrices
        .get(&Illuminant::D65)
        .or_else(|| matrices.values().next())
}

pub(crate) struct GamutCompression {
    pub(crate) threshold: f32,
    pub(crate) scale: f32,
}

impl GamutCompression {
    const THRESHOLD: f32 = 0.8;
    const POWER: f32 = 1.2;

    pub(crate) fn new(limit: f32) -> Option<Self> {
        if limit <= 1.0 {
            return None;
        }
        let threshold = Self::THRESHOLD;
        let span = limit - threshold;
        let scale =
            span / (((1.0 - threshold) / span).powf(-Self::POWER) - 1.0).powf(1.0 / Self::POWER);
        Some(Self { threshold, scale })
    }

    #[inline]
    pub(crate) fn compress(&self, distance: f32) -> f32 {
        if distance <= self.threshold {
            return distance;
        }
        let excess = distance - self.threshold;
        self.threshold
            + excess / (1.0 + (excess / self.scale).powf(Self::POWER)).powf(1.0 / Self::POWER)
    }
}

struct CameraToSrgb {
    matrix: [[f32; 4]; 3],
    compression: [Option<GamutCompression>; 3],
}

impl GamutCompression {
    /// The inverse of [`GamutCompression::compress`]: the distance a compressed distance came from.
    #[allow(dead_code)]
    fn decompress(&self, distance: f32) -> f32 {
        if distance <= self.threshold {
            return distance;
        }
        let v = ((distance - self.threshold) / self.scale)
            .powf(Self::POWER)
            .min(0.999);
        self.threshold + self.scale * (v / (1.0 - v)).powf(1.0 / Self::POWER)
    }
}

impl CameraToSrgb {
    fn new(raw_image: &RawImage) -> Option<Self> {
        let color_matrix = as_shot_color(raw_image)
            .map(|(_, color_matrix)| color_matrix)
            .or_else(|| primary_color_matrix(raw_image).cloned())?;
        if color_matrix.len() % 3 != 0 || color_matrix.len() > 12 {
            return None;
        }
        let primaries = crate::decode::white_balance::camera_to_srgb(
            &color_matrix,
            &camera_neutral(raw_image),
        )?;

        let mut matrix = [[0.0f32; 4]; 3];
        let mut limits = [0.0f32; 3];
        for (channel, primary) in primaries.iter().enumerate() {
            for (row, value) in matrix.iter_mut().zip(primary) {
                row[channel] = *value;
            }
            let peak = primary[0].max(primary[1]).max(primary[2]);
            if peak > 0.0 {
                for (limit, value) in limits.iter_mut().zip(primary) {
                    *limit = limit.max((peak - value) / peak);
                }
            }
        }

        Some(Self {
            matrix,
            compression: limits.map(GamutCompression::new),
        })
    }

    /// (threshold, scale) of each channel's compression, `[0, 0]` where there is none.
    fn gamut_parameters(&self) -> [[f32; 2]; 3] {
        self.compression
            .each_ref()
            .map(|c| c.as_ref().map_or([0.0; 2], |c| [c.threshold, c.scale]))
    }

    #[inline]
    fn pixel<const N: usize>(&self, camera: &[f32; N]) -> [f32; 3] {
        let mut rgb = self
            .matrix
            .map(|row| row.iter().zip(camera).map(|(m, c)| m * c).sum::<f32>());
        let achromatic = rgb[0].max(rgb[1]).max(rgb[2]);
        if achromatic <= 0.0 {
            return [0.0; 3];
        }
        for (value, compression) in rgb.iter_mut().zip(&self.compression) {
            if let Some(compression) = compression {
                let distance = (achromatic - *value) / achromatic;
                *value = achromatic - compression.compress(distance) * achromatic;
            }
        }
        rgb
    }

    fn camera(&self, wb: [f32; 3], clamp_limit: f32) -> Camera {
        Camera {
            wb,
            matrix: self.matrix.map(|row| [row[0], row[1], row[2]]),
            compression: self
                .compression
                .each_ref()
                .map(|c| c.as_ref().map(|c| (c.threshold, c.scale))),
            clamp_limit,
        }
    }

    fn convert_pixels<const N: usize>(&self, pixels: &Color2D<f32, N>) -> Color2D<f32, 3> {
        Color2D::new_with(
            pixels.data.par_iter().map(|p| self.pixel(p)).collect(),
            pixels.width,
            pixels.height,
        )
    }

    fn apply(&self, intermediate: Intermediate) -> Intermediate {
        match intermediate {
            Intermediate::ThreeColor(pixels) => {
                Intermediate::ThreeColor(self.convert_pixels(&pixels))
            }
            Intermediate::FourColor(pixels) => {
                Intermediate::ThreeColor(self.convert_pixels(&pixels))
            }
            monochrome => monochrome,
        }
    }
}

pub fn develop_raw_image(
    file_bytes: &[u8],
    fast_demosaic: bool,
    _highlight_compression: f32,
    linear_mode: String,
    denoise: f32,
    sharpen: f32,
    cancel_token: Option<(Arc<AtomicUsize>, usize)>,
) -> Result<DynamicImage> {
    let check_cancel = || -> Result<()> {
        if let Some((tracker, generation)) = &cancel_token
            && tracker.load(Ordering::SeqCst) != *generation
        {
            return Err(anyhow!("Load cancelled"));
        }
        Ok(())
    };

    check_cancel()?;

    let source = RawSource::new_from_slice(file_bytes);
    let decoder = rawler::get_decoder(&source)?;

    check_cancel()?;
    let mut raw_image: RawImage = decoder.raw_image(&source, &RawDecodeParams::default(), false)?;

    let orientation = metadata_orientation(decoder.as_ref(), &source)?;

    let is_linear_format = is_linear_raw_format(&raw_image);

    let (apply_ungamma, apply_calibration) = match linear_mode.as_str() {
        "gamma" => (true, true),
        "skip_calib" => (false, false),
        "gamma_skip_calib" => (true, false),
        _ => (false, true),
    };

    let original_white_level = raw_image
        .whitelevel
        .0
        .first()
        .cloned()
        .unwrap_or(u16::MAX as u32) as f32;
    let original_black_level = raw_image
        .blacklevel
        .levels
        .first()
        .map(|r| r.as_f32())
        .unwrap_or(0.0);

    raw_image.wb_coeffs = crate::decode::multi_exposure::neutralize_wb_if_multiexposure(
        raw_image.wb_coeffs,
        file_bytes,
    );

    let camera_to_srgb = (apply_calibration || !is_linear_format)
        .then(|| CameraToSrgb::new(&raw_image))
        .flatten();
    let cfa_balance = (!is_linear_format && camera_to_srgb.is_some())
        .then(|| cfa_white_balance(&raw_image))
        .flatten();
    let (clamp_limit, denoise, sharpen) = if fast_demosaic {
        (1.0, 0.0, 0.0)
    } else {
        (HIGHLIGHT_HEADROOM, denoise, sharpen)
    };
    let finish = |camera: Option<Camera>| Finish {
        camera,
        orientation,
        denoise,
        sharpen,
    };

    if let (Some(wb), Some(camera_to_srgb)) = (cfa_balance, &camera_to_srgb)
        && let Some(mosaic) = Mosaic::new(&raw_image, wb)
    {
        check_cancel()?;
        let finish = finish(Some(camera_to_srgb.camera(wb, clamp_limit)));
        let context = crate::render::gpu::context::decode_gpu_context().map_err(|e| anyhow!(e))?;
        let crop = |dim| {
            default_crop(
                raw_image.crop_area,
                raw_image.active_area,
                raw_image.dim(),
                dim,
            )
        };
        let image = if fast_demosaic {
            let (data, dim) = mosaic.bin();
            check_cancel()?;
            let crop = crop(dim);
            develop::finish_rgb(
                &context,
                &crop_rows(&data, dim.w, crop),
                crop.d.w as u32,
                crop.d.h as u32,
                &finish,
            )?
        } else {
            let domain = mosaic.domain();
            let crop = crop(domain.d);
            let crop = Rect::new(
                Point::new(domain.p.x + crop.p.x, domain.p.y + crop.p.y),
                crop.d,
            );
            mosaic.develop(&context, crop, &finish)?
        };
        return Ok(DynamicImage::ImageRgb32F(image));
    }

    for level in raw_image.whitelevel.0.iter_mut() {
        *level = u32::MAX;
    }

    let mut developer = RawDevelop::default();
    developer
        .steps
        .retain(|&step| step != ProcessingStep::SRgb && step != ProcessingStep::Calibrate);

    if is_linear_format {
        developer
            .steps
            .retain(|&step| step != ProcessingStep::Demosaic);
    } else if fast_demosaic {
        developer.demosaic_algorithm = DemosaicAlgorithm::Speed;
    }

    if cfa_balance.is_some() {
        developer
            .steps
            .retain(|&step| step != ProcessingStep::CropDefault);
    }

    check_cancel()?;
    let mut developed_intermediate = developer.develop_intermediate(&raw_image)?;

    let (crop_area, active_area, raw_dim) =
        (raw_image.crop_area, raw_image.active_area, raw_image.dim());
    drop(raw_image);

    if cfa_balance.is_none()
        && let Some(camera_to_srgb) = &camera_to_srgb
    {
        check_cancel()?;
        developed_intermediate = camera_to_srgb.apply(developed_intermediate);
    }

    let denominator = (original_white_level - original_black_level).max(1.0);
    let rescale_factor = (u32::MAX as f32 - original_black_level) / denominator;

    let (width, height) = {
        let dim = developed_intermediate.dim();
        (dim.w as u32, dim.h as u32)
    };

    check_cancel()?;

    match &mut developed_intermediate {
        Intermediate::Monochrome(pixels) => {
            pixels.data.par_iter_mut().for_each(|p| {
                let mut linear_val = *p * rescale_factor;
                if is_linear_format && apply_ungamma {
                    linear_val = srgb_to_linear(linear_val.max(0.0));
                }
                *p = linear_val.clamp(0.0, clamp_limit);
            });
        }
        Intermediate::ThreeColor(pixels) if cfa_balance.is_some() => {
            let wb = cfa_balance.unwrap();
            let camera = camera_to_srgb.as_ref().unwrap().camera(wb, clamp_limit);
            let crop = default_crop(crop_area, active_area, raw_dim, pixels.dim());
            let mut data = crop_rows(&pixels.data, pixels.width, crop).into_owned();
            data.par_iter_mut().for_each(|p| {
                for c in 0..3 {
                    p[c] = (p[c] * rescale_factor).max(0.0) * wb[c];
                }
            });
            check_cancel()?;

            let context =
                crate::render::gpu::context::decode_gpu_context().map_err(|e| anyhow!(e))?;
            let image = develop::finish_rgb(
                &context,
                &data,
                crop.d.w as u32,
                crop.d.h as u32,
                &finish(Some(camera)),
            )?;
            return Ok(DynamicImage::ImageRgb32F(image));
        }
        Intermediate::ThreeColor(pixels) => {
            pixels.data.par_iter_mut().for_each(|p| {
                let mut r = (p[0] * rescale_factor).max(0.0);
                let mut g = (p[1] * rescale_factor).max(0.0);
                let mut b = (p[2] * rescale_factor).max(0.0);

                if is_linear_format && apply_ungamma {
                    r = srgb_to_linear(r.max(0.0));
                    g = srgb_to_linear(g.max(0.0));
                    b = srgb_to_linear(b.max(0.0));
                }

                let (rec_r, rec_g, rec_b) = recover_clipped_pixel(r, g, b);

                p[0] = rec_r.clamp(0.0, clamp_limit);
                p[1] = rec_g.clamp(0.0, clamp_limit);
                p[2] = rec_b.clamp(0.0, clamp_limit);
            });
        }
        Intermediate::FourColor(pixels) => {
            pixels.data.par_iter_mut().for_each(|p| {
                p.iter_mut().for_each(|c| {
                    let mut linear_val = *c * rescale_factor;
                    if is_linear_format && apply_ungamma {
                        linear_val = srgb_to_linear(linear_val.max(0.0));
                    }
                    *c = linear_val.clamp(0.0, clamp_limit);
                });
            });
        }
    }

    check_cancel()?;

    let dynamic_image = match developed_intermediate {
        Intermediate::ThreeColor(pixels) => {
            DynamicImage::ImageRgb32F(rgb_pixels_to_rgb(pixels.data, width, height)?)
        }
        Intermediate::Monochrome(pixels) => {
            DynamicImage::ImageRgb32F(mono_pixels_to_rgb(&pixels.data, width, height)?)
        }
        _ => {
            return Err(anyhow!("Unsupported intermediate format for conversion"));
        }
    };

    develop::enhance(
        apply_orientation(dynamic_image, orientation),
        denoise,
        sharpen,
    )
}

fn crop_rows(data: &[[f32; 3]], stride: usize, crop: Rect) -> Cow<'_, [[f32; 3]]> {
    let rows = &data[crop.p.y * stride..(crop.p.y + crop.d.h) * stride];
    if crop.d.w == stride {
        return Cow::Borrowed(rows);
    }
    Cow::Owned(
        rows.par_chunks_exact(stride)
            .flat_map_iter(|row| row[crop.p.x..crop.p.x + crop.d.w].iter().copied())
            .collect(),
    )
}

fn default_crop(
    crop_area: Option<Rect>,
    active_area: Option<Rect>,
    raw_dim: Dim2,
    dim: Dim2,
) -> Rect {
    let full = Rect::new(Point::zero(), dim);
    let Some(mut crop) = crop_area else {
        return full;
    };
    if let Some(active_area) = active_area {
        crop = crop.intersection(&active_area).adapt(&active_area);
    }
    let original_width = active_area.map(|area| area.d.w).unwrap_or(raw_dim.w);
    if original_width > 0 {
        let scale_factor = dim.w as f32 / original_width as f32;
        if (scale_factor - 1.0).abs() > 1e-6 {
            crop.scale(scale_factor);
        }
    }
    let in_bounds = crop.p.x + crop.d.w <= dim.w && crop.p.y + crop.d.h <= dim.h;
    if crop.is_empty() || crop.d == dim || !in_bounds {
        full
    } else {
        crop
    }
}

fn cfa_white_balance(raw_image: &RawImage) -> Option<[f32; 3]> {
    let RawPhotometricInterpretation::Cfa(config) = &raw_image.photometric else {
        return None;
    };
    if raw_image.cpp != 1 || !config.cfa.is_rgb() || config.cfa.unique_colors() != 3 {
        return None;
    }
    if raw_image.wb_coeffs[0].is_nan() {
        Some([1.0; 3])
    } else {
        Some([
            raw_image.wb_coeffs[0],
            raw_image.wb_coeffs[1],
            raw_image.wb_coeffs[2],
        ])
    }
}

pub fn read_as_shot(file_bytes: &[u8]) -> Option<AsShot> {
    let source = RawSource::new_from_slice(file_bytes);
    let decoder = rawler::get_decoder(&source).ok()?;
    let mut raw_image = decoder
        .raw_image(&source, &RawDecodeParams::default(), true)
        .ok()?;
    raw_image.wb_coeffs = crate::decode::multi_exposure::neutralize_wb_if_multiexposure(
        raw_image.wb_coeffs,
        file_bytes,
    );
    let is_monochrome = raw_image.cpp == 1
        && !matches!(raw_image.photometric, RawPhotometricInterpretation::Cfa(_));

    Some(AsShot {
        white_balance: (!is_monochrome)
            .then(|| as_shot_color(&raw_image))
            .flatten()
            .map(|(white_balance, _)| white_balance)
            .unwrap_or_else(WhiteBalance::reference),
        exposure: crate::decode::baseline_exposure::read(
            decoder.as_ref(),
            file_bytes,
            is_monochrome,
        ),
        gamut: if is_monochrome {
            [[0.0; 2]; 3]
        } else {
            CameraToSrgb::new(&raw_image)
                .map(|c| c.gamut_parameters())
                .unwrap_or([[0.0; 2]; 3])
        },
    })
}

fn camera_neutral(raw_image: &RawImage) -> [f32; 4] {
    if raw_image.wb_coeffs[0].is_nan() {
        [1.0; 4]
    } else {
        raw_image.wb_coeffs.map(|c| 1.0 / c)
    }
}

fn as_shot_color(raw_image: &RawImage) -> Option<(WhiteBalance, FlatColorMatrix)> {
    let neutral = camera_neutral(raw_image);

    let matrices = &raw_image.color_matrix;
    if let (Some(matrix_a), Some(matrix_d65)) =
        (matrices.get(&Illuminant::A), matrices.get(&Illuminant::D65))
    {
        return WhiteBalance::from_dual_illuminant_camera_neutral(matrix_a, matrix_d65, &neutral);
    }

    let color_matrix = primary_color_matrix(raw_image)?;
    WhiteBalance::from_camera_neutral(color_matrix, &neutral)
        .map(|white_balance| (white_balance, color_matrix.clone()))
}

fn rgb_pixels_to_rgb(
    data: Vec<[f32; 3]>,
    width: u32,
    height: u32,
) -> Result<ImageBuffer<Rgb<f32>, Vec<f32>>> {
    ImageBuffer::from_raw(width, height, data.into_flattened())
        .ok_or_else(|| anyhow!("Developed image size does not match its dimensions"))
}

fn mono_pixels_to_rgb(
    data: &[f32],
    width: u32,
    height: u32,
) -> Result<ImageBuffer<Rgb<f32>, Vec<f32>>> {
    let mut rgb = vec![0.0f32; data.len() * 3];
    rgb.par_chunks_exact_mut(3)
        .zip(data.par_iter())
        .for_each(|(dst, &p)| dst.fill(p));
    ImageBuffer::from_raw(width, height, rgb)
        .ok_or_else(|| anyhow!("Developed image size does not match its dimensions"))
}

pub fn raw_dimensions(file_bytes: &[u8]) -> Option<(usize, usize)> {
    let source = RawSource::new_from_slice(file_bytes);
    let decoder = rawler::get_decoder(&source).ok()?;
    let raw_img = decoder
        .raw_image(&source, &RawDecodeParams::default(), true)
        .ok()?;
    Some((raw_img.width, raw_img.height))
}

pub fn get_fast_demosaic_scale_factor(
    file_bytes: &[u8],
    decoded_width: u32,
    decoded_height: u32,
) -> f32 {
    if let Some((raw_width, raw_height)) = raw_dimensions(file_bytes) {
        let max_orig = (raw_width as f32).max(raw_height as f32);
        let max_comp = (decoded_width as f32).max(decoded_height as f32);
        if max_orig > 0.0 {
            let ratio = max_comp / max_orig;
            if ratio > 0.1 && ratio < 0.29 {
                return 0.25;
            } else if (0.29..0.4).contains(&ratio) {
                return 1.0 / 3.0;
            } else if (0.4..0.75).contains(&ratio) {
                return 0.5;
            }
        }
    }
    1.0
}

#[cfg(test)]
mod gpu_decode_tests {
    /// Develops the RAW files in `RAPIDRAW_TEST_RAWS` (default: the user's Pictures/test folder) on the GPU the app uses.
    #[test]
    fn develops_full_size_raws_on_this_gpu() {
        let dir = std::env::var("RAPIDRAW_TEST_RAWS").unwrap_or_else(|_| {
            format!("{}/Pictures/test", std::env::var("USERPROFILE").unwrap_or_default())
        });
        let Ok(entries) = std::fs::read_dir(&dir) else {
            eprintln!("skipped: no {dir}");
            return;
        };
        let raws: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("cr2") || e.eq_ignore_ascii_case("dng") || e.eq_ignore_ascii_case("nef") || e.eq_ignore_ascii_case("arw") || e.eq_ignore_ascii_case("raf")))
            .collect();
        for path in raws.iter().take(2) {
            let bytes = std::fs::read(path).unwrap();
            for (denoise, sharpen) in [(0.0, 0.0), (0.5, 0.35)] {
                let start = std::time::Instant::now();
                let image = super::develop_raw_image(&bytes, false, 2.5, String::new(), denoise, sharpen, None)
                    .unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
                eprintln!("{} (denoise {denoise}): {}x{} in {:?}", path.display(), image.width(), image.height(), start.elapsed());
                assert!(image.width() > 1000 && image.height() > 1000);
            }
            // compare with the camera's own rendering (embedded JPEG): both GPU paths must show the same picture
            let full = super::develop_raw_image(&bytes, false, 2.5, String::new(), 0.0, 0.0, None).unwrap().to_rgb32f();
            let fast = super::develop_raw_image(&bytes, true, 2.5, String::new(), 0.0, 0.0, None).unwrap().to_rgb32f();
            // full demosaic (texture written by the demosaic pass) against the fast 2x2/4x4 bin (texture
            // uploaded from the CPU), compared on log luminance: same picture, nothing black or NaN
            let luma = |img: &image::Rgb32FImage, x: u32, y: u32| {
                let ix = ((x as u64 * img.width() as u64) / fast.width() as u64) as u32;
                let iy = ((y as u64 * img.height() as u64) / fast.height() as u64) as u32;
                let q = img.get_pixel(ix.min(img.width() - 1), iy.min(img.height() - 1)).0;
                assert!(q.iter().all(|v| v.is_finite()), "NaN in output");
                ((q[0] + q[1] + q[2]) / 3.0).max(1.0e-4).log2() as f64
            };
            // how much of the picture the camera recorded beyond sRGB: the develop pass compressed it into
            // sRGB, the shader inverts that (same formula as `gamut_residual` in shader.wgsl)
            let gamut = super::read_as_shot(&bytes).expect("as shot").gamut;
            eprintln!("{}: gamut compression (threshold, scale) per channel {:?}", path.display(), gamut);
            if gamut.iter().any(|g| g[1] > 0.0) {
                let (mut outside, mut deep, mut total) = (0u64, 0u64, 0u64);
                for y in (0..full.height()).step_by(11) {
                    for x in (0..full.width()).step_by(11) {
                        let c = full.get_pixel(x, y).0;
                        let a = c[0].max(c[1]).max(c[2]);
                        if a <= 1.0e-4 { continue; }
                        let mut most = 0.0f32;
                        for i in 0..3 {
                            let [t, k] = gamut[i];
                            let d = (a - c[i]) / a;
                            let dec = if k <= 0.0 || d <= t { d } else {
                                let v = ((d.min(1.0) - t) / k).powf(1.2).min(0.999);
                                t + k * (v / (1.0 - v)).powf(1.0 / 1.2)
                            };
                            most = most.max((dec - d) * a / a);
                        }
                        total += 1;
                        if most > 0.01 { outside += 1; }
                        if most > 0.1 { deep += 1; }
                    }
                }
                eprintln!("{}: {:.2}% of the pixels lie beyond sRGB (relative distance > 0.01), {:.2}% far beyond (> 0.1)", path.display(), 100.0 * outside as f64 / total as f64, 100.0 * deep as f64 / total as f64);
            }
            let (mut n, mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
            for y in (0..fast.height()).step_by(3) {
                for x in (0..fast.width()).step_by(3) {
                    let (a, b) = (luma(&fast, x, y), luma(&full, x, y));
                    n += 1.0; sx += a; sy += b; sxx += a * a; syy += b * b; sxy += a * b;
                }
            }
            let corr = (n * sxy - sx * sy) / ((n * sxx - sx * sx).sqrt() * (n * syy - sy * sy).sqrt());
            eprintln!("{}: full vs fast log-luma correlation {:.4}, mean stops {:.2} / {:.2}", path.display(), corr, sy / n, sx / n);
            assert!(sy / n > -10.0, "full develop is black");
            assert!(corr > 0.9, "full and fast develop disagree");
        }
    }
}

#[cfg(test)]
mod gamut_tests {
    use super::GamutCompression;

    #[test]
    fn decompress_inverts_compress_up_to_the_cameras_gamut_limit() {
        for limit in [1.1f32, 1.4, 2.0, 3.5] {
            let c = GamutCompression::new(limit).expect("compression");
            // the camera's own gamut edge lands exactly on 1 (the sRGB boundary)
            assert!((c.compress(limit) - 1.0).abs() < 1e-4, "limit {limit}: {}", c.compress(limit));
            let mut d = 0.0;
            while d <= limit {
                let back = c.decompress(c.compress(d));
                assert!((back - d).abs() < 2e-3 * (1.0 + d), "limit {limit} d {d}: back {back}");
                d += 0.01;
            }
            assert!((c.decompress(1.0) - limit).abs() < 0.02 * limit);
        }
    }
}
