//! The installed models on synthetic photos with a known answer. They need the weights:
//! `cargo xtask models --download` (CI does this), then
//! `cargo test -p lightcraft-segment -- --include-ignored`.

use std::time::Instant;

use lightcraft_raster::{Plane, Rgba8};
use lightcraft_segment::{Kind, Prompt, Segmenter, find_models_dir};

fn segmenter() -> Segmenter {
    let dir = find_models_dir(None).expect("models directory (cargo xtask models --download)");
    Segmenter::new(dir)
}

/// A 640 × 480 grey studio shot with a red disc (radius 90) at (400, 260).
fn disc_photo() -> (Rgba8, impl Fn(usize, usize) -> bool) {
    let inside = |x: usize, y: usize| ((x as f64 - 400.0).powi(2) + (y as f64 - 260.0).powi(2)).sqrt() < 90.0;
    let img = Rgba8::from_fn(640, 480, |x, y| {
        // a gentle gradient and a little texture, so it isn't a flat test card
        let g = (90 + (y / 12) as u8).saturating_add(((x * 7 + y * 13) % 9) as u8);
        if inside(x, y) { [200, 40 + ((x + y) % 11) as u8, 35, 255] } else { [g, g, g.saturating_add(4), 255] }
    });
    (img, inside)
}

/// Intersection over union of `p > 0.5` with the true region.
fn iou(p: &Plane, truth: impl Fn(usize, usize) -> bool) -> f64 {
    let (mut i, mut u) = (0usize, 0usize);
    for y in 0..p.height {
        for x in 0..p.width {
            let (a, b) = (p.get(x, y) > 0.5, truth(x, y));
            i += (a && b) as usize;
            u += (a || b) as usize;
        }
    }
    i as f64 / u.max(1) as f64
}

#[test]
#[ignore = "needs the models: cargo xtask models --download"]
fn object_from_a_box_and_from_a_click() {
    let seg = segmenter();
    assert!(seg.has_objects(), "the object model is installed in {}", seg.dir().display());
    let (img, inside) = disc_photo();
    let t = Instant::now();
    let emb = seg.embed(&img).expect("embedding");
    eprintln!("object: embedding {:.2} s", t.elapsed().as_secs_f64());
    // a box around the disc (normalized)
    let t = Instant::now();
    let by_box = seg.object(&emb, &Prompt { points: vec![], bbox: Some([290.0 / 640.0, 150.0 / 480.0, 510.0 / 640.0, 370.0 / 480.0]) }).expect("box");
    eprintln!("object: box decode {:.3} s", t.elapsed().as_secs_f64());
    assert_eq!((by_box.width, by_box.height), (640, 480));
    let a = iou(&by_box, &inside);
    assert!(a > 0.85, "box selects the disc: IoU {a:.3}");
    // a click on it
    let by_click = seg.object(&emb, &Prompt { points: vec![(400.0 / 640.0, 260.0 / 480.0, true)], bbox: None }).expect("click");
    let b = iou(&by_click, &inside);
    assert!(b > 0.85, "a click selects the disc: IoU {b:.3}");
    // a click on the background selects (some of) the background, never the disc
    let bg = seg.object(&emb, &Prompt { points: vec![(60.0 / 640.0, 60.0 / 480.0, true)], bbox: None }).expect("background click");
    assert!(bg.get(400, 260) < 0.5, "the disc is not part of the background selection");
    assert!(bg.data.iter().all(|v| (0.0..=1.0).contains(v)));
}

#[test]
#[ignore = "needs the models: cargo xtask models --download"]
fn object_rejects_bad_input() {
    let seg = segmenter();
    let (img, _) = disc_photo();
    let emb = seg.embed(&img).expect("embedding");
    assert!(seg.object(&emb, &Prompt::default()).is_err(), "an empty prompt is refused");
    assert!(seg.embed(&Rgba8::new(4, 4)).is_err(), "a tiny image is refused");
    // prompts outside the photo are clamped, not a crash
    let p = seg.object(&emb, &Prompt { points: vec![(1.7, -0.4, true)], bbox: None }).expect("clamped prompt");
    assert_eq!((p.width, p.height), (640, 480));
}

/// A 640 × 400 landscape: blue sky above a wavy horizon at y ≈ 175, textured ground, a red disc
/// (radius 70) standing at (420, 250).
fn landscape() -> (Rgba8, impl Fn(usize, usize) -> bool, impl Fn(usize, usize) -> bool) {
    let horizon = |x: usize| 175.0 + 25.0 * (x as f64 / 60.0).sin();
    let disc = |x: usize, y: usize| (x as f64 - 420.0).powi(2) + (y as f64 - 250.0).powi(2) < 70.0f64.powi(2);
    let sky = move |x: usize, y: usize| (y as f64) < horizon(x) && !disc(x, y);
    let img = Rgba8::from_fn(640, 400, |x, y| {
        if disc(x, y) {
            [190, 50, 40, 255]
        } else if (y as f64) < horizon(x) {
            let t = y as f64 / horizon(x);
            [(90.0 + 60.0 * t) as u8, (140.0 + 50.0 * t) as u8, (240.0 - 25.0 * t) as u8, 255]
        } else {
            let tex = (20.0 * (x as f64 * 0.35).sin() * (y as f64 * 0.15).cos()) as i32;
            [(82 + tex) as u8, (70 + tex) as u8, (42 + tex) as u8, 255]
        }
    });
    (img, sky, disc)
}

/// `p` (any size) sampled at `img`-sized coordinates.
fn at(p: &Plane, w: usize, h: usize) -> Plane {
    Plane::from_fn(w, h, |x, y| p.get(x * p.width / w, y * p.height / h))
}

#[test]
#[ignore = "needs the models: cargo xtask models --download"]
fn sky_and_subject_on_a_landscape() {
    let seg = segmenter();
    assert!(seg.has(Kind::Sky) && seg.has(Kind::Subject), "both U²-Net models are installed in {}", seg.dir().display());
    let (img, sky, disc) = landscape();
    for (kind, truth) in [(Kind::Sky, &sky as &dyn Fn(usize, usize) -> bool), (Kind::Subject, &disc)] {
        let t = Instant::now();
        let p = seg.segment(kind, &img).expect("segmentation");
        eprintln!("{}: {:.2} s", kind.name(), t.elapsed().as_secs_f64());
        // the model's working size with the photo's aspect (640 × 400 → 320 × 200)
        assert_eq!((p.width, p.height), (320, 200), "{}", kind.name());
        assert!(p.data.iter().all(|v| (0.0..=1.0).contains(v)));
        let a = iou(&at(&p, 640, 400), truth);
        assert!(a > 0.9, "{}: IoU {a:.3}", kind.name());
    }
}

#[test]
#[ignore = "needs the models: cargo xtask models --download"]
fn segmentation_refuses_bad_input_and_missing_models() {
    let seg = segmenter();
    assert!(seg.segment(Kind::Sky, &Rgba8::new(4, 4)).is_err(), "a tiny image is refused");
    assert!(seg.segment(Kind::Subject, &Rgba8 { width: 10, height: 10, data: vec![] }).is_err(), "a malformed image is refused");
    // a very wide photo still gives a usable map
    let p =
        seg.segment(Kind::Sky, &Rgba8::from_fn(1000, 20, |_, y| if y < 10 { [120, 170, 240, 255] } else { [70, 60, 40, 255] })).expect("panorama");
    assert_eq!((p.width, p.height), (320, 6));
    // no models installed: a clear error, not a crash
    let empty = std::env::temp_dir().join(format!("lightcraft-no-models-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let none = Segmenter::new(&empty);
    assert!(!none.has(Kind::Sky) && !none.has(Kind::Subject) && !none.has_objects());
    let (img, _, _) = landscape();
    let e = none.segment(Kind::Sky, &img).expect_err("no sky model");
    assert!(e.to_string().contains("not installed"), "{e}");
    assert!(none.embed(&img).is_err());
    // a damaged model file is an error too
    std::fs::write(empty.join("subject.safetensors"), b"not a model").unwrap();
    assert!(none.has(Kind::Subject));
    assert!(none.segment(Kind::Subject, &img).is_err(), "a damaged model is refused");
    let _ = std::fs::remove_dir_all(&empty);
}
