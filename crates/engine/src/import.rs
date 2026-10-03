//! Import: files and folders (recursive) → photos.
//!
//! 1. Expand folders recursively into supported files (hidden entries and the library's own
//!    folder are skipped), sorted for a stable order.
//! 2. Skip paths already in the catalog.
//! 3. Probe the rest — dimensions, metadata, and the **content hash** of the bytes — in parallel
//!    on native targets.
//! 4. Skip **duplicates by content** (same bytes already in the library, or twice in this batch).
//! 5. *Add* in place (the photo points at the original file) or *copy* into the library's
//!    `Originals/YYYY/YYYY-MM-DD/` folder (names made unique) and point at the copy.
//! 6. Read each photo's XMP sidecar (or a raw/DNG file's embedded XMP): the sidecar wins for
//!    metadata and develop settings are restored (see [`crate::sidecar`]).
//! 7. Optionally apply a preset and add keywords (the import dialog's options).
//! 8. Commit all new photos as one undoable op.
//!
//! The import dialog first calls [`scan`] (`library.importPreview`): the same expansion, probing
//! and duplicate detection without adding anything, so the user can review the candidates. The
//! probes are kept and reused by the import that follows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use serde::{Deserialize, Serialize};

use crate::Session;
use crate::media::ProbeInfo;

/// File extensions LightCraft imports (lower case).
pub const EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "tif", "tiff", "webp", "dng", "cr2", "cr3", "nef", "nrw", "arw", "raf", "orf", "rw2", "pef", "psd", "jxl", "gif", "bmp",
    "heic", "avif",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImportMode {
    /// Reference the files where they are.
    #[default]
    Add,
    /// Copy the files into the library's `Originals/` folder.
    Copy,
}

/// What an import does besides adding the photos.
#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
    pub mode: ImportMode,
    /// Applied to every imported photo (one History entry).
    pub preset: Option<lightcraft_develop::Preset>,
    /// Added to every imported photo.
    pub keywords: Vec<String>,
    /// Browsing a folder: photos come in as `local` (not in the library), and a file with the
    /// same content as one already known is still listed.
    pub local: bool,
}

/// A file found by [`scan`], for the import review.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImportCandidate {
    pub path: String,
    pub name: String,
    pub format: String,
    pub kind: MediaKind,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub captured: Option<String>,
    /// `"path"` (already in the library), `"content"` (same bytes in the library or earlier in this
    /// list): skipped by the import.
    pub duplicate: Option<String>,
    /// The photo that already has it.
    pub existing: Option<u64>,
    /// Not readable.
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Duplicate {
    pub path: String,
    /// The photo that already has these bytes (or this path).
    pub existing: Option<u64>,
    /// `"path"` (already imported from there) or `"content"` (same bytes elsewhere).
    pub reason: &'static str,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ImportReport {
    pub imported: Vec<u64>,
    pub duplicates: Vec<Duplicate>,
    /// (path, error)
    pub failed: Vec<(String, String)>,
    /// Files found after expanding folders.
    pub scanned: usize,
    /// Photos whose metadata/develop settings were read from an XMP sidecar (or embedded XMP).
    pub sidecars: usize,
}

/// The develop settings a photo gets on import: raws start from their as-shot white balance with
/// default sharpening / colour noise reduction, and file-embedded lens corrections on (as the
/// camera intended); a user default preset ([`ImportDefaults`]) goes on top.
pub fn import_defaults(p: &Photo) -> lightcraft_develop::DevelopSettings {
    p.import_defaults()
}

/// Does the photo still look as the camera rendered it (its embedded camera preview is then a
/// fair stand-in)? Not when a default preset changed it on import.
pub fn has_import_look(p: &Photo) -> bool {
    *p.develop == p.camera_defaults()
}

/// User defaults applied on import (Settings → Import; saved in the library's `prefs.json`).
/// Presets are referenced by id; a preset that no longer exists is ignored.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImportDefaults {
    /// Preset applied to raw files (`None` = the LightCraft default).
    pub raw_preset: Option<String>,
    /// Use a camera's own default (below) when the photo's camera has one.
    pub per_camera: bool,
    /// Per-camera raw defaults, keyed by the camera's make + model as shown in Info
    /// (`"Canon EOS R5"`).
    pub cameras: Vec<CameraDefault>,
    /// Preset applied to non-raw images (JPEG, PNG, TIFF, HEIC…; `None` = none).
    pub other_preset: Option<String>,
    /// Copyright notice given to imported photos that don't carry one (empty = none).
    pub copyright: String,
    /// Creator given to imported photos that don't name one (empty = none).
    pub creator: String,
    /// Metadata preset applied to every imported photo (`metadata.*`; `None` = none).
    pub metadata_preset: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CameraDefault {
    /// Make + model (`Meta::camera`).
    pub camera: String,
    /// Preset id; `None` = the LightCraft default for this camera.
    pub preset: Option<String>,
}

impl ImportDefaults {
    /// The preset id that applies to a photo of this kind and camera, if any.
    pub fn preset_for(&self, raw: bool, camera: &str) -> Option<&str> {
        if !raw {
            return self.other_preset.as_deref();
        }
        if self.per_camera
            && let Some(c) = self.cameras.iter().find(|c| !camera.is_empty() && c.camera.eq_ignore_ascii_case(camera))
        {
            return c.preset.as_deref();
        }
        self.raw_preset.as_deref()
    }
}

/// Give a freshly imported photo its default settings: the camera defaults, then the matching
/// default preset ([`ImportDefaults::preset_for`]), remembered as the photo's import look.
pub fn apply_import_defaults(s: &Session, p: &mut Photo) {
    // metadata defaults fill gaps only: the file's own copyright / creator win
    let d = &s.import_defaults;
    if p.meta.copyright.trim().is_empty() && !d.copyright.trim().is_empty() {
        p.meta.copyright = d.copyright.trim().to_string();
    }
    if p.meta.creator.trim().is_empty() && !d.creator.trim().is_empty() {
        p.meta.creator = d.creator.trim().to_string();
    }
    if let Some(mp) = d.metadata_preset.as_ref().and_then(|n| s.metadata_presets.iter().find(|m| m.name.eq_ignore_ascii_case(n))) {
        crate::cmd::metadata::apply_to(&mut p.meta, &mp.fields);
    }
    let base = p.camera_defaults();
    let raw = p.kind == lightcraft_catalog::MediaKind::Raw;
    let preset = s.import_defaults.preset_for(raw, &p.meta.camera).and_then(|id| s.presets.iter().find(|x| x.id == id));
    match preset {
        Some(pr) => {
            let look = std::sync::Arc::new(pr.apply(&base, 1.0));
            p.develop = look.clone();
            p.import_look = (*look != base).then_some(look);
        }
        None => {
            p.develop = std::sync::Arc::new(base);
            p.import_look = None;
        }
    }
}

pub fn is_supported(path: &Path) -> bool {
    path.extension().is_some_and(|e| EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// Expand files and folders (recursively) into supported files. `skip` (e.g. the library folder)
/// is never descended into.
pub fn expand(paths: &[String], skip: Option<&Path>) -> Vec<String> {
    fn walk(p: &Path, skip: Option<&Path>, out: &mut Vec<String>, top: bool) {
        let hidden = p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
        if (hidden && !top) || skip.is_some_and(|s| p == s) {
            return;
        }
        if p.is_dir() {
            let Ok(rd) = std::fs::read_dir(p) else { return };
            let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            v.sort();
            for c in v {
                walk(&c, skip, out, false);
            }
        } else if top || is_supported(p) {
            // explicitly named files are attempted even with an unknown extension (sniffed)
            out.push(p.to_string_lossy().to_string());
        }
    }
    let mut out = Vec::new();
    for p in paths {
        walk(Path::new(p), skip, &mut out, true);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

fn probe_all(s: &Session, paths: &[String]) -> Vec<Result<ProbeInfo, String>> {
    let Some(probe) = s.media.file_probe.clone() else {
        return paths
            .iter()
            .map(|p| {
                Ok(ProbeInfo {
                    format: Path::new(p).extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default(),
                    ..Default::default()
                })
            })
            .collect();
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8).min(paths.len().max(1));
        if n > 1 {
            let mut results: Vec<Option<Result<ProbeInfo, String>>> = vec![None; paths.len()];
            let next = std::sync::atomic::AtomicUsize::new(0);
            let out = std::sync::Mutex::new(&mut results);
            std::thread::scope(|sc| {
                for _ in 0..n {
                    sc.spawn(|| {
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if i >= paths.len() {
                                break;
                            }
                            let r = probe(&paths[i]);
                            out.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(r);
                        }
                    });
                }
            });
            return results.into_iter().map(|r| r.unwrap_or_else(|| Err("not probed".into()))).collect();
        }
    }
    paths.iter().map(|p| probe(p)).collect()
}

/// `Originals/YYYY/YYYY-MM-DD/name`, made unique.
fn copy_into_library(lib: &Path, src: &str, date: &str) -> Result<String, String> {
    let day = date.get(..10).filter(|d| d.len() == 10).unwrap_or("undated");
    let year = day.get(..4).unwrap_or("undated");
    let dir = lib.join("Originals").join(year).join(day);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = Path::new(src).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "photo".into());
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.clone(), String::new()),
    };
    let mut dst = dir.join(&name);
    let mut i = 1;
    while dst.exists() {
        dst = dir.join(format!("{stem}-{i}{ext}"));
        i += 1;
    }
    std::fs::copy(src, &dst).map_err(|e| format!("copy {src}: {e}"))?;
    Ok(dst.to_string_lossy().to_string())
}

/// Find and probe what importing `paths` would add, marking duplicates (by path, or by content
/// against the library and earlier candidates). Nothing is added; the probes are kept for the
/// import that follows.
pub fn scan(s: &mut Session, paths: &[String]) -> Vec<ImportCandidate> {
    let lib_dir = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
    let files = expand(paths, lib_dir.as_deref());
    let mut by_path: HashMap<String, PhotoId> = HashMap::new();
    let mut by_hash: HashMap<String, PhotoId> = HashMap::new();
    for p in s.catalog.photos() {
        if let Source::File { path } = &p.source {
            by_path.insert(path.clone(), p.id);
        }
        if let Some(h) = &p.content_hash {
            by_hash.insert(h.clone(), p.id);
        }
    }
    let todo: Vec<String> = files.iter().filter(|f| !by_path.contains_key(f.as_str())).cloned().collect();
    // probes from a preceding `scan` are reused when the file is unchanged (same size)
    let cached: Vec<Option<ProbeInfo>> = todo
        .iter()
        .map(|f| {
            let info = s.import_probes.remove(f)?;
            let size = std::fs::metadata(f).map(|m| m.len()).ok();
            (size.is_none() || size == Some(info.file_size)).then_some(info)
        })
        .collect();
    let missing: Vec<String> = todo.iter().zip(&cached).filter(|(_, c)| c.is_none()).map(|(f, _)| f.clone()).collect();
    let mut fresh = probe_all(s, &missing).into_iter();
    let probed: Vec<Result<ProbeInfo, String>> =
        cached.into_iter().map(|c| c.map(Ok).unwrap_or_else(|| fresh.next().unwrap_or_else(|| Err("not probed".into())))).collect();
    crate::memory::release();
    let mut probes: HashMap<String, Result<ProbeInfo, String>> = todo.into_iter().zip(probed).collect();
    let mut seen_hash: HashMap<String, Option<u64>> = by_hash.into_iter().map(|(h, id)| (h, Some(id.0))).collect();
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        let name = Path::new(&f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone());
        let mut c = ImportCandidate { path: f.clone(), name, ..Default::default() };
        if let Some(id) = by_path.get(&f) {
            c.duplicate = Some("path".into());
            c.existing = Some(id.0);
            if let Some(p) = s.catalog.photo(*id) {
                (c.format, c.kind, c.width, c.height, c.file_size, c.captured) =
                    (p.format.clone(), p.kind, p.width, p.height, p.file_size, p.captured.clone());
            }
            out.push(c);
            continue;
        }
        match probes.remove(&f) {
            Some(Ok(info)) => {
                (c.format, c.kind, c.width, c.height, c.file_size, c.captured) =
                    (info.format.clone(), info.kind, info.width, info.height, info.file_size, info.captured.clone());
                if let Some(h) = &info.content_hash {
                    match seen_hash.get(h) {
                        Some(existing) => {
                            c.duplicate = Some("content".into());
                            c.existing = *existing;
                        }
                        None => {
                            seen_hash.insert(h.clone(), None);
                        }
                    }
                }
                s.import_probes.insert(f, info);
            }
            Some(Err(e)) => c.error = Some(e),
            None => c.error = Some("not probed".into()),
        }
        out.push(c);
    }
    out
}

/// Import files/folders. See the module docs.
pub fn import(s: &mut Session, paths: &[String], mode: ImportMode) -> crate::Result<ImportReport> {
    import_with(s, paths, &ImportOptions { mode, ..Default::default() })
}

/// Import files/folders with options (preset, keywords). See the module docs.
pub fn import_with(s: &mut Session, paths: &[String], opts: &ImportOptions) -> crate::Result<ImportReport> {
    let mode = opts.mode;
    // Only a library on disk has an `Originals/` folder. The browser build keeps the bytes of every
    // added file in its own storage already, so "copy" there means "add".
    let mode = if mode == ImportMode::Copy && s.library.as_ref().is_some_and(|l| !l.on_disk) { ImportMode::Add } else { mode };
    let lib_dir = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
    if mode == ImportMode::Copy && lib_dir.is_none() {
        return Err(crate::EngineError::Other("copying into the library needs an open library".into()));
    }
    let files = expand(paths, lib_dir.as_deref());
    let mut report = ImportReport { scanned: files.len(), ..Default::default() };

    // existing paths and content hashes
    let mut by_path: HashMap<&str, PhotoId> = HashMap::new();
    let mut by_hash: HashMap<String, PhotoId> = HashMap::new();
    for p in s.catalog.photos() {
        if let Source::File { path } = &p.source {
            by_path.insert(path.as_str(), p.id);
        }
        if let Some(h) = &p.content_hash {
            by_hash.insert(h.clone(), p.id);
        }
    }
    let mut todo = Vec::new();
    // a file that was only browsed (Local) joins the library when it is imported for real
    let mut promote = Vec::new();
    for f in files {
        match by_path.get(f.as_str()) {
            Some(id) if !opts.local && s.catalog.photo(*id).is_some_and(|p| p.local) => {
                promote.push(Op::SetLocal { id: *id, local: false });
                report.imported.push(id.0);
            }
            Some(id) => report.duplicates.push(Duplicate { path: f, existing: Some(id.0), reason: "path" }),
            None => todo.push(f),
        }
    }

    let probed = probe_all(s, &todo);
    crate::memory::release();
    let now = (s.clock)();
    let mut ops = promote;
    for (path, info) in todo.into_iter().zip(probed) {
        let info = match info {
            Ok(i) => i,
            Err(e) => {
                log::warn!("import {path}: {e}");
                report.failed.push((path, e));
                continue;
            }
        };
        if let Some(h) = &info.content_hash
            && !opts.local
            && let Some(id) = by_hash.get(h)
        {
            report.duplicates.push(Duplicate { path, existing: Some(id.0), reason: "content" });
            continue;
        }
        let stored = match (mode, &lib_dir) {
            (ImportMode::Copy, Some(lib)) if !Path::new(&path).starts_with(lib) => {
                match copy_into_library(lib, &path, info.captured.as_deref().unwrap_or(&now)) {
                    Ok(p) => p,
                    Err(e) => {
                        report.failed.push((path, e));
                        continue;
                    }
                }
            }
            _ => path.clone(),
        };
        let id = s.catalog.alloc_photo_id();
        if let Some(h) = &info.content_hash {
            by_hash.insert(h.clone(), id);
        }
        let name = Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.clone());
        let mut p = Photo::new(id, Source::File { path: stored }, &name, &info.format, info.width, info.height, &now);
        p.kind = info.kind;
        p.file_size = info.file_size;
        p.captured = info.captured;
        p.meta = info.meta;
        p.as_shot_wb = info.as_shot_wb;
        p.content_hash = info.content_hash;
        p.embedded_lens = info.embedded_lens;
        apply_import_defaults(s, &mut p);
        let raw = p.kind == lightcraft_catalog::MediaKind::Raw;
        let packet = crate::sidecar::find_sidecar(&path, s.xmp.naming)
            .and_then(|f| std::fs::read_to_string(f).ok())
            .or_else(|| info.xmp.clone().filter(|_| raw));
        if let Some(x) = packet {
            match crate::sidecar::parse_sidecar(&x, raw) {
                Ok(sc) if sc != crate::sidecar::SidecarData::default() => {
                    crate::sidecar::merge_into(&mut p, &sc, &now);
                    report.sidecars += 1;
                }
                Ok(_) => {}
                Err(e) => log::warn!("import {path}: XMP: {e}"),
            }
        }
        for k in &opts.keywords {
            let k = lightcraft_catalog::keywords::clean(k);
            if !k.is_empty() && !p.meta.keywords.iter().any(|x| x.eq_ignore_ascii_case(&k)) {
                p.meta.keywords.push(k);
            }
        }
        if let Some(preset) = &opts.preset {
            let d = std::sync::Arc::new(preset.apply(&p.develop, 1.0));
            let label = format!("Preset: {}", preset.name);
            p.develop = d.clone();
            p.edited = Some(now.clone());
            p.history.push(lightcraft_catalog::HistoryStep { label, settings: d });
        }
        report.imported.push(id.0);
        p.local = opts.local;
        ops.push(Op::AddPhoto { photo: Box::new(p) });
    }
    if !ops.is_empty() {
        s.commit(&format!("Add {} Photo{}", ops.len(), if ops.len() == 1 { "" } else { "s" }), Op::Batch { ops })?;
    }
    Ok(report)
}

/// The current local time as ISO 8601 (`YYYY-MM-DDTHH:MM:SS`, UTC on targets without a clock
/// offset); for [`Session::clock`] on native hosts.
pub fn system_clock() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        civil(secs)
    }
    #[cfg(target_arch = "wasm32")]
    {
        "2026-01-01T00:00:00".to_string()
    }
}

/// Unix seconds → `YYYY-MM-DDTHH:MM:SS` (UTC), proleptic Gregorian.
pub fn civil(secs: i64) -> String {
    lightcraft_catalog::dates::civil(secs)
}

impl Session {
    /// A thumbnail for an import candidate (not in the catalog): a raw's embedded preview, else a
    /// small render of the file. `slot` makes the job's photo id unique (`u64::MAX - slot`).
    pub fn candidate_thumb_job(&mut self, c: &ImportCandidate, edge: usize, slot: u64) -> Option<crate::media::QuickJob> {
        if c.error.is_some() {
            return None;
        }
        let id = PhotoId(u64::MAX - slot);
        let mut p = Photo::new(id, Source::File { path: c.path.clone() }, &c.name, &c.format, c.width.max(1), c.height.max(1), "");
        p.kind = c.kind;
        if let Some(info) = self.import_probes.get(&c.path) {
            p.as_shot_wb = info.as_shot_wb;
            p.embedded_lens = info.embedded_lens;
        }
        p.develop = std::sync::Arc::new(p.import_defaults());
        let edge = edge.clamp(64, crate::media::SourceLevel::Thumb.max_edge());
        let level = crate::media::SourceLevel::Thumb;
        let source = self.media.origin_ref(&p.source, level.max_edge());
        let key = lightcraft_preview::Hasher128::new().str(&c.path).u64(c.file_size).u64(edge as u64).finish().0 as u64;
        let small = crate::media::RenderJob {
            photo: id,
            level,
            source,
            origin: p.source.clone(),
            info: crate::media::source_info(&p),
            settings: p.develop.clone(),
            request: lightcraft_pipeline::RenderRequest::fit(edge, edge),
            key,
            cache: None,
            stages: None,
            view_cache: None,
            seg: None,
        };
        let embedded = match (&self.media.preview_loader, c.kind) {
            (Some(l), MediaKind::Raw) => Some((c.path.clone(), l.clone(), edge)),
            _ => None,
        };
        Some(crate::media::QuickJob { photo: id, key, cached: Vec::new(), embedded, small: Some(Box::new(small)) })
    }

    /// Use the system clock for import/edit times (native hosts).
    pub fn with_system_clock(mut self) -> Self {
        self.clock = Box::new(system_clock);
        self
    }
}
