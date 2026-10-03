//! Getting a photo into a model's input and its output back: resizing, letterboxing.

use lightcraft_raster::resample::{Filter, resize};
use lightcraft_raster::{Plane, Rgb32f, Rgba8};

/// `(w, h)` scaled so its long edge is `long` (aspect kept, each side at least 1).
pub fn fit_long_edge(w: usize, h: usize, long: usize) -> (usize, usize) {
    let (w, h) = (w.max(1) as f64, h.max(1) as f64);
    let k = long as f64 / w.max(h);
    (((w * k).round() as usize).max(1), ((h * k).round() as usize).max(1))
}

/// `img` as 0..1 RGB floats, resized to `w × h` (Mitchell filter: area-correct when shrinking).
pub fn resized(img: &Rgba8, w: usize, h: usize) -> Rgb32f {
    let f = Rgb32f { width: img.width, height: img.height, data: img.data.iter().map(|p| [p[0], p[1], p[2]].map(|v| v as f32 / 255.0)).collect() };
    let mut r = resize(&f, w, h, Filter::Mitchell);
    r.data.iter_mut().for_each(|p| *p = p.map(|v| v.clamp(0.0, 1.0)));
    r
}

/// `img` scaled so its long edge is `size`, placed at the top left of a `size × size` canvas filled
/// with `pad` (the Segment Anything input layout). Returns the canvas and the scaled image's size.
pub fn letterbox(img: &Rgba8, size: usize, pad: [f32; 3]) -> (Rgb32f, (usize, usize)) {
    let (w, h) = fit_long_edge(img.width, img.height, size);
    let r = resized(img, w, h);
    let mut out = Rgb32f::filled(size, size, pad);
    for y in 0..h {
        out.data[y * size..y * size + w].copy_from_slice(&r.data[y * w..(y + 1) * w]);
    }
    (out, (w, h))
}

/// The top-left `w × h` of `p`, resized to `out_w × out_h` (bilinear), clamped to 0..1.
pub fn crop_resize(p: &Plane, w: usize, h: usize, out_w: usize, out_h: usize) -> Plane {
    let (w, h) = (w.clamp(1, p.width.max(1)), h.clamp(1, p.height.max(1)));
    let crop = Plane::from_fn(w, h, |x, y| p.get(x, y));
    let mut r = resize(&crop, out_w, out_h, Filter::Bilinear);
    r.data.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0));
    r
}

/// Logistic sigmoid.
pub fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(w: usize, h: usize, f: impl Fn(usize, usize) -> [u8; 4]) -> Rgba8 {
        Rgba8::from_fn(w, h, f)
    }

    #[test]
    fn long_edge_fits() {
        assert_eq!(fit_long_edge(6000, 4000, 1024), (1024, 683));
        assert_eq!(fit_long_edge(4000, 6000, 1024), (683, 1024));
        assert_eq!(fit_long_edge(10, 10, 320), (320, 320));
        // degenerate sizes still give a usable shape
        assert_eq!(fit_long_edge(0, 0, 64), (64, 64));
        assert_eq!(fit_long_edge(10000, 1, 64), (64, 1));
    }

    #[test]
    fn letterbox_pads_bottom_and_right() {
        // left half red, right half blue, 40 × 20
        let img = rgba(40, 20, |x, _| if x < 20 { [255, 0, 0, 255] } else { [0, 0, 255, 255] });
        let (c, (w, h)) = letterbox(&img, 16, [0.5, 0.5, 0.5]);
        assert_eq!((c.width, c.height, w, h), (16, 16, 16, 8));
        assert!(c.get(2, 2)[0] > 0.9 && c.get(2, 2)[2] < 0.1, "red at the left");
        assert!(c.get(13, 2)[2] > 0.9 && c.get(13, 2)[0] < 0.1, "blue at the right");
        assert_eq!(c.get(5, 12), [0.5, 0.5, 0.5], "padding below the image");
        assert!(c.data.iter().flatten().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn crop_resize_undoes_letterboxing() {
        // a 16 × 16 output whose top 8 rows hold a horizontal ramp
        let p = Plane::from_fn(16, 16, |x, y| if y < 8 { x as f32 / 15.0 } else { 7.0 });
        let r = crop_resize(&p, 16, 8, 40, 20);
        assert_eq!((r.width, r.height), (40, 20));
        assert!(r.get(0, 10) < 0.1 && r.get(39, 10) > 0.9, "the ramp spans the image");
        assert!(r.data.iter().all(|v| (0.0..=1.0).contains(v)), "padding never leaks in, values clamp");
        // asking for more than the plane holds is clamped, not a panic
        assert_eq!(crop_resize(&p, 100, 100, 4, 4).width, 4);
    }

    #[test]
    fn sigmoid_is_a_probability() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-6);
        assert!(sigmoid(20.0) > 0.999 && sigmoid(-20.0) < 0.001);
        assert!(sigmoid(-200.0) >= 0.0 && sigmoid(200.0) <= 1.0);
    }
}
