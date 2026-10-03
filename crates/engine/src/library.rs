//! A persistent library: a directory holding the catalog (op log + snapshot), user presets, the
//! last view state and the preview cache.
//!
//! ```text
//! LightCraft Library/
//!   catalog.snap   catalog.log      (lightcraft-catalog journal)
//!   presets.json   view.json        (user presets + favourites; last source/filter/sort/selection)
//!   prefs.json     (library preferences: XMP sidecars, import defaults, cache size, last export)
//!   thumbs/        (rendered thumbnail cache, safe to delete)
//!   masks/         (AI mask segmentations: sky, subject, objects; safe to delete)
//!   Originals/     (photos imported with "copy into library")
//! ```
//!
//! The files live in [`Store`]s: a directory ([`FsStore`]) natively, or any other implementation
//! via [`Session::open_library_in`] (the browser build keeps them in OPFS or IndexedDB).
//!
//! Every top-level [`Session::execute`] persists the ops it produced (fsynced) before returning,
//! so a crash loses at most the command in flight. The log is compacted into a snapshot when it
//! grows (see [`lightcraft_catalog::SnapshotPolicy`]) and on [`Session::close_library`].

use std::path::{Path, PathBuf};

use lightcraft_catalog::{FsStore, Journal, LoadReport, Store};
use lightcraft_develop::Preset;
use serde::{Deserialize, Serialize};

use crate::{EngineError, LibrarySource, Result, Selection, Session};

/// Library directory name inside the user's Pictures folder.
pub const DEFAULT_NAME: &str = "LightCraft Library";

/// The default library location: `$LIGHTCRAFT_LIBRARY` if set, else `~/Pictures/LightCraft Library`
/// (`%USERPROFILE%\Pictures\LightCraft Library` on Windows).
pub fn default_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("LIGHTCRAFT_LIBRARY").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let home = if cfg!(windows) { std::env::var_os("USERPROFILE") } else { std::env::var_os("HOME") }?;
    Some(PathBuf::from(home).join("Pictures").join(DEFAULT_NAME))
}

pub struct Library {
    /// The library directory, or a descriptive pseudo-path when the library isn't on disk.
    pub dir: PathBuf,
    /// The library is a real directory: photos can be copied into `Originals/` and thumbnails are
    /// cached in `thumbs/`.
    pub on_disk: bool,
    journal: Journal,
    /// presets.json, view.json, prefs.json.
    files: Box<dyn Store>,
    /// What happened when the catalog was loaded.
    pub report: LoadReport,
    /// Last persistence error (shown by the UI; ops stay pending and are retried).
    pub last_error: Option<String>,
    presets_written: String,
    view_written: Vec<u8>,
}

/// Where a library's files live (see [`Session::open_library_in`]).
pub struct LibraryStores {
    /// Shown as the library's location (`library.info`).
    pub dir: PathBuf,
    /// The catalog journal (`catalog.snap`, `catalog.log`).
    pub catalog: Box<dyn Store>,
    /// presets.json, view.json, prefs.json.
    pub files: Box<dyn Store>,
    /// `dir` is a real directory (enables `Originals/` copies and the `thumbs/` disk cache).
    pub on_disk: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PresetsFile {
    user: Vec<Preset>,
    favorites: Vec<String>,
    /// Favourite profiles, and the recently applied ones (newest first).
    profile_favorites: Vec<String>,
    profile_recent: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct ViewFile {
    source: LibrarySource,
    browse: Option<crate::Browse>,
    filter: lightcraft_catalog::Filter,
    sort: lightcraft_catalog::Sort,
    selection: Selection,
}

impl Library {
    pub fn journal(&self) -> &Journal {
        &self.journal
    }
    pub fn thumbs_dir(&self) -> PathBuf {
        self.dir.join("thumbs")
    }
    pub fn originals_dir(&self) -> PathBuf {
        self.dir.join("Originals")
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PrefsFile {
    xmp: crate::sidecar::XmpPrefs,
    /// Parameters of the last export (for Export with Previous).
    #[serde(alias = "last_export")]
    last_export: Option<serde_json::Value>,
    /// The user's export presets.
    export_presets: Vec<crate::export::ExportPreset>,
    /// Metadata presets.
    metadata_presets: Vec<crate::cmd::metadata::MetadataPreset>,
    /// Filter presets.
    filter_presets: Vec<crate::cmd::filters::FilterPreset>,
    /// Develop defaults for imported photos.
    import: crate::import::ImportDefaults,
    /// Thumbnail disk cache budget (MB, 0 = default).
    cache_mb: u32,
}

fn presets_json(s: &Session) -> String {
    let presets = &s.presets;
    let f = PresetsFile {
        user: presets.iter().filter(|p| !p.builtin).cloned().collect(),
        favorites: presets.iter().filter(|p| p.builtin && p.favorite).map(|p| p.id.clone()).collect(),
        profile_favorites: s.profile_favorites.clone(),
        profile_recent: s.profile_recent.clone(),
    };
    serde_json::to_string_pretty(&f).unwrap_or_default()
}

fn read_json<T: serde::de::DeserializeOwned>(store: &mut dyn Store, name: &str) -> Option<T> {
    store.read(name).ok().flatten().and_then(|b| serde_json::from_slice(&b).ok())
}

impl Session {
    /// Open (or create) the library at `dir` into this session, replacing its catalog. With
    /// `seed_demo`, a newly created library starts with the procedural demo photos.
    pub fn open_library(&mut self, dir: impl AsRef<Path>, seed_demo: bool) -> Result<&LoadReport> {
        let dir = dir.as_ref().to_path_buf();
        let open = || FsStore::open(&dir).map_err(|e| EngineError::Other(format!("can't open library {}: {e}", dir.display())));
        let stores = LibraryStores { catalog: Box::new(open()?), files: Box::new(open()?), on_disk: true, dir: dir.clone() };
        self.open_library_in(stores, seed_demo)
    }

    /// Open (or create) a library whose files live in `stores` (e.g. browser storage), replacing
    /// this session's catalog.
    pub fn open_library_in(&mut self, stores: LibraryStores, seed_demo: bool) -> Result<&LoadReport> {
        let LibraryStores { dir, catalog, mut files, on_disk } = stores;
        let (mut journal, catalog, report) = Journal::open(catalog)?;
        self.catalog = catalog;
        self.undo.clear();
        self.redo.clear();
        self.interaction = None;
        self.pending_log.clear();
        self.selection = Selection::default();
        self.source = LibrarySource::All;
        if report.created && seed_demo {
            crate::demo::load(self);
            journal.snapshot(&self.catalog)?;
        }
        // presets
        if let Some(f) = read_json::<PresetsFile>(files.as_mut(), "presets.json") {
            for p in &mut self.presets {
                p.favorite = p.builtin && f.favorites.contains(&p.id) || (!p.builtin && p.favorite);
            }
            for u in f.user {
                if !self.presets.iter().any(|p| p.id == u.id) {
                    self.presets.push(u);
                }
            }
            let known = |id: &String| crate::presets::profile(id).is_some();
            self.profile_favorites = f.profile_favorites.into_iter().filter(known).collect();
            self.profile_recent = f.profile_recent.into_iter().filter(known).take(crate::presets::RECENT_PROFILES).collect();
        }
        // preferences
        let prefs = read_json::<PrefsFile>(files.as_mut(), "prefs.json").unwrap_or_default();
        self.xmp = prefs.xmp;
        self.last_export = prefs.last_export;
        self.export_presets = prefs.export_presets;
        self.metadata_presets = prefs.metadata_presets;
        self.filter_presets = prefs.filter_presets;
        self.import_defaults = prefs.import;
        self.cache_mb = prefs.cache_mb;
        // view state
        if let Some(v) = read_json::<ViewFile>(files.as_mut(), "view.json") {
            self.source = v.source;
            self.browse = v.browse;
            self.filter = v.filter;
            self.sort = v.sort;
            self.selection = v.selection;
            self.selection.ids.retain(|id| self.catalog.photo(*id).is_some());
            self.selection.active = self.selection.active.filter(|id| self.catalog.photo(*id).is_some());
        }
        if self.selection.active.is_none()
            && let Some(first) = self.visible_cloned().first()
        {
            self.selection = Selection::single(*first);
        }
        let presets_written = presets_json(self);
        // photo ids are per library: drop decoded sources of the previous one
        self.media.clear_sources();
        if on_disk {
            self.media.attach_disk_cache(&dir.join("thumbs"), self.cache_bytes());
        }
        if let Some(svc) = &self.media.segmenter {
            svc.set_disk(on_disk.then(|| dir.join("masks")));
        }
        let view_written = self.view_json();
        self.library = Some(Library { dir, on_disk, journal, files, report, last_error: None, presets_written, view_written });
        Ok(&self.library.as_ref().expect("just set").report)
    }

    /// Write pending ops to the log (fsynced), compact when due, and save changed presets.
    /// Called after every top-level command; cheap when nothing changed.
    pub fn persist(&mut self) -> Result<()> {
        let Some(lib) = self.library.as_mut() else { return Ok(()) };
        if !self.pending_log.is_empty() {
            if let Err(e) = lib.journal.append(&self.pending_log) {
                lib.last_error = Some(e.to_string());
                log::error!("library: {e}");
                return Err(e.into());
            }
            self.pending_log.clear();
            lib.last_error = None;
        }
        // Never snapshot mid-interaction: the catalog then holds an uncommitted preview value.
        if lib.journal.wants_snapshot() && self.interaction.is_none() {
            lib.journal.snapshot(&self.catalog)?;
        }
        let presets = presets_json(self);
        let Some(lib) = self.library.as_mut() else { return Ok(()) };
        if presets != lib.presets_written {
            if let Err(e) = lib.files.write_atomic("presets.json", presets.as_bytes()) {
                log::error!("library: presets: {e}");
            } else {
                lib.presets_written = presets;
            }
        }
        Ok(())
    }

    /// Persist if commands left ops pending (cheap; frontends call it once per frame for state
    /// changed outside [`Session::execute`]).
    pub fn persist_if_dirty(&mut self) {
        if self.library.is_some() && !self.pending_log.is_empty() {
            let _ = self.persist();
        }
    }

    /// Flush everything and write a snapshot (on quit). Ends an open interaction first.
    pub fn close_library(&mut self) -> Result<()> {
        if self.library.is_none() {
            return Ok(());
        }
        let _ = self.end_interaction();
        self.persist()?;
        self.save_view();
        let Some(lib) = self.library.as_mut() else { return Ok(()) };
        lib.journal.snapshot(&self.catalog)?;
        Ok(())
    }

    fn view_json(&self) -> Vec<u8> {
        let view = ViewFile {
            source: self.source,
            browse: self.browse.clone(),
            filter: self.filter.clone(),
            sort: self.sort,
            selection: self.selection.clone(),
        };
        serde_json::to_vec_pretty(&view).unwrap_or_default()
    }

    /// Save the view state (source, filter, sort, selection) if it changed since it was last
    /// written. Native hosts get this from [`Session::close_library`]; the browser host calls it
    /// periodically, since a tab can be closed without notice.
    pub fn save_view(&mut self) {
        if self.library.is_none() {
            return;
        }
        let v = self.view_json();
        let Some(lib) = self.library.as_mut() else { return };
        if v != lib.view_written {
            match lib.files.write_atomic("view.json", &v) {
                Ok(()) => lib.view_written = v,
                Err(e) => log::error!("library: view: {e}"),
            }
        }
    }

    /// Save the library preferences (no-op for in-memory sessions).
    pub fn save_prefs(&mut self) -> Result<()> {
        let v = serde_json::to_vec_pretty(&PrefsFile {
            xmp: self.xmp,
            last_export: self.last_export.clone(),
            export_presets: self.export_presets.clone(),
            metadata_presets: self.metadata_presets.clone(),
            filter_presets: self.filter_presets.clone(),
            import: self.import_defaults.clone(),
            cache_mb: self.cache_mb,
        })
        .unwrap_or_default();
        let Some(lib) = self.library.as_mut() else { return Ok(()) };
        lib.files.write_atomic("prefs.json", &v).map_err(|e| EngineError::Other(format!("prefs: {e}")))
    }

    /// The thumbnail disk cache budget in bytes ([`Session::cache_mb`], else the default).
    pub fn cache_bytes(&self) -> u64 {
        if self.cache_mb == 0 { crate::media::DISK_CACHE_BYTES } else { u64::from(self.cache_mb) << 20 }
    }

    /// Change the thumbnail disk cache budget (MB, 0 = default): re-attaches the cache, which
    /// trims it to the new size. Saved with the library preferences.
    pub fn set_cache_mb(&mut self, mb: u32) -> Result<()> {
        self.cache_mb = mb;
        if let Some(lib) = self.library.as_ref().filter(|l| l.on_disk) {
            let dir = lib.thumbs_dir();
            self.media.attach_disk_cache(&dir, self.cache_bytes());
        }
        self.save_prefs()
    }

    /// Compact the log into a snapshot now.
    pub fn compact_library(&mut self) -> Result<()> {
        self.persist()?;
        if self.interaction.is_some() {
            return Err(EngineError::Other("can't compact during an interaction".into()));
        }
        if let Some(lib) = self.library.as_mut() {
            lib.journal.snapshot(&self.catalog)?;
        }
        Ok(())
    }
}
