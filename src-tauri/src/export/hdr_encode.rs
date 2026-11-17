//! HDR export encoders: PNG and AVIF with a PQ or HLG signal (BT.2020, CICP signalled), and Ultra HDR
//! JPEG (an SDR JPEG plus a gain map, the format Instagram, Threads, Android and Chrome read).
//!
//! Input is the HDR render: linear light in sRGB/Rec.709 primaries, 1.0 = SDR white (203 nits), values
//! above 1.0 are the HDR highlights.

use crate::export::encode::SRGB_ICC_PROFILE;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{DynamicImage, ImageEncoder, RgbImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::io::Cursor;

/// Nits that 1.0 (SDR white) of the HDR render stands for (ITU-R BT.2408 reference white).
pub const SDR_WHITE_NITS: f32 = 203.0;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum HdrTransfer {
    #[default]
    Pq,
    Hlg,
}

impl HdrTransfer {
    /// CICP transfer characteristics code (ITU-T H.273).
    fn cicp(self) -> u8 {
        match self {
            HdrTransfer::Pq => 16,
            HdrTransfer::Hlg => 18,
        }
    }
}

/// SMPTE ST 2084 inverse EOTF: absolute luminance in nits to a 0..1 signal.
pub fn pq_encode(nits: f32) -> f32 {
    const M1: f32 = 0.159_301_76;
    const M2: f32 = 78.843_75;
    const C1: f32 = 0.835_937_5;
    const C2: f32 = 18.851_563;
    const C3: f32 = 18.687_5;
    let y = (nits / 10_000.0).clamp(0.0, 1.0).powf(M1);
    ((C1 + C2 * y) / (1.0 + C3 * y)).powf(M2)
}

/// ITU-R BT.2100 HLG: display light in nits (for a 1000 nit reference display) to a 0..1 signal.
pub fn hlg_encode(nits: f32) -> f32 {
    const A: f32 = 0.178_832_77;
    const B: f32 = 0.284_668_92;
    const C: f32 = 0.559_910_7;
    // inverse OOTF (system gamma 1.2 at 1000 nits), then the HLG OETF
    let scene = (nits / 1000.0).clamp(0.0, 1.0).powf(1.0 / 1.2);
    if scene <= 1.0 / 12.0 {
        (3.0 * scene).sqrt()
    } else {
        (A * (12.0 * scene - B).ln() + C).clamp(0.0, 1.0)
    }
}

/// Rec.709/sRGB primaries to Rec.2020 (linear light).
fn to_rec2020(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    [
        0.627_404 * r + 0.329_283 * g + 0.043_313 * b,
        0.069_097 * r + 0.919_540 * g + 0.011_362 * b,
        0.016_391 * r + 0.088_013 * g + 0.895_595 * b,
    ]
}

/// The non-linear BT.2020 R'G'B' signal (0..1) of a linear sRGB-primaries pixel (1.0 = SDR white).
fn signal(rgb: [f32; 3], transfer: HdrTransfer) -> [f32; 3] {
    let wide = to_rec2020([rgb[0].max(0.0), rgb[1].max(0.0), rgb[2].max(0.0)]);
    wide.map(|v| {
        let nits = v * SDR_WHITE_NITS;
        match transfer {
            HdrTransfer::Pq => pq_encode(nits),
            HdrTransfer::Hlg => hlg_encode(nits),
        }
    })
}

fn finite(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

// ---------------------------------------------------------------------------------------- PNG

/// 16-bit PNG with a PQ or HLG signal in BT.2020, signalled with a `cICP` chunk.
pub fn encode_hdr_png(
    image: &DynamicImage,
    transfer: HdrTransfer,
    dpi: Option<u16>,
) -> Result<Vec<u8>, String> {
    let (width, height) = (image.width(), image.height());
    let rgb = image.to_rgb32f();
    let mut samples = vec![0u16; rgb.as_raw().len()];
    samples
        .par_chunks_mut(3)
        .zip(rgb.as_raw().par_chunks(3))
        .for_each(|(out, px)| {
            let s = signal([finite(px[0]), finite(px[1]), finite(px[2])], transfer);
            for c in 0..3 {
                out[c] = (s[c].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16;
            }
        });

    let mut bytes = Vec::new();
    PngEncoder::new(Cursor::new(&mut bytes))
        .write_image(
            bytemuck::cast_slice(&samples),
            width,
            height,
            image::ExtendedColorType::Rgb16,
        )
        .map_err(|e| format!("Failed to encode HDR PNG: {}", e))?;
    // primaries 9 (BT.2020), transfer 16 (PQ) / 18 (HLG), matrix 0 (RGB), full range
    crate::metadata::export_metadata::insert_png_chunk(&mut bytes, b"cICP", &[9, transfer.cicp(), 0, 1]);
    if let Some(dpi) = dpi {
        let ppm = (dpi as f64 / 0.0254).round() as u32;
        let mut phys = Vec::with_capacity(9);
        phys.extend_from_slice(&ppm.to_be_bytes());
        phys.extend_from_slice(&ppm.to_be_bytes());
        phys.push(1);
        crate::metadata::export_metadata::insert_png_chunk(&mut bytes, b"pHYs", &phys);
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------------------- AVIF

/// 10-bit AVIF with a PQ or HLG signal in BT.2020 (YCbCr 4:4:4, BT.2020 non-constant luminance).
pub fn encode_hdr_avif(
    image: &DynamicImage,
    transfer: HdrTransfer,
    quality: u8,
) -> Result<Vec<u8>, String> {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let rgb = image.to_rgb32f();
    let planes: Vec<[u16; 3]> = rgb
        .as_raw()
        .par_chunks(3)
        .map(|px| {
            let [r, g, b] = signal([finite(px[0]), finite(px[1]), finite(px[2])], transfer);
            let y = 0.2627 * r + 0.6780 * g + 0.0593 * b;
            let cb = (b - y) / 1.8814;
            let cr = (r - y) / 1.4746;
            let code = |v: f32| (v * 1023.0 + 0.5).clamp(0.0, 1023.0) as u16;
            [code(y), code(cb + 0.5), code(cr + 0.5)]
        })
        .collect();

    let encoded = ravif::Encoder::new()
        .with_quality(quality.clamp(1, 100) as f32)
        .with_speed(7)
        .encode_raw_planes_10_bit(
            width,
            height,
            planes,
            None::<[u16; 0]>,
            ravif::PixelRange::Full,
            ravif::MatrixCoefficients::BT2020NCL,
        )
        .map_err(|e| format!("Failed to encode HDR AVIF: {}", e))?;
    let mut bytes = encoded.avif_file;
    set_avif_colr(&mut bytes, 9, transfer.cicp(), 9)?;
    Ok(bytes)
}

/// ravif always labels its output sRGB: rewrite the `colr` box (`nclx`) to the real CICP values.
fn set_avif_colr(bytes: &mut [u8], primaries: u8, transfer: u8, matrix: u8) -> Result<(), String> {
    let at = bytes
        .windows(8)
        .position(|w| &w[..4] == b"colr" && &w[4..] == b"nclx")
        .ok_or("AVIF has no colr box")?;
    let body = at + 8;
    if bytes.len() < body + 7 {
        return Err("AVIF colr box is truncated".into());
    }
    bytes[body..body + 2].copy_from_slice(&(primaries as u16).to_be_bytes());
    bytes[body + 2..body + 4].copy_from_slice(&(transfer as u16).to_be_bytes());
    bytes[body + 4..body + 6].copy_from_slice(&(matrix as u16).to_be_bytes());
    bytes[body + 6] = 0x80; // full range
    Ok(())
}

// ------------------------------------------------------------------------------ Ultra HDR JPEG

/// Offsets in the gain map, as in the Ultra HDR / Adobe gain map specification (1/64).
const GAIN_OFFSET: f32 = 1.0 / 64.0;

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// How the gain map is encoded; also what a reader needs to rebuild the HDR image from the base image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainMapParams {
    /// log2 of the smallest and largest gain (HDR / SDR) in the map
    pub min_log2: f32,
    pub max_log2: f32,
}

/// An Ultra HDR JPEG: `sdr` is the base image shown everywhere, `hdr` (same size) its HDR rendition.
/// `exif_source`: EXIF is copied from this file when `keep_metadata` is set.
pub fn encode_ultra_hdr(
    sdr: &DynamicImage,
    hdr: &DynamicImage,
    quality: u8,
    metadata: Option<&dyn Fn(&mut Vec<u8>) -> Result<(), String>>,
) -> Result<Vec<u8>, String> {
    let sdr = sdr.to_rgb8();
    let hdr = hdr.to_rgb32f();
    if sdr.dimensions() != hdr.dimensions() {
        return Err(format!(
            "The SDR ({}x{}) and HDR ({}x{}) renders differ in size",
            sdr.width(),
            sdr.height(),
            hdr.width(),
            hdr.height()
        ));
    }
    let (gain_map, params) = build_gain_map(&sdr, &hdr);
    let gain_jpeg = encode_gain_map_jpeg(&gain_map, &params, quality)?;

    // the base image
    let mut primary = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(Cursor::new(&mut primary), quality);
    encoder
        .set_icc_profile(SRGB_ICC_PROFILE.to_vec())
        .map_err(|e| e.to_string())?;
    encoder
        .write_image(sdr.as_raw(), sdr.width(), sdr.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("Failed to encode the Ultra HDR base image: {}", e))?;
    if let Some(write_metadata) = metadata {
        write_metadata(&mut primary)?;
    }
    assemble(primary, &gain_jpeg, &params)
}

/// Per-channel gain map at half resolution: log2((hdr + o) / (sdr + o)) scaled to 0..255.
fn build_gain_map(sdr: &RgbImage, hdr: &image::Rgb32FImage) -> (RgbImage, GainMapParams) {
    let (w, h) = sdr.dimensions();
    let (gw, gh) = (w.div_ceil(2).max(1), h.div_ceil(2).max(1));
    let sdr_lin: Vec<f32> = sdr
        .as_raw()
        .par_iter()
        .map(|&v| srgb_to_linear(v as f32 / 255.0))
        .collect();

    let mut log_gain = vec![0.0f32; gw as usize * gh as usize * 3];
    log_gain
        .par_chunks_mut(gw as usize * 3)
        .enumerate()
        .for_each(|(gy, row)| {
            for gx in 0..gw as usize {
                let mut sum_s = [0.0f32; 3];
                let mut sum_h = [0.0f32; 3];
                let mut n = 0.0f32;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (x, y) = ((gx * 2 + dx) as u32, (gy * 2 + dy) as u32);
                        if x < w && y < h {
                            let i = (y * w + x) as usize * 3;
                            for c in 0..3 {
                                sum_s[c] += sdr_lin[i + c];
                                sum_h[c] += finite(hdr.as_raw()[i + c]).max(0.0);
                            }
                            n += 1.0;
                        }
                    }
                }
                for c in 0..3 {
                    row[gx * 3 + c] =
                        ((sum_h[c] / n + GAIN_OFFSET) / (sum_s[c] / n + GAIN_OFFSET)).log2();
                }
            }
        });

    let (mut min, mut max) = (f32::MAX, f32::MIN);
    for v in &log_gain {
        min = min.min(*v);
        max = max.max(*v);
    }
    if !min.is_finite() || !max.is_finite() {
        (min, max) = (0.0, 0.0);
    }
    // a readable range even for an image without any HDR content
    max = max.max(min + 0.01);
    let params = GainMapParams {
        min_log2: min,
        max_log2: max,
    };
    let range = max - min;
    let bytes: Vec<u8> = log_gain
        .par_iter()
        .map(|v| (((v - min) / range).clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
        .collect();
    (RgbImage::from_raw(gw, gh, bytes).expect("gain map size"), params)
}

/// Rebuilds the HDR value of one channel from the SDR value (0..1, sRGB encoded) and its gain map byte.
#[cfg(test)]
pub fn apply_gain(sdr_srgb: f32, gain_byte: u8, p: &GainMapParams) -> f32 {
    let recovery = gain_byte as f32 / 255.0;
    let log_boost = p.min_log2 * (1.0 - recovery) + p.max_log2 * recovery;
    (srgb_to_linear(sdr_srgb) + GAIN_OFFSET) * log_boost.exp2() - GAIN_OFFSET
}

fn gain_map_xmp(p: &GainMapParams) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"RapidRAW+\">\
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
<rdf:Description rdf:about=\"\" xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\" \
hdrgm:Version=\"1.0\" hdrgm:GainMapMin=\"{min}\" hdrgm:GainMapMax=\"{max}\" hdrgm:Gamma=\"1\" \
hdrgm:OffsetSDR=\"{o}\" hdrgm:OffsetHDR=\"{o}\" hdrgm:HDRCapacityMin=\"0\" hdrgm:HDRCapacityMax=\"{cap}\" \
hdrgm:BaseRenditionIsHDR=\"False\"/></rdf:RDF></x:xmpmeta>",
        min = p.min_log2,
        max = p.max_log2,
        o = GAIN_OFFSET,
        cap = p.max_log2.max(0.0)
    )
}

fn primary_xmp(gain_map_len: usize) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"RapidRAW+\">\
<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
<rdf:Description rdf:about=\"\" xmlns:Container=\"http://ns.google.com/photos/1.0/container/\" \
xmlns:Item=\"http://ns.google.com/photos/1.0/container/item/\" \
xmlns:hdrgm=\"http://ns.adobe.com/hdr-gain-map/1.0/\" hdrgm:Version=\"1.0\">\
<Container:Directory><rdf:Seq>\
<rdf:li rdf:parseType=\"Resource\"><Container:Item Item:Semantic=\"Primary\" Item:Mime=\"image/jpeg\"/></rdf:li>\
<rdf:li rdf:parseType=\"Resource\"><Container:Item Item:Semantic=\"GainMap\" Item:Mime=\"image/jpeg\" Item:Length=\"{gain_map_len}\"/></rdf:li>\
</rdf:Seq></Container:Directory></rdf:Description></rdf:RDF></x:xmpmeta>"
    )
}

const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

fn app_segment(marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut seg = Vec::with_capacity(payload.len() + 4);
    seg.extend_from_slice(&[0xFF, marker]);
    seg.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    seg.extend_from_slice(payload);
    seg
}

fn xmp_segment(packet: &str) -> Vec<u8> {
    let mut payload = XMP_HEADER.to_vec();
    payload.extend_from_slice(packet.as_bytes());
    app_segment(0xE1, &payload)
}

/// The gain map as a JPEG carrying its own XMP (the parameters a reader needs).
fn encode_gain_map_jpeg(map: &RgbImage, params: &GainMapParams, quality: u8) -> Result<Vec<u8>, String> {
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(Cursor::new(&mut jpeg), quality.clamp(60, 98))
        .write_image(map.as_raw(), map.width(), map.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("Failed to encode the gain map: {}", e))?;
    let (_, at) = split_leading_segments(&jpeg)?;
    let xmp = xmp_segment(&gain_map_xmp(params));
    jpeg.splice(at..at, xmp);
    Ok(jpeg)
}

/// Drops every XMP segment from the start of a JPEG and returns where new APP segments go: after the
/// leading APP0/APP1/APP2 segments (JFIF, EXIF, ICC).
fn split_leading_segments(jpeg: &[u8]) -> Result<(Vec<u8>, usize), String> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return Err("Not a JPEG".into());
    }
    let mut out = Vec::with_capacity(jpeg.len());
    out.extend_from_slice(&jpeg[..2]);
    let mut pos = 2;
    let mut insert_at = 2;
    while pos + 4 <= jpeg.len() && jpeg[pos] == 0xFF && (0xE0..=0xEF).contains(&jpeg[pos + 1]) {
        let len = u16::from_be_bytes([jpeg[pos + 2], jpeg[pos + 3]]) as usize;
        if len < 2 || pos + 2 + len > jpeg.len() {
            return Err("Corrupt JPEG segment".into());
        }
        let payload = &jpeg[pos + 4..pos + 2 + len];
        let is_xmp = jpeg[pos + 1] == 0xE1 && payload.starts_with(XMP_HEADER);
        if !is_xmp {
            out.extend_from_slice(&jpeg[pos..pos + 2 + len]);
            insert_at = out.len();
        }
        pos += 2 + len;
    }
    out.extend_from_slice(&jpeg[pos..]);
    Ok((out, insert_at))
}

/// Size of the MPF APP2 segment this module writes (marker, length, "MPF\0", header, IFD, two entries).
const MPF_SEGMENT_LEN: usize = 2 + 2 + 4 + 8 + 42 + 32;

fn mpf_segment(primary_len: usize, gain_len: usize, mpf_header_pos: usize) -> Vec<u8> {
    let mut p = Vec::with_capacity(MPF_SEGMENT_LEN);
    p.extend_from_slice(b"MPF\0");
    // big endian MP header; offsets below are relative to the "MM" bytes
    p.extend_from_slice(&[0x4D, 0x4D, 0x00, 0x2A, 0, 0, 0, 8]);
    p.extend_from_slice(&3u16.to_be_bytes());
    // MPFVersion "0100"
    p.extend_from_slice(&0xB000u16.to_be_bytes());
    p.extend_from_slice(&7u16.to_be_bytes());
    p.extend_from_slice(&4u32.to_be_bytes());
    p.extend_from_slice(b"0100");
    // NumberOfImages = 2
    p.extend_from_slice(&0xB001u16.to_be_bytes());
    p.extend_from_slice(&4u16.to_be_bytes());
    p.extend_from_slice(&1u32.to_be_bytes());
    p.extend_from_slice(&2u32.to_be_bytes());
    // MPEntry: two 16-byte entries stored right after the IFD (8 + 2 + 36 + 4 = 50)
    p.extend_from_slice(&0xB002u16.to_be_bytes());
    p.extend_from_slice(&7u16.to_be_bytes());
    p.extend_from_slice(&32u32.to_be_bytes());
    p.extend_from_slice(&50u32.to_be_bytes());
    p.extend_from_slice(&0u32.to_be_bytes()); // no next IFD
    // entry 1: the base image (representative, JPEG)
    p.extend_from_slice(&0x2003_0000u32.to_be_bytes());
    p.extend_from_slice(&(primary_len as u32).to_be_bytes());
    p.extend_from_slice(&0u32.to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]);
    // entry 2: the gain map, right after the base image
    p.extend_from_slice(&0u32.to_be_bytes());
    p.extend_from_slice(&(gain_len as u32).to_be_bytes());
    p.extend_from_slice(&((primary_len - mpf_header_pos) as u32).to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]);
    app_segment(0xE2, &p)
}

fn assemble(primary: Vec<u8>, gain_jpeg: &[u8], _params: &GainMapParams) -> Result<Vec<u8>, String> {
    let (mut base, at) = split_leading_segments(&primary)?;
    let xmp = xmp_segment(&primary_xmp(gain_jpeg.len()));
    let mpf_at = at + xmp.len();
    base.splice(at..at, xmp);
    // placeholder of the right size, sizes and offsets are filled in once the base length is known
    base.splice(mpf_at..mpf_at, vec![0u8; MPF_SEGMENT_LEN]);
    let primary_len = base.len();
    // "MM" starts after marker (2), length (2) and "MPF\0" (4)
    let mpf = mpf_segment(primary_len, gain_jpeg.len(), mpf_at + 8);
    debug_assert_eq!(mpf.len(), MPF_SEGMENT_LEN);
    base[mpf_at..mpf_at + MPF_SEGMENT_LEN].copy_from_slice(&mpf);
    base.extend_from_slice(gain_jpeg);
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(w: u32, h: u32) -> (DynamicImage, DynamicImage) {
        let mut hdr = image::Rgb32FImage::new(w, h);
        let mut sdr = image::RgbImage::new(w, h);
        for (x, y, px) in hdr.enumerate_pixels_mut() {
            let t = x as f32 / (w - 1) as f32;
            // a ramp up to 4x SDR white, with a warm tint in the highlights
            let v = 0.02 + t * 4.0;
            let lin = [v, v * (1.0 - 0.25 * t), v * (1.0 - 0.6 * t)];
            *px = image::Rgb(lin);
            let enc = |l: f32| {
                let l = l.clamp(0.0, 1.0);
                let s = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
                (s * 255.0 + 0.5) as u8
            };
            sdr.put_pixel(x, y, image::Rgb([enc(lin[0]), enc(lin[1]), enc(lin[2])]));
        }
        (DynamicImage::ImageRgb8(sdr), DynamicImage::ImageRgb32F(hdr))
    }

    #[test]
    fn pq_matches_known_values() {
        assert!((pq_encode(0.0)).abs() < 1e-4);
        assert!((pq_encode(10_000.0) - 1.0).abs() < 1e-4);
        // reference white 203 nits is about 58% of the PQ signal range
        assert!((pq_encode(203.0) - 0.5807).abs() < 0.002, "{}", pq_encode(203.0));
        assert!(pq_encode(1000.0) > pq_encode(203.0));
        assert!(hlg_encode(203.0) > 0.5 && hlg_encode(203.0) < 0.8);
        assert!((hlg_encode(1000.0) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn png_carries_cicp_and_16_bits() {
        let (_, hdr) = scene(32, 8);
        let bytes = encode_hdr_png(&hdr, HdrTransfer::Pq, Some(300)).unwrap();
        let at = bytes.windows(4).position(|w| w == b"cICP").expect("cICP chunk");
        assert_eq!(&bytes[at + 4..at + 8], &[9, 16, 0, 1]);
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert!(matches!(decoded, DynamicImage::ImageRgb16(_)));
        // brighter input gives a larger signal
        let rgb = decoded.to_rgb16();
        assert!(rgb.get_pixel(31, 4).0[0] > rgb.get_pixel(0, 4).0[0]);
    }

    #[test]
    fn avif_is_labelled_bt2020_pq() {
        let (_, hdr) = scene(32, 16);
        let bytes = encode_hdr_avif(&hdr, HdrTransfer::Pq, 80).unwrap();
        assert_eq!(&bytes[4..8], b"ftyp");
        let at = bytes.windows(8).position(|w| &w[..4] == b"colr" && &w[4..] == b"nclx").unwrap();
        assert_eq!(&bytes[at + 8..at + 14], &[0, 9, 0, 16, 0, 9]);
        let hlg = encode_hdr_avif(&hdr, HdrTransfer::Hlg, 80).unwrap();
        let at = hlg.windows(8).position(|w| &w[..4] == b"colr" && &w[4..] == b"nclx").unwrap();
        assert_eq!(&hlg[at + 8..at + 14], &[0, 9, 0, 18, 0, 9]);
    }

    #[test]
    fn ultra_hdr_has_a_valid_container_and_recovers_the_hdr_image() {
        let (sdr, hdr) = scene(96, 48);
        let bytes = encode_ultra_hdr(&sdr, &hdr, 95, None).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);

        // the MPF entry points at the second image
        let at = bytes.windows(4).position(|w| w == b"MPF\0").expect("MPF segment");
        let header = at + 4;
        assert_eq!(&bytes[header..header + 4], &[0x4D, 0x4D, 0x00, 0x2A]);
        let entry = |i: usize, field: usize| {
            let o = header + 50 + i * 16 + field;
            u32::from_be_bytes(bytes[o..o + 4].try_into().unwrap()) as usize
        };
        let (primary_len, gain_len, gain_offset) = (entry(0, 4), entry(1, 4), entry(1, 8));
        assert_eq!(header + gain_offset, primary_len, "gain map starts right after the base image");
        assert_eq!(primary_len + gain_len, bytes.len());
        assert_eq!(&bytes[primary_len..primary_len + 2], &[0xFF, 0xD8]);
        assert_eq!(&bytes[primary_len - 2..primary_len], &[0xFF, 0xD9], "base image ends with EOI");

        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("Item:Semantic=\"GainMap\""));
        assert!(text.contains(&format!("Item:Length=\"{gain_len}\"")));
        assert!(text.contains("hdrgm:GainMapMax"));

        // both parts decode, and the gain map restores the highlights
        let base = image::load_from_memory(&bytes[..primary_len]).unwrap().to_rgb8();
        let map = image::load_from_memory(&bytes[primary_len..]).unwrap().to_rgb8();
        assert_eq!(base.dimensions(), (96, 48));
        assert_eq!(map.dimensions(), (48, 24));

        let params = {
            let grab = |key: &str| -> f32 {
                let s = &text[text.find(key).unwrap() + key.len() + 2..];
                s[..s.find('"').unwrap()].parse().unwrap()
            };
            GainMapParams { min_log2: grab("hdrgm:GainMapMin"), max_log2: grab("hdrgm:GainMapMax") }
        };
        let hdr = hdr.to_rgb32f();
        let mut worst: f32 = 0.0;
        for x in (0..96).step_by(6) {
            let (b, g) = (base.get_pixel(x, 20), map.get_pixel(x / 2, 10));
            for c in 0..3 {
                let rebuilt = apply_gain(b.0[c] as f32 / 255.0, g.0[c], &params);
                let truth = hdr.get_pixel(x, 20).0[c];
                // compare in stops: the gain map is 8-bit and half resolution
                worst = worst.max((((rebuilt + 0.02) / (truth + 0.02)).log2()).abs());
            }
        }
        assert!(worst < 0.35, "highlights are off by {worst} stops");
        assert!(params.max_log2 > 1.5, "a 4x highlight needs a large gain");
    }

    #[test]
    fn ultra_hdr_without_hdr_content_still_encodes() {
        let sdr = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(16, 16, image::Rgb([120, 120, 120])));
        let lin = srgb_to_linear(120.0 / 255.0);
        let hdr = DynamicImage::ImageRgb32F(image::Rgb32FImage::from_pixel(16, 16, image::Rgb([lin; 3])));
        let bytes = encode_ultra_hdr(&sdr, &hdr, 90, None).unwrap();
        assert!(bytes.len() > 100);
    }
}
