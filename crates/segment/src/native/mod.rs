//! Native inference (candle, CPU): loads each installed model on first use.

mod kernels;
mod sam;
mod tiny_vit;
pub mod u2net;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use lightcraft_raster::{Plane, Rgba8};

use crate::{Error, Kind, Prompt, Result};

pub use sam::Embedding;

/// The model files `cargo xtask models` writes (see `models/README.md`).
pub const SKY_FILE: &str = "sky.safetensors";
pub const SUBJECT_FILE: &str = "subject.safetensors";
pub const OBJECT_FILE: &str = "object.safetensors";

type Slot<T> = OnceLock<std::result::Result<T, String>>;

pub struct Models {
    dir: PathBuf,
    sky: Slot<u2net::U2Net>,
    subject: Slot<u2net::U2Net>,
    sam: Slot<sam::Sam>,
}

impl Models {
    pub fn new(dir: &Path) -> Models {
        Models { dir: dir.to_path_buf(), sky: OnceLock::new(), subject: OnceLock::new(), sam: OnceLock::new() }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn kind_file(kind: Kind) -> &'static str {
        match kind {
            Kind::Sky => SKY_FILE,
            Kind::Subject => SUBJECT_FILE,
        }
    }

    pub fn has(&self, kind: Kind) -> bool {
        self.file(Self::kind_file(kind)).is_file()
    }

    pub fn has_objects(&self) -> bool {
        self.file(OBJECT_FILE).is_file()
    }

    pub fn revision(&self) -> u64 {
        // file names and sizes: a different download is a different model
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for name in [SKY_FILE, SUBJECT_FILE, OBJECT_FILE] {
            let size = std::fs::metadata(self.file(name)).map(|m| m.len()).unwrap_or(0);
            for b in name.bytes().chain(size.to_le_bytes()) {
                h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
            }
        }
        h
    }

    /// The weights in `name` (safetensors), or why they can't be read.
    fn weights(&self, name: &str, what: &'static str) -> Result<VarBuilder<'static>> {
        let path = self.file(name);
        if !path.is_file() {
            return Err(Error::Missing(what, path.display().to_string()));
        }
        let tensors: HashMap<String, Tensor> =
            candle_core::safetensors::load(&path, &Device::Cpu).map_err(|e| Error::Model(what, format!("{}: {e}", path.display())))?;
        Ok(VarBuilder::from_tensors(tensors, DType::F32, &Device::Cpu))
    }

    fn u2net(&self, kind: Kind) -> Result<&u2net::U2Net> {
        let (slot, scale) = match kind {
            Kind::Sky => (&self.sky, u2net::Scale::Fixed),
            Kind::Subject => (&self.subject, u2net::Scale::Max),
        };
        let name = Self::kind_file(kind);
        if !self.file(name).is_file() {
            return Err(Error::Missing(kind.name(), self.file(name).display().to_string()));
        }
        slot.get_or_init(|| {
            let vb = self.weights(name, kind.name()).map_err(|e| e.to_string())?;
            u2net::U2Net::load(vb, scale).map_err(|e| format!("{name}: {e}"))
        })
        .as_ref()
        .map_err(|e| Error::Model(kind.name(), e.clone()))
    }

    pub fn segment(&self, kind: Kind, img: &Rgba8) -> Result<Plane> {
        self.u2net(kind)?.segment(img).map_err(|e| Error::Model(kind.name(), e.to_string()))
    }

    fn sam(&self) -> Result<&sam::Sam> {
        if !self.has_objects() {
            return Err(Error::Missing("object", self.file(OBJECT_FILE).display().to_string()));
        }
        self.sam
            .get_or_init(|| {
                let vb = self.weights(OBJECT_FILE, "object").map_err(|e| e.to_string())?;
                sam::Sam::load(vb).map_err(|e| format!("{OBJECT_FILE}: {e}"))
            })
            .as_ref()
            .map_err(|e| Error::Model("object", e.clone()))
    }

    pub fn embed(&self, img: &Rgba8) -> Result<Embedding> {
        self.sam()?.embed(img).map_err(|e| Error::Model("object", e.to_string()))
    }

    pub fn object(&self, emb: &Embedding, prompt: &Prompt) -> Result<Plane> {
        self.sam()?.object(emb, prompt).map_err(|e| Error::Model("object", e.to_string()))
    }
}
