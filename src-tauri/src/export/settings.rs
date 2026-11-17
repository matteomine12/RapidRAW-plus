use crate::render::color_space::OutputSpace;
use crate::render::gpu::processor::RenderOutputPrecision;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(try_from = "u8", into = "u8")]
pub enum TiffBitDepth {
    Eight = 8,
    #[default]
    Sixteen = 16,
}

impl TryFrom<u8> for TiffBitDepth {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            8 => Ok(Self::Eight),
            16 => Ok(Self::Sixteen),
            _ => Err(format!(
                "Invalid TIFF bit depth '{}'; expected 8 or 16.",
                value
            )),
        }
    }
}

impl From<TiffBitDepth> for u8 {
    fn from(value: TiffBitDepth) -> Self {
        value as u8
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub enum ResizeMode {
    LongEdge,
    ShortEdge,
    Width,
    Height,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ResizeOptions {
    pub mode: ResizeMode,
    pub value: u32,
    pub dont_enlarge: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::enum_variant_names)]
pub enum BorderBasis {
    #[default]
    LongEdge,
    ShortEdge,
    EachEdge,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BorderOptions {
    #[serde(default)]
    pub basis: BorderBasis,
    pub horizontal_percent: f32,
    pub vertical_percent: f32,
    pub color: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PadOptions {
    pub ratio_width: f32,
    pub ratio_height: f32,
    pub color: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ExportSettings {
    pub jpeg_quality: u8,
    #[serde(default)]
    pub tiff_bit_depth: TiffBitDepth,
    pub resize: Option<ResizeOptions>,
    #[serde(default)]
    pub border: Option<BorderOptions>,
    #[serde(default)]
    pub pad: Option<PadOptions>,
    pub keep_metadata: bool,
    #[serde(default)]
    pub preserve_timestamps: bool,
    pub strip_gps: bool,
    pub filename_template: Option<String>,
    pub watermark: Option<WatermarkSettings>,
    #[serde(default)]
    pub export_masks: bool,
    #[serde(default)]
    pub preserve_folders: bool,
    #[serde(default)]
    pub destination_type: Option<String>,
    #[serde(default)]
    pub subfolder: Option<String>,
    #[serde(default)]
    pub dpi: Option<u16>,
    #[serde(default)]
    pub export_original: Option<OriginalExportOptions>,
    /// Keep the highlights of an HDR edit (Ultra HDR JPEG, PNG/AVIF with PQ or HLG, JPEG XL, float TIFF).
    #[serde(default = "default_true")]
    pub hdr_export: bool,
    /// Transfer function of HDR PNG and AVIF files.
    #[serde(default)]
    pub hdr_transfer: crate::export::hdr_encode::HdrTransfer,
    /// Colour space of HDR files (Ultra HDR JPEG: sRGB or Display P3; PNG and AVIF: Rec.709/sRGB, Display P3
    /// or Rec.2020). `None` picks the format's default.
    #[serde(default)]
    pub hdr_color_space: Option<crate::render::color_space::OutputSpace>,
}

fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct OriginalExportOptions {
    pub crop: bool,
    pub keep_exposure: bool,
}

#[derive(Clone)]
pub(crate) enum ExportAdjustmentsMode {
    UseSidecars {
        active_path: Option<String>,
        active_adjustments: Option<Value>,
    },
    GlobalOverride(Value),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub enum WatermarkAnchor {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WatermarkSettings {
    pub path: String,
    pub anchor: WatermarkAnchor,
    pub scale: f32,
    pub spacing: f32,
    pub opacity: f32,
}

/// The colour space an HDR export is rendered in: the one asked for if the format supports it, else the
/// format's default (Rec.2020 for PNG and AVIF, sRGB for JPEG).
pub fn hdr_output_space(output_format: &str, export_settings: &ExportSettings) -> OutputSpace {
    let (default, wide_allowed) = match output_format.to_lowercase().as_str() {
        "png" | "avif" => (OutputSpace::Rec2020, true),
        "jpg" | "jpeg" => (OutputSpace::Srgb, false),
        _ => return OutputSpace::Srgb,
    };
    match export_settings.hdr_color_space {
        Some(OutputSpace::Rec2020) if !wide_allowed => default,
        Some(space) => space,
        None => default,
    }
}

/// Border, padding and watermark are composed in 8/16 bit, so HDR exports skip them (and use the SDR rendition).
fn hdr_export_possible(adjustments: &serde_json::Value, export_settings: &ExportSettings) -> bool {
    adjustments["hdrEnabled"].as_bool() == Some(true)
        && export_settings.hdr_export
        && export_settings.border.is_none()
        && export_settings.pad.is_none()
        && export_settings.watermark.is_none()
}

/// An HDR edit exported as JPEG XL, TIFF, PNG or AVIF keeps its highlights above SDR white: the render
/// is linear float and the encoder writes it as float or as a PQ/HLG signal.
pub fn wants_hdr_export(
    adjustments: &serde_json::Value,
    output_format: &str,
    export_settings: &ExportSettings,
) -> bool {
    hdr_export_possible(adjustments, export_settings)
        && matches!(
            output_format.to_lowercase().as_str(),
            "jxl" | "tif" | "tiff" | "png" | "avif"
        )
}

/// An HDR edit exported as JPEG becomes an Ultra HDR JPEG: the SDR rendition plus a gain map, so it needs
/// two renders (SDR and HDR).
pub fn wants_ultra_hdr(
    adjustments: &serde_json::Value,
    output_format: &str,
    export_settings: &ExportSettings,
) -> bool {
    hdr_export_possible(adjustments, export_settings)
        && matches!(output_format.to_lowercase().as_str(), "jpg" | "jpeg")
}

pub fn render_output_precision(
    output_format: &str,
    export_settings: &ExportSettings,
    hdr: bool,
) -> RenderOutputPrecision {
    if hdr {
        RenderOutputPrecision::HdrLinear
    } else if matches!(output_format.to_lowercase().as_str(), "tif" | "tiff")
        && export_settings.tiff_bit_depth == TiffBitDepth::Sixteen
    {
        RenderOutputPrecision::SixteenBit
    } else {
        RenderOutputPrecision::EightBit
    }
}
