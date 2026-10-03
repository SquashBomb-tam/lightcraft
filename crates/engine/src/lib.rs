//! The LightCraft engine façade.
//!
//! Every user-visible action is a command with a stable id (`photo.rate`, `develop.set`,
//! `album.create`, `mask.add`…) and JSON parameters. The egui UI, the CLI, the control channel and
//! the MCP server all go through [`Session::execute`].
//!
//! State: a [`Catalog`] (mutated only by ops, so every change is undoable and journaled), the
//! library view (filter/sort/source), the selection, the develop clipboard, presets, and caches
//! of decoded source proxies. Rendering is done by [`RenderJob`]s that are `Send` so frontends can
//! run them off the UI thread.
#![forbid(unsafe_code)]

pub mod cmd;
pub mod crs;
pub mod demo;
pub mod devices;
pub mod export;
pub mod files;
pub mod import;
pub mod library;
pub mod media;
pub mod memory;
pub mod merge;
pub mod preset_import;
pub mod presets;
pub mod rename;
pub mod segment;
pub mod sidecar;
mod view;

use std::sync::Arc;

pub use cmd::{CommandInfo, CommandSpec, command_specs, find_command};
use lightcraft_catalog::{Catalog, Filter, Op, PhotoId, Sort};
use lightcraft_develop::DevelopSettings;
pub use media::{RenderJob, SourceLevel};
use serde_json::Value;
pub use view::{Browse, LibrarySource, Selection};
pub use {lightcraft_catalog as catalog, lightcraft_develop as develop, lightcraft_gpu as gpu, lightcraft_pipeline as pipeline};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("command `{0}` is not available right now: {1}")]
    Disabled(String, String),
    #[error("invalid parameters for `{cmd}`: {msg}")]
    BadParams { cmd: String, msg: String },
    #[error("{0}")]
    Catalog(#[from] lightcraft_catalog::CatalogError),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// One undo step: the inverse op and a label.
#[derive(Clone, Debug)]
pub struct UndoEntry {
    pub label: String,
    pub op: Op,
}

/// An in-progress slider drag / brush stroke: one undo step when it ends.
#[derive(Clone, Debug)]
pub struct Interaction {
    pub label: String,
    pub photo: PhotoId,
    pub original: Arc<DevelopSettings>,
}

pub struct Session {
    pub catalog: Catalog,
    pub source: LibrarySource,
    pub filter: Filter,
    pub sort: Sort,
    pub selection: Selection,
    /// The grid order for the current source/filter/sort (cached by catalog revision).
    visible: Vec<PhotoId>,
    visible_key: Option<(u64, String)>,
    pub undo: Vec<UndoEntry>,
    pub redo: Vec<UndoEntry>,
    pub interaction: Option<Interaction>,
    /// Copied develop settings (partial JSON) for Paste.
    pub clipboard: Option<Value>,
    /// The folder on disk the [`LibrarySource::Folder`] view browses.
    pub browse: Option<Browse>,
    /// Copied metadata (`photo.copyMetadata`): photo.setMeta params.
    pub meta_clipboard: Option<Value>,
    /// The photo that was active before the current one (Paste Settings from Previous).
    pub previous_active: Option<PhotoId>,
    /// Groups last used for Copy (Lightroom remembers them).
    pub copy_groups: Vec<lightcraft_develop::SettingsGroup>,
    pub presets: Vec<lightcraft_develop::Preset>,
    /// Favourite profile ids (persisted with the library, like preset favourites).
    pub profile_favorites: Vec<String>,
    /// Recently applied profile ids, newest first (at most [`presets::RECENT_PROFILES`]).
    pub profile_recent: Vec<String>,
    /// Executed commands (actions / debugging / replay).
    pub journal: Vec<(String, Value)>,
    /// Ops applied since the last `drain_log` (for persistence).
    pending_log: Vec<Op>,
    pub media: media::MediaCache,
    /// Current time provider (ISO 8601); injectable for tests and wasm.
    pub clock: Box<dyn Fn() -> String + Send>,
    depth: u32,
    /// Selected mask (Masking panel), by mask id.
    pub active_mask: Option<u32>,
    /// Selected spot (Remove panel), by index into the active photo's spots.
    pub active_spot: Option<usize>,
    /// The persistent library this session writes to (`None` = in-memory only).
    pub library: Option<library::Library>,
    /// XMP sidecar preferences (persisted with the library).
    pub xmp: sidecar::XmpPrefs,
    /// Parameters of the last export (`app.export` params, minus targets), persisted in prefs.json.
    pub last_export: Option<serde_json::Value>,
    /// The user's export presets (built-ins: [`export::builtin_presets`]), persisted in prefs.json.
    pub export_presets: Vec<export::ExportPreset>,
    /// Metadata presets (`metadata.*`), persisted in prefs.json.
    pub metadata_presets: Vec<cmd::metadata::MetadataPreset>,
    /// Saved filter-bar settings (`filter.*`), persisted in prefs.json.
    pub filter_presets: Vec<cmd::filters::FilterPreset>,
    /// Before/After: the "before" settings chosen per photo (this session; default: the photo's
    /// import state). See `cmd/before.rs`.
    pub before: std::collections::HashMap<PhotoId, Arc<DevelopSettings>>,
    /// File probes from the last import review (`library.importPreview`), reused by the import.
    pub import_probes: std::collections::HashMap<String, media::ProbeInfo>,
    /// Develop defaults applied on import (persisted in prefs.json).
    pub import_defaults: import::ImportDefaults,
    /// Disk budget of the library's thumbnail cache in MB (0 = default; persisted in prefs.json).
    pub cache_mb: u32,
}

impl Default for Session {
    fn default() -> Self {
        Session::new()
    }
}

impl Session {
    pub fn new() -> Session {
        Session {
            catalog: Catalog::new(),
            source: LibrarySource::All,
            filter: Filter::default(),
            sort: Sort::default(),
            selection: Selection::default(),
            visible: Vec::new(),
            visible_key: None,
            undo: Vec::new(),
            redo: Vec::new(),
            interaction: None,
            clipboard: None,
            meta_clipboard: None,
            browse: None,
            previous_active: None,
            copy_groups: lightcraft_develop::SettingsGroup::default_copy(),
            presets: presets::builtin(),
            profile_favorites: Vec::new(),
            profile_recent: Vec::new(),
            journal: Vec::new(),
            pending_log: Vec::new(),
            media: media::MediaCache::default(),
            clock: Box::new(|| "2026-09-30T12:00:00".to_string()),
            depth: 0,
            active_mask: None,
            active_spot: None,
            library: None,
            xmp: sidecar::XmpPrefs::default(),
            last_export: None,
            export_presets: Vec::new(),
            metadata_presets: Vec::new(),
            filter_presets: Vec::new(),
            before: Default::default(),
            import_probes: Default::default(),
            import_defaults: import::ImportDefaults::default(),
            cache_mb: 0,
        }
    }

    /// A session with the procedurally generated demo library loaded.
    pub fn with_demo() -> Session {
        let mut s = Session::new();
        demo::load(&mut s);
        s
    }

    /// Run a command by id. THE entry point for every frontend.
    pub fn execute(&mut self, id: &str, params: &Value) -> Result<Value> {
        let spec = find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        (spec.enabled)(self).map_err(|why| EngineError::Disabled(id.to_string(), why))?;
        let empty = Value::Object(Default::default());
        let params = if params.is_null() { &empty } else { params };
        let log_start = self.pending_log.len();
        let was_active = self.active();
        self.depth += 1;
        let r = (spec.run)(self, params);
        self.depth -= 1;
        if self.depth == 0 && was_active.is_some() && self.active() != was_active {
            self.previous_active = was_active;
        }
        if r.is_ok() && spec.journal && self.depth == 0 {
            self.journal.push((id.to_string(), params.clone()));
            if self.journal.len() > 10_000 {
                self.journal.drain(..1000);
            }
        }
        if r.is_ok() && self.depth == 0 && self.xmp.auto_write && self.interaction.is_none() && self.pending_log.len() > log_start {
            self.auto_write_sidecars(&self.pending_log[log_start..]);
        }
        if self.depth == 0 && self.library.is_some() {
            // Make the command durable before reporting success (on failure the ops stay pending,
            // are retried after the next command, and `library.info` reports the error).
            let _ = self.persist();
        }
        r
    }

    pub fn commands(&self) -> Vec<CommandInfo> {
        command_specs().iter().map(|c| c.info(self)).collect()
    }

    // ---------------------------------------------------------------- ops, undo

    /// Apply an op as one undoable step.
    pub fn commit(&mut self, label: &str, op: Op) -> Result<()> {
        let fwd = op.clone();
        let inv = self.catalog.apply(op)?;
        self.pending_log.push(fwd);
        self.undo.push(UndoEntry { label: label.to_string(), op: inv });
        if self.undo.len() > 1000 {
            self.undo.remove(0);
        }
        self.redo.clear();
        Ok(())
    }

    /// Fold the last `n` undo steps into one (commands that commit step by step because each op
    /// depends on the state the previous one left).
    pub fn merge_undo(&mut self, n: usize, label: &str) {
        if n < 2 || n > self.undo.len() {
            return;
        }
        let tail = self.undo.split_off(self.undo.len() - n);
        let ops = tail.into_iter().rev().map(|e| e.op).collect();
        self.undo.push(UndoEntry { label: label.to_string(), op: Op::Batch { ops } });
    }

    /// Apply without recording undo (interactive previews).
    fn apply_silent(&mut self, op: Op) -> Result<()> {
        self.catalog.apply(op)?;
        Ok(())
    }

    pub fn undo_step(&mut self) -> Result<String> {
        let e = self.undo.pop().ok_or_else(|| EngineError::Other("nothing to undo".into()))?;
        let redo = match self.apply_with_files(&e.op) {
            Ok(r) => r,
            Err(err) => {
                self.undo.push(e);
                return Err(err);
            }
        };
        self.pending_log.push(e.op);
        self.redo.push(UndoEntry { label: e.label.clone(), op: redo });
        Ok(e.label)
    }

    pub fn redo_step(&mut self) -> Result<String> {
        let e = self.redo.pop().ok_or_else(|| EngineError::Other("nothing to redo".into()))?;
        let undo = match self.apply_with_files(&e.op) {
            Ok(r) => r,
            Err(err) => {
                self.redo.push(e);
                return Err(err);
            }
        };
        self.pending_log.push(e.op);
        self.undo.push(UndoEntry { label: e.label.clone(), op: undo });
        Ok(e.label)
    }

    /// Apply an undo/redo op, first moving the files its renames imply (all or nothing).
    fn apply_with_files(&mut self, op: &Op) -> Result<Op> {
        let moves = self.file_moves(op);
        Session::move_files(&moves)?;
        match self.catalog.apply(op.clone()) {
            Ok(inv) => Ok(inv),
            Err(e) => {
                let back: Vec<(String, String)> = moves.iter().rev().map(|(a, b)| (b.clone(), a.clone())).collect();
                let _ = Session::move_files(&back);
                Err(e.into())
            }
        }
    }

    /// Ops applied since the last call, for the op-log store.
    pub fn drain_log(&mut self) -> Vec<Op> {
        std::mem::take(&mut self.pending_log)
    }

    // ---------------------------------------------------------------- develop edits

    /// The photo being edited (the active photo).
    pub fn active(&self) -> Option<PhotoId> {
        self.selection.active.filter(|id| self.catalog.photo(*id).is_some())
    }

    pub fn develop_of(&self, id: PhotoId) -> Option<Arc<DevelopSettings>> {
        self.catalog.photo(id).map(|p| p.develop.clone())
    }

    /// Change a photo's develop settings. During an interaction the change is previewed without an
    /// undo step; otherwise it is committed with a history entry.
    pub fn set_develop(&mut self, id: PhotoId, settings: DevelopSettings, label: &str) -> Result<()> {
        let now = (self.clock)();
        let settings = Arc::new(settings);
        if let Some(i) = &self.interaction
            && i.photo == id
        {
            return self.apply_silent(Op::SetDevelop { id, settings, label: label.into(), edited: Some(now) });
        }
        let op = self.develop_op(id, (*settings).clone(), label).ok_or(lightcraft_catalog::CatalogError::NoPhoto(id))?;
        self.commit(label, op)
    }

    /// The op that sets a photo's develop settings and appends a History entry (for batches).
    pub fn develop_op(&self, id: PhotoId, settings: DevelopSettings, label: &str) -> Option<Op> {
        self.catalog.photo(id)?;
        let settings = Arc::new(settings);
        let step = lightcraft_catalog::HistoryStep { label: label.into(), settings: settings.clone() };
        Some(Op::Batch {
            ops: vec![Op::SetDevelop { id, settings, label: label.into(), edited: Some((self.clock)()) }, Op::PushHistory { id, step }],
        })
    }

    pub fn begin_interaction(&mut self, label: &str) -> Result<()> {
        if self.interaction.is_some() {
            self.end_interaction()?;
        }
        let id = self.active().ok_or_else(|| EngineError::Other("no active photo".into()))?;
        let original = self.develop_of(id).unwrap_or_default();
        self.interaction = Some(Interaction { label: label.into(), photo: id, original });
        Ok(())
    }

    /// Commit the interaction as one undo step (no-op if nothing changed).
    pub fn end_interaction(&mut self) -> Result<()> {
        let Some(i) = self.interaction.take() else { return Ok(()) };
        let Some(cur) = self.develop_of(i.photo) else { return Ok(()) };
        if *cur == *i.original {
            return Ok(());
        }
        // Restore the original silently, then commit the final value as one step.
        self.apply_silent(Op::SetDevelop { id: i.photo, settings: i.original.clone(), label: i.label.clone(), edited: None })?;
        self.set_develop(i.photo, (*cur).clone(), &i.label)
    }

    pub fn cancel_interaction(&mut self) -> Result<()> {
        if let Some(i) = self.interaction.take() {
            self.apply_silent(Op::SetDevelop { id: i.photo, settings: i.original, label: i.label, edited: None })?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------- library view

    /// Photos shown in the grid/filmstrip for the current source, filter and sort.
    pub fn visible(&mut self) -> &[PhotoId] {
        let key = (self.catalog.revision, format!("{:?}|{:?}|{:?}|{:?}", self.source, self.filter, self.sort, self.browse));
        if self.visible_key.as_ref() != Some(&key) {
            let mut f = self.source.to_filter(&self.filter, &self.catalog);
            if self.source == LibrarySource::Folder {
                // no folder chosen: nothing (an empty path matches nothing)
                let b = self.browse.clone().unwrap_or_default();
                f.folder = Some(b.path);
                f.subfolders = b.subfolders;
            }
            self.visible = self.catalog.query(&f, &self.sort);
            if matches!(self.source, LibrarySource::Album(_))
                && self.sort.key == lightcraft_catalog::SortKey::CaptureDate
                && let LibrarySource::Album(a) = self.source
                && let Some(al) = self.catalog.album(a)
                && !al.is_smart()
                && self.filter == Filter::default()
            {
                let order = al.photos.clone();
                self.visible.sort_by_key(|id| order.iter().position(|x| x == id).unwrap_or(usize::MAX));
                if !self.sort.ascending {
                    self.visible.reverse();
                }
            }
            if self.source == LibrarySource::RecentlyAdded {
                // newest import first, whatever the sort (the grid groups by import day)
                let cat = &self.catalog;
                self.visible.sort_by(|a, b| {
                    let key = |id: &PhotoId| cat.photo(*id).map(|p| p.imported.clone()).unwrap_or_default();
                    key(b).cmp(&key(a)).then(a.cmp(b))
                });
            }
            if self.source == LibrarySource::Missing {
                let missing: std::collections::HashSet<PhotoId> = cmd::missing::missing(self).into_iter().map(|m| m.0).collect();
                self.visible.retain(|id| missing.contains(id));
            }
            if self.source != LibrarySource::RecentlyDeleted {
                self.visible = self.catalog.arrange_stacks(&self.visible);
            }
            self.visible_key = Some(key);
        }
        &self.visible
    }

    pub fn visible_cloned(&mut self) -> Vec<PhotoId> {
        self.visible().to_vec()
    }

    /// Targets of photo commands: explicit `ids`/`id` param, else the selection.
    pub fn targets(&self, p: &Value) -> Vec<PhotoId> {
        if let Some(a) = p.get("ids").and_then(Value::as_array) {
            return a.iter().filter_map(Value::as_u64).map(PhotoId).collect();
        }
        if let Some(id) = p.get("id").and_then(Value::as_u64) {
            return vec![PhotoId(id)];
        }
        if self.selection.ids.is_empty() { self.selection.active.into_iter().collect() } else { self.selection.ids.clone() }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_color;
#[cfg(test)]
mod tests_export;
#[cfg(test)]
mod tests_import;
#[cfg(test)]
mod tests_libops;
#[cfg(test)]
mod tests_library;
#[cfg(test)]
mod tests_merge;
#[cfg(test)]
mod tests_organize;
#[cfg(test)]
mod tests_prefs;
#[cfg(test)]
mod tests_spots;
#[cfg(test)]
mod tests_xmp;
