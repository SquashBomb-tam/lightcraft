//! AI segmentation for the Sky, Subject, Background and Object masks.
//!
//! Models are permissively licensed weight files shipped beside the application (see
//! `models/README.md` and `assets/ATTRIBUTION.md`) and run with pure-Rust inference
//! ([candle](https://github.com/huggingface/candle), CPU). Without them — the browser build, or a
//! development checkout that hasn't run `cargo xtask models --download` — [`Segmenter::has`] says
//! so and the masks fall back to the pipeline's classical estimates.
//!
//! Every model takes a display-referred 8-bit RGB image of the *oriented source* (the photo after
//! the user's Rotate/Flip, before lens corrections and crop) and returns a probability map over
//! that same image, so the pipeline can resample it through any later geometry
//! (`lightcraft_pipeline::segmaps`).
#![forbid(unsafe_code)]

mod prep;

#[cfg(not(target_arch = "wasm32"))]
mod native;

use std::path::{Path, PathBuf};

use lightcraft_raster::{Plane, Rgba8};

pub use prep::{fit_long_edge, letterbox};

/// U²-Net's convolutions in execution order (`cargo xtask models` names and checks the Sky and
/// Subject weights against them).
#[cfg(not(target_arch = "wasm32"))]
pub use native::u2net::{Layer as U2NetLayer, layers as u2net_layers};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the {0} model is not installed (expected {1})")]
    Missing(&'static str, String),
    #[error("{0}: {1}")]
    Model(&'static str, String),
    #[error("AI masks are not available in this build")]
    Unsupported,
}

pub type Result<T> = std::result::Result<T, Error>;

/// What a segmentation is of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Sky,
    /// The main subject (salient foreground); Background is its complement.
    Subject,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Sky => "sky",
            Kind::Subject => "subject",
        }
    }
}

/// An Object mask's prompt in normalized oriented-source coordinates (0..1): points on the object
/// (`true`) or off it (`false`), and/or a box around it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prompt {
    pub points: Vec<(f64, f64, bool)>,
    pub bbox: Option<[f64; 4]>,
}

impl Prompt {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty() && self.bbox.is_none()
    }
}

/// The image features an Object prompt is decoded against (computed once per photo).
#[derive(Clone)]
pub struct Embedding {
    #[cfg(not(target_arch = "wasm32"))]
    inner: native::Embedding,
}

impl std::fmt::Debug for Embedding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Embedding")
    }
}

/// The installed models, loaded on first use. Shareable across threads; inference calls on one
/// model run one at a time.
pub struct Segmenter {
    dir: PathBuf,
    #[cfg(not(target_arch = "wasm32"))]
    native: native::Models,
}

impl std::fmt::Debug for Segmenter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Segmenter").field("dir", &self.dir).finish()
    }
}

impl Segmenter {
    /// The models in `dir` (files are only opened when first used).
    pub fn new(dir: impl Into<PathBuf>) -> Segmenter {
        let dir = dir.into();
        Segmenter {
            #[cfg(not(target_arch = "wasm32"))]
            native: native::Models::new(&dir),
            dir,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether the model for `kind` is installed.
    pub fn has(&self, kind: Kind) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.native.has(kind);
        #[cfg(target_arch = "wasm32")]
        {
            let _ = kind;
            false
        }
    }

    /// Whether the Object model is installed.
    pub fn has_objects(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.native.has_objects();
        #[cfg(target_arch = "wasm32")]
        false
    }

    /// Identifies the installed models' versions (file names and sizes), for cache keys: a
    /// segmentation computed by other weights is never reused.
    pub fn revision(&self) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        return self.native.revision();
        #[cfg(target_arch = "wasm32")]
        0
    }

    /// The probability (0..1) of each pixel of `img` belonging to `kind`, at the model's working
    /// resolution with `img`'s aspect ratio.
    pub fn segment(&self, kind: Kind, img: &Rgba8) -> Result<Plane> {
        check(img)?;
        #[cfg(not(target_arch = "wasm32"))]
        return self.native.segment(kind, img);
        #[cfg(target_arch = "wasm32")]
        {
            let _ = kind;
            Err(Error::Unsupported)
        }
    }

    /// The features Object prompts on `img` are decoded against.
    pub fn embed(&self, img: &Rgba8) -> Result<Embedding> {
        check(img)?;
        #[cfg(not(target_arch = "wasm32"))]
        return self.native.embed(img).map(|inner| Embedding { inner });
        #[cfg(target_arch = "wasm32")]
        Err(Error::Unsupported)
    }

    /// The object `prompt` selects in the image `emb` was computed from: a probability map with
    /// that image's aspect ratio.
    pub fn object(&self, emb: &Embedding, prompt: &Prompt) -> Result<Plane> {
        if prompt.is_empty() {
            return Err(Error::Model("object", "an empty prompt (no points, no box)".into()));
        }
        #[cfg(not(target_arch = "wasm32"))]
        return self.native.object(&emb.inner, prompt);
        #[cfg(target_arch = "wasm32")]
        {
            let _ = emb;
            Err(Error::Unsupported)
        }
    }
}

fn check(img: &Rgba8) -> Result<()> {
    if img.width < 8 || img.height < 8 || img.data.len() != img.width * img.height {
        return Err(Error::Model("input", format!("an unusable {}×{} image", img.width, img.height)));
    }
    Ok(())
}

/// Where an installed application keeps its models, relative to its executable: beside it
/// (`models/`, Windows and portable builds), in a macOS bundle's `Resources/models/`, or in
/// `share/lightcraft/models/` (Linux packages). `LIGHTCRAFT_MODELS` overrides; a development
/// checkout's `models/` (filled by `cargo xtask models --download`) is the last resort.
pub fn find_models_dir(exe: Option<&Path>) -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("LIGHTCRAFT_MODELS").map(PathBuf::from) {
        return Some(d);
    }
    let mut candidates = Vec::new();
    if let Some(dir) = exe.and_then(Path::parent) {
        candidates.push(dir.join("models"));
        candidates.push(dir.join("../Resources/models"));
        candidates.push(dir.join("../share/lightcraft/models"));
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models"));
    candidates.into_iter().find(|d| d.join("README.md").is_file() || std::fs::read_dir(d).is_ok_and(|mut r| r.next().is_some()))
}
