use glam::{Mat3, Vec2, Vec3};

pub(crate) const WP_D65: Vec2 = Vec2::new(0.3127, 0.3290);

pub(crate) const PRIMARIES_SRGB: [Vec2; 3] = [
    Vec2::new(0.64, 0.33),
    Vec2::new(0.30, 0.60),
    Vec2::new(0.15, 0.06),
];

pub(crate) const PRIMARIES_REC2020: [Vec2; 3] = [
    Vec2::new(0.708, 0.292),
    Vec2::new(0.170, 0.797),
    Vec2::new(0.131, 0.046),
];

pub(crate) const PRIMARIES_DISPLAY_P3: [Vec2; 3] = [
    Vec2::new(0.680, 0.320),
    Vec2::new(0.265, 0.690),
    Vec2::new(0.150, 0.060),
];

/// The colour space of an export. The editing pipeline works in linear sRGB primaries, but values may go
/// outside that gamut (negative channels) when a wide output space is asked for: the colours the camera
/// recorded beyond sRGB are rebuilt from the RAW develop pass's gamut compression (see `gamut_residual`
/// in the main shader) and the result is converted to this space.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputSpace {
    #[default]
    #[serde(rename = "srgb")]
    Srgb,
    #[serde(rename = "p3")]
    DisplayP3,
    #[serde(rename = "rec2020")]
    Rec2020,
}

impl OutputSpace {
    /// Linear sRGB (Rec.709 primaries, D65) to this space's linear RGB.
    pub fn from_srgb(self) -> Mat3 {
        let primaries = match self {
            OutputSpace::Srgb => return Mat3::IDENTITY,
            OutputSpace::DisplayP3 => &PRIMARIES_DISPLAY_P3,
            OutputSpace::Rec2020 => &PRIMARIES_REC2020,
        };
        primaries_to_xyz_matrix(primaries, WP_D65).inverse()
            * primaries_to_xyz_matrix(&PRIMARIES_SRGB, WP_D65)
    }

    /// Colour primaries code of ITU-T H.273 (CICP).
    pub fn cicp_primaries(self) -> u8 {
        match self {
            OutputSpace::Srgb => 1,
            OutputSpace::DisplayP3 => 12,
            OutputSpace::Rec2020 => 9,
        }
    }

    pub fn is_wide(self) -> bool {
        self != OutputSpace::Srgb
    }
}

fn xy_to_xyz(xy: Vec2) -> Vec3 {
    if xy.y < 1e-6 {
        Vec3::ZERO
    } else {
        Vec3::new(xy.x / xy.y, 1.0, (1.0 - xy.x - xy.y) / xy.y)
    }
}

pub(crate) fn primaries_to_xyz_matrix(primaries: &[Vec2; 3], white_point: Vec2) -> Mat3 {
    let r_xyz = xy_to_xyz(primaries[0]);
    let g_xyz = xy_to_xyz(primaries[1]);
    let b_xyz = xy_to_xyz(primaries[2]);
    let primaries_matrix = Mat3::from_cols(r_xyz, g_xyz, b_xyz);
    let white_point_xyz = xy_to_xyz(white_point);
    let s = primaries_matrix.inverse() * white_point_xyz;
    Mat3::from_cols(r_xyz * s.x, g_xyz * s.y, b_xyz * s.z)
}

fn rotate_and_scale_primary(primary: Vec2, white_point: Vec2, scale: f32, rotation: f32) -> Vec2 {
    let p_rel = primary - white_point;
    let p_scaled = p_rel * scale;
    let (sin_r, cos_r) = rotation.sin_cos();
    let p_rotated = Vec2::new(
        p_scaled.x * cos_r - p_scaled.y * sin_r,
        p_scaled.x * sin_r + p_scaled.y * cos_r,
    );
    white_point + p_rotated
}

pub fn calculate_agx_matrices_glam() -> (Mat3, Mat3) {
    let pipe_work_profile_to_xyz = primaries_to_xyz_matrix(&PRIMARIES_SRGB, WP_D65);
    let base_profile_to_xyz = primaries_to_xyz_matrix(&PRIMARIES_REC2020, WP_D65);
    let xyz_to_base_profile = base_profile_to_xyz.inverse();
    let pipe_to_base = xyz_to_base_profile * pipe_work_profile_to_xyz;

    let inset = [0.294_624_5, 0.25861925, 0.14641371];
    let rotation = [0.03540329, -0.02108586, -0.06305724];
    let outset = [0.290_776_4, 0.263_155_4, 0.045_810_72];
    let unrotation = [0.03540329, -0.02108586, -0.06305724];
    let master_outset_ratio = 1.0;
    let master_unrotation_ratio = 0.0;

    let mut inset_and_rotated_primaries = [Vec2::ZERO; 3];
    for i in 0..3 {
        inset_and_rotated_primaries[i] =
            rotate_and_scale_primary(PRIMARIES_REC2020[i], WP_D65, 1.0 - inset[i], rotation[i]);
    }
    let rendering_to_xyz = primaries_to_xyz_matrix(&inset_and_rotated_primaries, WP_D65);
    let base_to_rendering = xyz_to_base_profile * rendering_to_xyz;

    let mut outset_and_unrotated_primaries = [Vec2::ZERO; 3];
    for i in 0..3 {
        outset_and_unrotated_primaries[i] = rotate_and_scale_primary(
            PRIMARIES_REC2020[i],
            WP_D65,
            1.0 - master_outset_ratio * outset[i],
            master_unrotation_ratio * unrotation[i],
        );
    }
    let outset_to_xyz = primaries_to_xyz_matrix(&outset_and_unrotated_primaries, WP_D65);
    let temp_matrix = xyz_to_base_profile * outset_to_xyz;
    let rendering_to_base = temp_matrix.inverse();

    let pipe_to_rendering = base_to_rendering * pipe_to_base;
    let rendering_to_pipe = pipe_to_base.inverse() * rendering_to_base;

    (pipe_to_rendering, rendering_to_pipe)
}

#[cfg(test)]
mod output_space_tests {
    use super::*;

    #[test]
    fn white_stays_white_and_srgb_is_identity() {
        for space in [OutputSpace::Srgb, OutputSpace::DisplayP3, OutputSpace::Rec2020] {
            let white = space.from_srgb() * Vec3::ONE;
            assert!((white - Vec3::ONE).abs().max_element() < 1e-4, "{space:?}: {white:?}");
        }
        assert_eq!(OutputSpace::Srgb.from_srgb(), Mat3::IDENTITY);
    }

    #[test]
    fn matrices_match_the_published_ones() {
        // sRGB to Display P3 (e.g. the CSS Color 4 reference) and to Rec.2020 (ITU-R BT.2087)
        let p3 = OutputSpace::DisplayP3.from_srgb();
        assert!((p3.x_axis.x - 0.8225).abs() < 2e-3 && (p3.x_axis.y - 0.0332).abs() < 2e-3);
        assert!((p3.z_axis.z - 0.9105).abs() < 2e-3);
        let bt2020 = OutputSpace::Rec2020.from_srgb();
        assert!((bt2020.x_axis.x - 0.6274).abs() < 2e-3 && (bt2020.y_axis.y - 0.9195).abs() < 2e-3);
    }

    #[test]
    fn a_colour_outside_srgb_becomes_valid_in_a_wide_space() {
        // a saturated cyan outside sRGB: negative red in linear sRGB
        let cyan = Vec3::new(-0.15, 0.6, 0.5);
        let p3 = OutputSpace::DisplayP3.from_srgb() * cyan;
        let bt2020 = OutputSpace::Rec2020.from_srgb() * cyan;
        assert!(p3.min_element() > -0.02, "{p3:?}");
        assert!(bt2020.min_element() > 0.0, "{bt2020:?}");
    }
}
