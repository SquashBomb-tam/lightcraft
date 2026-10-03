//! AI segmentations for the Sky, Subject, Background and Object masks.
//!
//! A render job whose settings use an AI mask carries a [`SegJob`]. When the job runs (on a render
//! worker, never the UI thread) it asks the [`SegService`] for the photo's maps: from memory, from
//! the library's `masks/` folder, or — the first time — from the model. Concurrent jobs for the
//! same map wait for the one computing it. Maps live over the *oriented source* (see
//! `lightcraft_pipeline::segmaps`), so they stay valid through any crop, straighten or lens
//! correction; they depend only on the photo's pixels, its Rotate/Flip, the model version and
//! (Object masks) the prompt.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use lightcraft_develop::{DevelopSettings, MaskShape};
use lightcraft_geom::Point;
use lightcraft_pipeline::geometry::Frame;
use lightcraft_pipeline::{RenderRequest, Segmentations, SourceInfo};
use lightcraft_raster::{Plane, Rgb32f, Rgba8};
use lightcraft_segment::{Embedding, Kind, Prompt, Segmenter};

/// Long edge of the image a segmentation is computed from (sources smaller than this — grid
/// thumbnails — give a provisional map that a larger source later replaces).
pub const INPUT_EDGE: usize = 1024;

/// Maps and embeddings kept in memory.
const MAX_MAPS: usize = 48;
const MAX_EMBEDDINGS: usize = 3;

type Slot<T> = Arc<OnceLock<Result<T, String>>>;

/// The installed models plus the segmentations computed with them.
pub struct SegService {
    seg: Segmenter,
    maps: Mutex<Entries<Arc<Plane>>>,
    embeddings: Mutex<Entries<Embedding>>,
    disk: Mutex<Option<PathBuf>>,
    /// Model runs in progress (the UI shows "Detecting…").
    running: AtomicUsize,
}

/// Counts a model run for [`SegService::busy`] while alive.
struct Running<'a>(&'a AtomicUsize);

impl<'a> Running<'a> {
    fn start(n: &'a AtomicUsize) -> Self {
        n.fetch_add(1, Ordering::SeqCst);
        Running(n)
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Results by key with a use counter for eviction; an entry is a slot that one thread fills while
/// others wait on it.
struct Entries<T> {
    map: HashMap<u64, (Slot<T>, u64)>,
    tick: u64,
    cap: usize,
}

impl<T> Entries<T> {
    fn new(cap: usize) -> Self {
        Entries { map: HashMap::new(), tick: 0, cap }
    }

    /// The slot for `key` (new and empty if there is none), evicting the least recently used
    /// finished entries beyond the capacity.
    fn slot(&mut self, key: u64) -> Slot<T> {
        self.tick += 1;
        let tick = self.tick;
        let slot = self.map.entry(key).or_insert_with(|| (Arc::new(OnceLock::new()), tick));
        slot.1 = tick;
        let s = slot.0.clone();
        while self.map.len() > self.cap {
            // only finished entries go: a slot being computed has waiters
            let Some(old) = self.map.iter().filter(|(_, (s, _))| s.get().is_some()).min_by_key(|(_, (_, t))| *t).map(|(k, _)| *k) else { break };
            self.map.remove(&old);
        }
        s
    }

    fn get(&mut self, key: u64) -> Option<Slot<T>> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(&key).map(|e| {
            e.1 = tick;
            e.0.clone()
        })
    }
}

impl std::fmt::Debug for SegService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SegService").field("models", &self.seg.dir()).finish()
    }
}

impl SegService {
    pub fn new(seg: Segmenter) -> Self {
        SegService {
            seg,
            maps: Mutex::new(Entries::new(MAX_MAPS)),
            embeddings: Mutex::new(Entries::new(MAX_EMBEDDINGS)),
            disk: Mutex::new(None),
            running: AtomicUsize::new(0),
        }
    }

    /// Whether a model is running right now.
    pub fn busy(&self) -> bool {
        self.running.load(Ordering::SeqCst) > 0
    }

    pub fn segmenter(&self) -> &Segmenter {
        &self.seg
    }

    /// Keep computed maps in `dir` too (the library's `masks/` folder), so they survive restarts.
    pub fn set_disk(&self, dir: Option<PathBuf>) {
        *self.disk.lock().unwrap_or_else(|e| e.into_inner()) = dir;
    }

    /// Whether `s` has a mask this service could compute a segmentation for.
    pub fn needed(&self, s: &DevelopSettings) -> bool {
        ai_shapes(s).any(|sh| match sh {
            MaskShape::Sky => self.seg.has(Kind::Sky),
            MaskShape::Subject | MaskShape::Background => self.seg.has(Kind::Subject),
            MaskShape::Object { .. } => self.seg.has_objects(),
            _ => false,
        })
    }

    /// A map: from memory, else disk, else `compute` (then stored in both). Concurrent callers for
    /// the same `key` share one computation.
    fn map(&self, key: u64, compute: &dyn Fn() -> Result<Plane, String>) -> Result<Arc<Plane>, String> {
        let slot = self.maps.lock().unwrap_or_else(|e| e.into_inner()).slot(key);
        slot.get_or_init(|| {
            if let Some(p) = self.disk_path(key).and_then(|f| read_map(&f).ok()) {
                return Ok(Arc::new(p));
            }
            let p = {
                let _running = Running::start(&self.running);
                compute()?
            };
            if let Some(f) = self.disk_path(key)
                && let Err(e) = write_map(&f, &p)
            {
                log::warn!("segment: can't keep {}: {e}", f.display());
            }
            Ok(Arc::new(p))
        })
        .clone()
    }

    /// A map already in memory or on disk (without computing it).
    fn cached_map(&self, key: u64) -> Option<Arc<Plane>> {
        if let Some(s) = self.maps.lock().unwrap_or_else(|e| e.into_inner()).get(key)
            && let Some(Ok(p)) = s.get()
        {
            return Some(p.clone());
        }
        let p = Arc::new(read_map(&self.disk_path(key)?).ok()?);
        let slot = self.maps.lock().unwrap_or_else(|e| e.into_inner()).slot(key);
        let _ = slot.set(Ok(p.clone()));
        Some(p)
    }

    fn embedding(&self, key: u64, compute: impl FnOnce() -> Result<Embedding, String>) -> Result<Embedding, String> {
        let slot = self.embeddings.lock().unwrap_or_else(|e| e.into_inner()).slot(key);
        slot.get_or_init(|| {
            let _running = Running::start(&self.running);
            compute()
        })
        .clone()
    }

    fn disk_path(&self, key: u64) -> Option<PathBuf> {
        self.disk.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|d| d.join(format!("{key:016x}.seg")))
    }
}

/// The AI shapes among `s`'s masks (visible or not: a hidden mask's overlay still shows it).
fn ai_shapes(s: &DevelopSettings) -> impl Iterator<Item = &MaskShape> {
    s.masks
        .iter()
        .flat_map(|m| m.components.iter().map(|c| &c.shape))
        .filter(|sh| matches!(sh, MaskShape::Sky | MaskShape::Subject | MaskShape::Background | MaskShape::Object { .. }))
}

/// What a render job needs from the [`SegService`].
#[derive(Clone)]
pub struct SegJob {
    pub service: Arc<SegService>,
    /// The photo's content key ([`crate::media::content_key`]).
    pub content: String,
}

impl std::fmt::Debug for SegJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SegJob").field("content", &self.content).finish()
    }
}

impl SegJob {
    /// The segmentations `s`'s AI masks use, for a render of `src`: cached ones, else computed now
    /// (this blocks the calling render worker for the model's run time). A model that fails is
    /// logged and left out: the pipeline then falls back to its classical estimate.
    pub fn resolve(&self, src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings) -> Segmentations {
        let svc = &self.service;
        let seg = &svc.seg;
        let full = src.width.max(src.height) >= INPUT_EDGE;
        let edge = src.width.max(src.height).min(INPUT_EDGE);
        let input: OnceLock<Rgba8> = OnceLock::new();
        let input = || input.get_or_init(|| model_input(src, info, s, edge));
        let base = |what: &str, tier_full: bool| {
            let mut h = 0xcbf2_9ce4_8422_2325u64;
            for part in [
                self.content.as_str(),
                what,
                &format!("{:?}", s.orientation),
                &format!("{:x}", seg.revision()),
                if tier_full { "full" } else { "small" },
            ] {
                for b in part.bytes().chain([0u8]) {
                    h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
                }
            }
            h
        };
        // a provisional (small-source) map serves until a full one exists; a full source always
        // gets a full map
        let get = |what: &str, compute: &dyn Fn() -> Result<Plane, String>| -> Option<Arc<Plane>> {
            let r = if full {
                svc.map(base(what, true), compute)
            } else if let Some(p) = svc.cached_map(base(what, true)) {
                Ok(p)
            } else {
                svc.map(base(what, false), compute)
            };
            r.map_err(|e| log::warn!("segment: {what}: {e}")).ok()
        };
        let mut out = Segmentations::default();
        let shapes: Vec<&MaskShape> = ai_shapes(s).collect();
        if shapes.iter().any(|sh| matches!(sh, MaskShape::Sky)) && seg.has(Kind::Sky) {
            out.sky = get("sky", &|| seg.segment(Kind::Sky, input()).map_err(|e| e.to_string()));
        }
        if shapes.iter().any(|sh| matches!(sh, MaskShape::Subject | MaskShape::Background)) && seg.has(Kind::Subject) {
            out.subject = get("subject", &|| seg.segment(Kind::Subject, input()).map_err(|e| e.to_string()));
        }
        if seg.has_objects() {
            let frame = Frame::with_lens(src.width, src.height, s, false, info.lens.as_ref());
            for sh in shapes.iter().filter(|sh| matches!(sh, MaskShape::Object { .. })) {
                let (Some(key), Some(prompt)) = (lightcraft_pipeline::segmaps::object_key(sh), source_prompt(sh, &frame)) else { continue };
                if out.objects.iter().any(|(k, _)| *k == key) {
                    continue;
                }
                let what = format!("object:{}", prompt_text(&prompt));
                let emb_key = base("embedding", full);
                let compute = || {
                    let emb = svc.embedding(emb_key, || seg.embed(input()).map_err(|e| e.to_string()))?;
                    seg.object(&emb, &prompt).map_err(|e| e.to_string())
                };
                if let Some(m) = get(&what, &compute) {
                    out.objects.push((key, m));
                }
            }
        }
        out
    }
}

/// The image the models see: the photo with default settings and its Rotate/Flip, no lens
/// corrections, no crop, long edge `edge`, display-referred sRGB.
pub fn model_input(src: &Rgb32f, info: &SourceInfo, s: &DevelopSettings, edge: usize) -> Rgba8 {
    let settings = DevelopSettings { orientation: s.orientation, ..DevelopSettings::default() };
    let info = SourceInfo { lens: None, ..*info };
    let req = RenderRequest { apply_crop: false, ..RenderRequest::fit(edge, edge) };
    lightcraft_pipeline::render(src, &info, &settings, &req).image
}

/// An Object shape's prompt moved from the render's normalized (transformed) coordinates into the
/// oriented source's, through the lens/perspective warp.
pub fn source_prompt(shape: &MaskShape, frame: &Frame) -> Option<Prompt> {
    let MaskShape::Object { hint, bbox, exclude } = shape else { return None };
    let (ow, oh) = (frame.ow.max(1.0), frame.oh.max(1.0));
    let to_src = |p: Point| {
        let t = Point::new(p.x * ow, p.y * oh);
        let g = frame.warp.as_ref().map_or(t, |w| w.to_source(t, 1));
        ((g.x / ow).clamp(0.0, 1.0), (g.y / oh).clamp(0.0, 1.0))
    };
    let mut points: Vec<(f64, f64, bool)> = hint.iter().map(|p| to_src(*p)).map(|(x, y)| (x, y, true)).collect();
    points.extend(exclude.iter().map(|p| to_src(*p)).map(|(x, y)| (x, y, false)));
    let bbox = bbox.map(|[x0, y0, x1, y1]| {
        let (a, b) = (to_src(Point::new(x0, y0)), to_src(Point::new(x1, y1)));
        [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
    });
    let p = Prompt { points, bbox };
    (!p.is_empty()).then_some(p)
}

fn prompt_text(p: &Prompt) -> String {
    format!(
        "{:?}{:?}",
        p.points.iter().map(|(x, y, b)| (format!("{x:.5}"), format!("{y:.5}"), b)).collect::<Vec<_>>(),
        p.bbox.map(|b| b.map(|v| format!("{v:.5}")))
    )
}

const MAGIC: &[u8; 8] = b"LCSEG\x001\x00";

/// A map on disk: magic, width and height (u32 LE), then the values quantized to bytes, deflated.
fn write_map(path: &Path, p: &Plane) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let bytes: Vec<u8> = p.data.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    let mut out = Vec::with_capacity(16 + bytes.len() / 4);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(p.width as u32).to_le_bytes());
    out.extend_from_slice(&(p.height as u32).to_le_bytes());
    out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(&bytes, 6));
    // write then rename, so a crash never leaves a half map behind
    let tmp = path.with_extension("seg.tmp");
    std::fs::File::create(&tmp)?.write_all(&out)?;
    std::fs::rename(&tmp, path)
}

fn read_map(path: &Path) -> std::io::Result<Plane> {
    let mut buf = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut buf)?;
    let bad = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{}: {what}", path.display()));
    if buf.len() < 16 || &buf[..8] != MAGIC {
        return Err(bad("not a segmentation map"));
    }
    let w = u32::from_le_bytes(buf[8..12].try_into().unwrap_or_default()) as usize;
    let h = u32::from_le_bytes(buf[12..16].try_into().unwrap_or_default()) as usize;
    if w == 0 || h == 0 || w * h > 1 << 26 {
        return Err(bad("bad size"));
    }
    let bytes = miniz_oxide::inflate::decompress_to_vec_with_limit(&buf[16..], w * h).map_err(|_| bad("corrupt data"))?;
    if bytes.len() != w * h {
        return Err(bad("truncated"));
    }
    Ok(Plane { width: w, height: h, data: bytes.iter().map(|b| *b as f32 / 255.0).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use lightcraft_develop::{Mask, MaskComponent, MaskOp};

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-seg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A service whose models aren't installed (an empty folder).
    fn service(name: &str) -> (SegService, PathBuf) {
        let d = scratch(name);
        (SegService::new(Segmenter::new(d.join("models"))), d)
    }

    fn ramp(w: usize, h: usize) -> Plane {
        Plane::from_fn(w, h, |x, y| ((x + 2 * y) % 17) as f32 / 16.0)
    }

    fn close(a: &Plane, b: &Plane) -> bool {
        (a.width, a.height) == (b.width, b.height) && a.data.iter().zip(&b.data).all(|(x, y)| (x - y).abs() <= 0.5 / 255.0 + 1e-6)
    }

    #[test]
    fn maps_round_trip_through_disk() {
        let d = scratch("roundtrip");
        for p in [ramp(37, 23), Plane::from_fn(1, 1, |_, _| 1.0), Plane::from_fn(5, 4, |x, _| if x % 2 == 0 { -3.0 } else { 9.0 })] {
            let f = d.join("m.seg");
            write_map(&f, &p).unwrap();
            let back = read_map(&f).unwrap();
            // out-of-range values are stored clamped to 0..1
            let want = Plane { data: p.data.iter().map(|v| v.clamp(0.0, 1.0)).collect(), ..p.clone() };
            assert!(close(&back, &want), "{}×{}", p.width, p.height);
            assert!(!d.join("m.seg.tmp").exists(), "no temporary file is left behind");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn damaged_map_files_are_rejected() {
        let d = scratch("damaged");
        let good = d.join("good.seg");
        write_map(&good, &ramp(64, 48)).unwrap();
        let bytes = std::fs::read(&good).unwrap();
        let mut huge = bytes.clone();
        huge[8..12].copy_from_slice(&100_000u32.to_le_bytes());
        huge[12..16].copy_from_slice(&100_000u32.to_le_bytes());
        let mut zero = bytes.clone();
        zero[8..12].copy_from_slice(&0u32.to_le_bytes());
        let mut bigger = bytes.clone();
        bigger[8..12].copy_from_slice(&65u32.to_le_bytes());
        let cases: [(&str, Vec<u8>); 7] = [
            ("empty", vec![]),
            ("not a map", b"PNG\x89 something else entirely".to_vec()),
            ("header only", bytes[..16].to_vec()),
            ("truncated data", bytes[..bytes.len() / 2].to_vec()),
            ("absurd size", huge),
            ("zero width", zero),
            ("size larger than the data", bigger),
        ];
        for (what, b) in cases {
            let f = d.join("bad.seg");
            std::fs::write(&f, b).unwrap();
            assert!(read_map(&f).is_err(), "{what}");
        }
        assert!(read_map(&d.join("missing.seg")).is_err(), "a missing file");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn concurrent_requests_share_one_computation() {
        let (svc, d) = service("shared");
        let runs = AtomicUsize::new(0);
        let compute = || {
            runs.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(80));
            Ok(ramp(8, 8))
        };
        let results: Vec<Arc<Plane>> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..6).map(|_| s.spawn(|| svc.map(42, &compute).unwrap())).collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert_eq!(runs.load(Ordering::SeqCst), 1, "one model run for six requests");
        assert!(results.iter().all(|r| Arc::ptr_eq(r, &results[0])), "everyone gets the same map");
        // a different key is a different computation
        svc.map(43, &compute).unwrap();
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn busy_while_a_model_runs() {
        let (svc, d) = service("busy");
        assert!(!svc.busy());
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel::<()>();
        std::thread::scope(|s| {
            let svc = &svc;
            let h = s.spawn(move || {
                svc.map(7, &|| {
                    started_tx.send(()).unwrap();
                    finish_rx.recv().unwrap();
                    Ok(ramp(4, 4))
                })
            });
            started_rx.recv().unwrap();
            assert!(svc.busy(), "busy during the run");
            finish_tx.send(()).unwrap();
            h.join().unwrap().unwrap();
        });
        assert!(!svc.busy(), "idle afterwards");
        // a failing run doesn't leave it busy either
        assert!(svc.map(8, &|| Err("the model failed".into())).is_err());
        assert!(!svc.busy());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn maps_persist_in_the_library_folder() {
        let (svc, d) = service("persist");
        let masks = d.join("masks");
        svc.set_disk(Some(masks.clone()));
        let original = svc.map(0xabc, &|| Ok(ramp(30, 20))).unwrap();
        assert!(masks.join(format!("{:016x}.seg", 0xabc)).is_file());
        // a new session (the app restarted) reads it back instead of running the model
        let (again, d2) = service("persist-2");
        again.set_disk(Some(masks.clone()));
        let read = again.map(0xabc, &|| panic!("the model must not run again")).unwrap();
        assert!(close(&read, &original));
        assert!(again.cached_map(0xabc).is_some());
        assert!(again.cached_map(0xdef).is_none(), "nothing is invented for an unknown key");
        // a failure is reported and nothing is written
        assert_eq!(again.map(0xdef, &|| Err("no".into())).unwrap_err(), "no");
        assert!(!masks.join(format!("{:016x}.seg", 0xdef)).exists());
        // a damaged file on disk is recomputed rather than used
        std::fs::write(masks.join(format!("{:016x}.seg", 0x111)), b"junk").unwrap();
        let fresh = again.map(0x111, &|| Ok(ramp(3, 3))).unwrap();
        assert_eq!((fresh.width, fresh.height), (3, 3));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    #[test]
    fn eviction_drops_the_oldest_finished_entries_only() {
        let mut e: Entries<u32> = Entries::new(2);
        let a = e.slot(1);
        a.set(Ok(1)).unwrap();
        let pending = e.slot(2); // being computed: never evicted
        let c = e.slot(3);
        c.set(Ok(3)).unwrap();
        assert!(e.get(1).is_none(), "the oldest finished entry went");
        assert!(e.get(2).is_some() && e.get(3).is_some());
        let _ = e.slot(4);
        assert!(e.get(2).is_some(), "the unfinished entry stays even over capacity");
        drop(pending);
    }

    fn settings_with(shape: MaskShape) -> DevelopSettings {
        let mut s = DevelopSettings::default();
        s.masks.push(Mask { components: vec![MaskComponent { op: MaskOp::Add, invert: false, shape }], ..Mask::default() });
        s
    }

    #[test]
    fn nothing_is_needed_or_computed_without_models() {
        let (svc, d) = service("none");
        let svc = Arc::new(svc);
        let object = MaskShape::Object { hint: vec![Point::new(0.5, 0.5)], bbox: None, exclude: vec![] };
        for shape in [MaskShape::Sky, MaskShape::Subject, MaskShape::Background, object] {
            let s = settings_with(shape);
            assert!(!svc.needed(&s));
            let job = SegJob { service: svc.clone(), content: "photo".into() };
            let src = Rgb32f::filled(64, 48, [0.4, 0.5, 0.6]);
            let out = job.resolve(&src, &SourceInfo::default(), &s);
            assert!(out.is_empty(), "no maps: the pipeline uses its classical estimates");
        }
        assert!(!svc.needed(&DevelopSettings::default()));
        assert!(!svc.busy());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn object_prompts_move_into_source_coordinates() {
        let s = DevelopSettings::default();
        let frame = Frame::with_lens(400, 300, &s, false, None);
        // no geometry: the same coordinates; a box drawn right-to-left is normalized
        let shape = MaskShape::Object { hint: vec![Point::new(0.25, 0.5)], bbox: Some([0.8, 0.9, 0.2, 0.1]), exclude: vec![Point::new(0.9, 0.1)] };
        let p = source_prompt(&shape, &frame).unwrap();
        assert_eq!(p.points.len(), 2);
        let (x, y, on) = p.points[0];
        assert!((x - 0.25).abs() < 1e-9 && (y - 0.5).abs() < 1e-9 && on);
        assert!(!p.points[1].2, "the exclude point is off the object");
        let b = p.bbox.unwrap();
        assert!((b[0] - 0.2).abs() < 1e-9 && (b[1] - 0.1).abs() < 1e-9 && (b[2] - 0.8).abs() < 1e-9 && (b[3] - 0.9).abs() < 1e-9, "{b:?}");
        // points off the photo are clamped onto it
        let off = MaskShape::Object { hint: vec![Point::new(1.5, -0.2)], bbox: None, exclude: vec![] };
        let (x, y, _) = source_prompt(&off, &frame).unwrap().points[0];
        assert_eq!((x, y), (1.0, 0.0));
        // nothing to prompt with, or not an object
        assert!(source_prompt(&MaskShape::Object { hint: vec![], bbox: None, exclude: vec![] }, &frame).is_none());
        assert!(source_prompt(&MaskShape::Sky, &frame).is_none());
    }
}
