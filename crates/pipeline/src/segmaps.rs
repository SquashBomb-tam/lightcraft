//! AI segmentations for the Sky, Subject, Background and Object masks.
//!
//! A segmentation is a probability map (0..1) over the *oriented source*: the photo after the user's
//! Rotate/Flip, before lens corrections, perspective and crop. The engine computes it once per photo
//! (and per object prompt) with a segmentation model and hands it to the render; here it is
//! resampled through the render's [`Frame`], like the photo itself, so crops, straightening and lens
//! corrections never invalidate it. Its edges are then snapped to the render's own detail with a
//! guided filter, so a map computed at ~1000 px still gives crisp edges in a 60 MP export.

use std::sync::Arc;

use lightcraft_develop::MaskShape;
use lightcraft_geom::Point;
use lightcraft_raster::{Plane, par_rows};

use crate::geometry::Frame;
use crate::masks::guided_cross;

/// The segmentations a render's masks can use. Empty (no models, or nothing computed yet) means
/// the mask stage falls back to its classical estimates.
#[derive(Clone, Debug, Default)]
pub struct Segmentations {
    pub sky: Option<Arc<Plane>>,
    pub subject: Option<Arc<Plane>>,
    /// Object masks, by [`object_key`] of their shape.
    pub objects: Vec<(u64, Arc<Plane>)>,
}

impl Segmentations {
    /// No segmentations.
    pub const NONE: Segmentations = Segmentations { sky: None, subject: None, objects: Vec::new() };

    /// [`Segmentations::NONE`] with a `'static` lifetime.
    pub fn none() -> &'static Segmentations {
        static NONE: Segmentations = Segmentations::NONE;
        &NONE
    }

    pub fn is_empty(&self) -> bool {
        self.sky.is_none() && self.subject.is_none() && self.objects.is_empty()
    }

    /// The map for an Object mask shape.
    pub fn object(&self, shape: &MaskShape) -> Option<&Arc<Plane>> {
        let k = object_key(shape)?;
        self.objects.iter().find(|(o, _)| *o == k).map(|(_, m)| m)
    }
}

/// Identifies an Object mask's prompt (its points and box); `None` for other shapes.
pub fn object_key(shape: &MaskShape) -> Option<u64> {
    let MaskShape::Object { .. } = shape else { return None };
    // FNV-1a over the shape's canonical JSON (the prompt is all an Object shape holds)
    let json = serde_json::to_string(shape).ok()?;
    Some(json.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)))
}

/// Normalized oriented-source coordinates of each output pixel's centre, through crop, flips and
/// the warp (lens corrections, perspective): where [`Frame::sample`] reads the photo.
fn source_norm(frame: &Frame, w: usize, h: usize) -> impl Fn(usize, usize) -> Point + Sync + '_ {
    let o2t = frame.out_to_oriented(w, h);
    let (ow, oh) = (frame.ow.max(1.0), frame.oh.max(1.0));
    move |x, y| {
        let t = o2t.apply(Point::new(x as f64 + 0.5, y as f64 + 0.5));
        let g = match &frame.warp {
            Some(wp) => wp.to_source(t, 1),
            None => t,
        };
        Point::new(g.x / ow, g.y / oh)
    }
}

/// `map` (over the oriented source) resampled into a `w × h` render through `frame`, bilinearly;
/// outside the source the map's nearest edge value is used.
pub fn resample(map: &Plane, frame: &Frame, w: usize, h: usize) -> Plane {
    let mut out = Plane::new(w, h);
    if w == 0 || h == 0 || map.width == 0 || map.height == 0 {
        return out;
    }
    let at = source_norm(frame, w, h);
    let (mw, mh) = (map.width as f64, map.height as f64);
    par_rows(&mut out.data, w, |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            let n = at(x, y);
            *v = bilinear(map, (n.x * mw) as f32, (n.y * mh) as f32).clamp(0.0, 1.0);
        }
    });
    out
}

/// Bilinear sample of `p` at continuous pixel coordinates (pixel centres at +0.5), edge-clamped.
fn bilinear(p: &Plane, x: f32, y: f32) -> f32 {
    let (fx, fy) = (x - 0.5, y - 0.5);
    let (x0, y0) = (fx.floor(), fy.floor());
    let (tx, ty) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as isize, y0 as isize);
    let a = p.get_clamped(x0, y0);
    let b = p.get_clamped(x0 + 1, y0);
    let c = p.get_clamped(x0, y0 + 1);
    let d = p.get_clamped(x0 + 1, y0 + 1);
    let top = a + (b - a) * tx;
    let bot = c + (d - c) * tx;
    top + (bot - top) * ty
}

/// A segmentation as a mask alpha for a `w × h` render: [`resample`]d, then edge-refined against
/// the render's log luminance `log_l` with a guided filter whose radius scales with the render size
/// (`px_per_long`), so previews and exports agree.
pub fn alpha(map: &Plane, frame: &Frame, w: usize, h: usize, log_l: &Plane) -> Plane {
    let coarse = resample(map, frame, w, h);
    if log_l.width != w || log_l.height != h {
        return coarse;
    }
    // ~1.5 map pixels of uncertainty, in render pixels; at least 1 px
    let map_long = map.width.max(map.height).max(1) as f64;
    let sigma = (1.5 * frame.px_per_long(w) / map_long).max(1.0) as f32;
    let fine = guided_cross(log_l, &coarse, sigma, 0.02);
    // trust the model where it is sure; refine only its uncertain band
    let mut out = coarse;
    for (o, f) in out.data.iter_mut().zip(&fine.data) {
        let c = *o;
        let sure = ((c - 0.5).abs() * 2.0).powi(2);
        *o = (c * sure + f * (1.0 - sure)).clamp(0.0, 1.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_develop::DevelopSettings;
    use lightcraft_geom::Orientation;

    /// A map that is 1 in its top half (the "sky") and 0 below.
    fn top_half(w: usize, h: usize) -> Plane {
        Plane::from_fn(w, h, |_, y| if y < h / 2 { 1.0 } else { 0.0 })
    }

    #[test]
    fn resample_follows_the_frame() {
        let s = DevelopSettings::default();
        let f = Frame::new(600, 400, &s, true);
        let a = resample(&top_half(60, 40), &f, 300, 200);
        assert_eq!((a.width, a.height), (300, 200));
        assert!(a.get(150, 20) > 0.99 && a.get(150, 180) < 0.01, "top is sky, bottom isn't");
        assert!((a.get(150, 99) - 0.5).abs() < 0.5 && a.get(150, 90) > 0.9 && a.get(150, 110) < 0.1, "edge at the middle");
        // a crop to the bottom half sees no sky at all
        let mut s = DevelopSettings::default();
        s.crop.geometry.rect = lightcraft_geom::Rect::new(0.0, 0.55, 1.0, 1.0);
        let a = resample(&top_half(60, 40), &Frame::new(600, 400, &s, true), 300, 90);
        assert!(a.data.iter().all(|v| *v < 0.01), "cropped away");
    }

    #[test]
    fn resample_follows_user_orientation() {
        // the map lives on the oriented source: rotating the photo rotates the frame, and the map
        // (computed for that orientation) still lands where its content is
        let s = DevelopSettings { orientation: Orientation::Rotate90, ..DevelopSettings::default() };
        let f = Frame::new(600, 400, &s, true);
        assert_eq!((f.ow, f.oh), (400.0, 600.0));
        let a = resample(&top_half(40, 60), &f, 200, 300);
        assert!(a.get(100, 10) > 0.99 && a.get(100, 290) < 0.01);
    }

    #[test]
    fn empty_inputs_give_empty_or_zero_planes() {
        let f = Frame::new(600, 400, &DevelopSettings::default(), true);
        assert_eq!(resample(&Plane::new(0, 0), &f, 30, 20).data, vec![0.0; 600]);
        assert!(resample(&top_half(6, 4), &f, 0, 0).data.is_empty());
    }

    /// The first row (top to bottom) where column `x` of `a` drops below 0.5.
    fn crossing(a: &Plane, x: usize) -> usize {
        (0..a.height).find(|&y| a.get(x, y) < 0.5).unwrap_or(a.height)
    }

    #[test]
    fn alpha_snaps_a_coarse_edge_to_the_photo() {
        // The photo's horizon is at row 120 of 200 (bright sky above, dark land below). The model,
        // working at a tenth of the size, put a soft edge (1 → 0 over two map rows) about half a
        // map pixel too high, so the coarse mask crosses 50 % near row 115. Refinement must move
        // the crossing onto the photo's edge and keep the confident areas confident.
        let (w, h) = (300, 200);
        let f = Frame::new(600, 400, &DevelopSettings::default(), true);
        let log_l = Plane::from_fn(w, h, |_, y| if y < 120 { 2.0 } else { -3.0 });
        let map = Plane::from_fn(30, 20, |_, y| (1.0 - (y as f32 + 0.5 - 10.5) / 2.0).clamp(0.0, 1.0));
        let coarse = resample(&map, &f, w, h);
        let a = alpha(&map, &f, w, h, &log_l);
        let (c0, c1) = (crossing(&coarse, 150), crossing(&a, 150));
        assert!(c0.abs_diff(120) >= 4, "the test's coarse edge must be off: {c0}");
        assert!(c1.abs_diff(120) <= 2, "refined edge at row {c1} (coarse {c0}), photo edge at 120");
        assert!(a.get(150, 60) > 0.95 && a.get(150, 170) < 0.05, "confident areas stay confident");
        assert!(a.data.iter().all(|v| (0.0..=1.0).contains(v)));
        // where the photo has no edge the model's uncertainty stays: the side it leans to is kept
        let flat = Plane::filled(w, h, 1.0);
        let b = alpha(&map, &f, w, h, &flat);
        assert!((0..h).all(|y| (b.get(150, y) - 0.5).signum() == (coarse.get(150, y) - 0.5).signum() || (coarse.get(150, y) - 0.5).abs() < 0.05));
        // a guide of the wrong size is ignored (no refinement, no panic)
        let b = alpha(&map, &f, w, h, &Plane::new(10, 10));
        assert_eq!(b.data, coarse.data);
    }

    #[test]
    fn object_keys_identify_prompts() {
        let a = MaskShape::Object { hint: vec![Point::new(0.4, 0.5)], bbox: None, exclude: vec![] };
        let b = MaskShape::Object { hint: vec![Point::new(0.41, 0.5)], bbox: None, exclude: vec![] };
        assert_eq!(object_key(&a), object_key(&a.clone()));
        assert_ne!(object_key(&a), object_key(&b));
        assert_eq!(object_key(&MaskShape::Sky), None);
        let seg = Segmentations { objects: vec![(object_key(&a).unwrap(), Arc::new(top_half(4, 4)))], ..Default::default() };
        assert!(seg.object(&a).is_some() && seg.object(&b).is_none() && seg.object(&MaskShape::Subject).is_none());
        assert!(Segmentations::NONE.is_empty() && !seg.is_empty());
    }
}
