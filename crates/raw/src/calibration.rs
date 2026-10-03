//! Built-in colour calibrations for cameras whose raw files carry no colour matrix.
//!
//! Non-DNG raws don't describe the sensor's colour response in a form [`crate::color`] can use, so they fall back
//! to the neutral "camera RGB ≈ linear sRGB" model (`matrix_is_fallback`), which renders them greyish and
//! hue-shifted. This table supplies a `ColorMatrix1`-style matrix (XYZ → camera at a calibration illuminant) per
//! camera model. Every entry is our own measurement from the camera's own data, never an Adobe or other
//! third-party matrix. Files that carry their own matrix (DNG) always keep it.
//!
//! # Sony α7R IV / α7R IVA (`ILCE-7RM4`, `ILCE-7RM4A`)
//!
//! Fitted (eight degrees of freedom: the matrix is scale-free) to two kinds of data the camera itself records, from
//! six CC0 sample files on raw.pixls.us (four α7R IV and two α7R IVA frames: a landscape at dusk and a garden in
//! daylight):
//!
//! 1. **White locus.** Every ARW stores the white-balance levels of the camera's Kelvin presets. For the 2500,
//!    3200, 4500, 6000 and 8500 K presets (averaged over both bodies, which differ by ≈ 3 % in blue) the matrix
//!    must map the Planckian white of that temperature to the camera neutral `1 / levels`. This pins how whites
//!    are treated, and with them saturation along the warm–cool axis. The matrix reproduces every preset within
//!    0.3 %, so Temperature reads on the camera's own Kelvin scale.
//! 2. **Hue.** The camera's embedded JPEG preview of each frame, registered to the raw data and compared in
//!    8 × 8-pixel blocks by OkLab hue angle. Hue only: Sony's tone curve, Creative Style and DRO change lightness
//!    and saturation but largely keep hue.
//!
//! Fits on the landscape frames alone and on the garden frames alone agree within 1.4 % per entry. Cross-checked,
//! not fitted, against the widely published DNG reference matrix for this model on the 24 ColorChecker patches
//! under D65: mean OkLab distance 0.015 (the neutral fallback: 0.058), at most 0.046, skin patches ≤ 0.006,
//! median chroma 94 % of the reference's (fallback: 45 %). Under 3200 K: mean 0.018. Least certain: very
//! saturated blues and purples, which the sample scenes barely contain.

use crate::{Mat3, RawImage};

struct Calibration {
    /// Exif `Make`, upper case; matched as a prefix.
    make: &'static str,
    /// Exif `Model` values (case-insensitive) sharing this calibration.
    models: &'static [&'static str],
    /// XYZ → camera at `illuminant`, × 10 000 (DNG `ColorMatrix1` layout).
    color_matrix: [[i32; 3]; 3],
    /// Exif `LightSource` code of the calibration illuminant (21 = D65).
    illuminant: u16,
}

const TABLE: &[Calibration] = &[Calibration {
    make: "SONY",
    models: &["ILCE-7RM4", "ILCE-7RM4A"],
    color_matrix: [[6802, -1534, -1299], [-6641, 14929, 1270], [-3237, 4578, 4605]],
    illuminant: 21,
}];

/// The built-in `(ColorMatrix1, CalibrationIlluminant1)` for a camera, if we have calibrated it.
pub fn color_matrix(make: &str, model: &str) -> Option<(Mat3, u16)> {
    let (make, model) = (make.trim().to_ascii_uppercase(), model.trim());
    let c = TABLE.iter().find(|c| make.starts_with(c.make) && c.models.iter().any(|m| m.eq_ignore_ascii_case(model)))?;
    Some((Mat3(c.color_matrix.map(|r| r.map(|v| v as f64 / 10_000.0))), c.illuminant))
}

/// Give `img` its camera's built-in calibration when the file carries no colour matrix of its own.
pub(crate) fn apply(img: &mut RawImage) {
    if crate::color::has_matrix(&img.color) {
        return;
    }
    let (Some(make), Some(model)) = (img.metadata.make.as_deref(), img.metadata.model.as_deref()) else { return };
    if let Some((m, illuminant)) = color_matrix(make, model) {
        img.color.color_matrix = [Some(m), None];
        img.color.illuminant = [illuminant, 0];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{camera_transform, neutral_to_xy};
    use crate::{ColorData, RawData, RawFormat};
    use lightcraft_color::cct;

    fn sony(make: Option<&str>, model: Option<&str>) -> RawImage {
        RawImage {
            format: RawFormat::Arw,
            width: 2,
            height: 2,
            cpp: 1,
            data: RawData::U16(vec![0; 4]),
            cfa: crate::Cfa::bayer("RGGB"),
            bits: 14,
            black: crate::BlackLevel::uniform(512.0),
            white: vec![16383.0],
            active_area: crate::Rect::new(0, 0, 2, 2),
            crop: crate::Rect::new(0, 0, 2, 2),
            orientation: lightcraft_geom::Orientation::Normal,
            color: ColorData::default(),
            wb_multipliers: Some([3.0, 1.0, 1.5]),
            linearized: false,
            opcodes: crate::OpcodeLists::default(),
            metadata: crate::Metadata { make: make.map(Into::into), model: model.map(Into::into), ..Default::default() },
        }
    }

    #[test]
    fn lookup_by_make_and_model() {
        assert!(color_matrix("SONY", "ILCE-7RM4").is_some());
        assert!(color_matrix("Sony ", " ilce-7rm4a").is_some());
        assert!(color_matrix("SONY", "ILCE-7RM5").is_none());
        assert!(color_matrix("SONY", "ILCE-7RM4 II").is_none());
        assert!(color_matrix("CANON", "ILCE-7RM4").is_none());
        assert!(color_matrix("", "").is_none());
        assert!(color_matrix("SONY", "").is_none());
    }

    /// The camera's own Kelvin white-balance presets (α7R IV and α7R IVA bodies) must read back as their labels.
    #[test]
    fn sony_a7r4_reads_its_own_kelvin_presets() {
        let (m, illuminant) = color_matrix("SONY", "ILCE-7RM4").unwrap();
        let color = ColorData { color_matrix: [Some(m), None], illuminant: [illuminant, 0], ..ColorData::default() };
        let presets: [(f64, [[f64; 3]; 2]); 5] = [
            (2500.0, [[1308.0, 1024.0, 3700.0], [1312.0, 1024.0, 3828.0]]),
            (3200.0, [[1696.0, 1024.0, 2660.0], [1700.0, 1024.0, 2760.0]]),
            (4500.0, [[2248.0, 1024.0, 1916.0], [2252.0, 1024.0, 1996.0]]),
            (6000.0, [[2696.0, 1024.0, 1576.0], [2704.0, 1024.0, 1644.0]]),
            (8500.0, [[3188.0, 1024.0, 1328.0], [3196.0, 1024.0, 1388.0]]),
        ];
        for (kelvin, bodies) in presets {
            for levels in bodies {
                let (t, tint) = cct::xy_to_temp_tint(neutral_to_xy(&color, levels.map(|l| 1024.0 / l)));
                assert!((t / kelvin - 1.0).abs() < 0.05, "{kelvin} K preset reads {t:.0} K");
                assert!(tint.abs() < 8.0, "{kelvin} K preset reads tint {tint:.1}");
            }
        }
    }

    #[test]
    fn applies_only_without_a_matrix_of_its_own() {
        let mut img = sony(Some("SONY"), Some("ILCE-7RM4"));
        assert!(camera_transform(&img, lightcraft_color::D65).matrix_is_fallback);
        apply(&mut img);
        assert!(!camera_transform(&img, lightcraft_color::D65).matrix_is_fallback);

        // a matrix the file brings itself wins
        let own = Mat3([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        img.color = ColorData { color_matrix: [Some(own), None], illuminant: [21, 0], ..ColorData::default() };
        apply(&mut img);
        assert_eq!(img.color.color_matrix[0], Some(own));
    }

    #[test]
    fn leaves_unknown_and_unnamed_cameras_on_the_fallback() {
        for (make, model) in [(None, Some("ILCE-7RM4")), (Some("SONY"), None), (None, None), (Some(""), Some("")), (Some("SONY"), Some("ILCE-1"))] {
            let mut img = sony(make, model);
            apply(&mut img);
            assert_eq!(img.color, ColorData::default(), "{make:?} {model:?}");
            assert!(camera_transform(&img, lightcraft_color::D65).matrix_is_fallback);
        }
    }

    /// The calibrated matrix must give a usable transform for the whites a photographer actually meets.
    #[test]
    fn transform_is_finite_and_keeps_white_from_candle_to_shade() {
        let mut img = sony(Some("SONY"), Some("ILCE-7RM4A"));
        apply(&mut img);
        for kelvin in [2000.0, 2850.0, 4000.0, 5500.0, 6500.0, 7500.0, 10_000.0] {
            let t = camera_transform(&img, cct::temp_tint_to_xy(kelvin, 0.0));
            assert!(t.matrix.0.iter().flatten().all(|v| v.is_finite()), "{kelvin} K");
            assert!(t.wb.iter().all(|v| v.is_finite() && *v >= 1.0), "{kelvin} K: {:?}", t.wb);
            let white = t.matrix.apply([1.0; 3]);
            assert!(white.iter().all(|v| (v - 1.0).abs() < 1e-9), "{kelvin} K: white maps to {white:?}");
        }
    }
}
