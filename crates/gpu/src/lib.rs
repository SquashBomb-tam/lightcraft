//! The LightCraft develop pipeline on the GPU (wgpu compute, WGSL kernels).
//!
//! The CPU pipeline (`lightcraft-pipeline`) is the reference: every kernel here is a port of a CPU
//! stage, both read the same resolved parameters ([`lightcraft_pipeline::Plan`],
//! [`lightcraft_pipeline::finish::FinishParams`]), and the equivalence tests (`tests/`) render the
//! same settings on both and bound the difference in 8-bit sRGB. Stages without a kernel run on the
//! CPU inside the same render (per-stage hybrid); see `docs/gpu-pipeline.md`.
//!
//! Use [`render`]: it returns `None` when the GPU is unavailable, disabled (`LIGHTCRAFT_GPU=0` or
//! [`set_enabled`]), or the render does not fit the device — callers then render on the CPU.
//! The browser build has no GPU path yet (WebGPU device creation is asynchronous): everything here
//! compiles to the CPU fallback on wasm32.
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_develop::DevelopSettings;
use lightcraft_pipeline::{RenderRequest, Rendered, SourceInfo, StageCache};
use lightcraft_raster::Rgb32f;

#[cfg(not(target_arch = "wasm32"))]
mod ctx;
#[cfg(not(target_arch = "wasm32"))]
mod params;
#[cfg(not(target_arch = "wasm32"))]
mod render;

#[cfg(not(target_arch = "wasm32"))]
pub use render::GpuStages;

static ENABLED: AtomicBool = AtomicBool::new(true);
/// Set when a GPU render failed: the process stays on the CPU from then on.
static BROKEN: AtomicBool = AtomicBool::new(false);

/// Stop using the GPU for the rest of the process (after a device error).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn mark_broken() {
    BROKEN.store(true, Ordering::Relaxed);
}

/// Allow or forbid GPU rendering at runtime (a preference). `LIGHTCRAFT_GPU=0` forbids it for the
/// whole process regardless.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether GPU rendering is allowed (environment, preference, no earlier failure).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed) && !BROKEN.load(Ordering::Relaxed) && !env_disabled()
}

fn env_disabled() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var("LIGHTCRAFT_GPU").is_ok_and(|v| matches!(v.trim(), "0" | "off" | "false" | "no")))
}

#[cfg(not(target_arch = "wasm32"))]
static GPU: std::sync::OnceLock<Option<ctx::Gpu>> = std::sync::OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn device() -> Option<&'static ctx::Gpu> {
    if env_disabled() {
        return None;
    }
    GPU.get_or_init(|| std::panic::catch_unwind(ctx::Gpu::new).ok().flatten()).as_ref()
}

/// The device if it has been created (never creates it).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn existing_device() -> Option<&'static ctx::Gpu> {
    GPU.get().and_then(|g| g.as_ref())
}

/// Create the device and compile the kernels on a background thread now (app start), so the
/// first render doesn't wait ~0.3–0.4 s for it and the UI thread never does.
pub fn warm_up() {
    #[cfg(not(target_arch = "wasm32"))]
    if !env_disabled() && GPU.get().is_none() {
        let _ = std::thread::Builder::new().name("lc-gpu-init".into()).spawn(|| {
            let _ = device();
        });
    }
}

/// Has device creation finished (successfully or not)? Never blocks — for status displays.
pub fn ready() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        env_disabled() || GPU.get().is_some()
    }
    #[cfg(target_arch = "wasm32")]
    {
        true
    }
}

/// Whether a usable GPU adapter exists and GPU rendering is enabled (creates the device on first
/// use).
pub fn available() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        enabled() && device().is_some()
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// The adapter's name and backend, e.g. "Apple M4 Pro (Metal)".
pub fn adapter_name() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        device().map(|g| format!("{} ({:?})", g.info.name, g.info.backend))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// Device buffers held by the renderer.
#[cfg(not(target_arch = "wasm32"))]
pub use ctx::GpuMemory;

/// Device buffers held by the renderer.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuMemory {
    pub allocated: u64,
    pub pooled: u64,
    pub retired: u64,
}

/// Device memory held by the renderer's buffers (zero without a device).
pub fn memory() -> GpuMemory {
    #[cfg(not(target_arch = "wasm32"))]
    {
        ctx::memory()
    }
    #[cfg(target_arch = "wasm32")]
    {
        GpuMemory::default()
    }
}

/// Keep at most `bytes` of recycled buffers in the free pool from now on (trimming it now).
pub fn set_pool_limit(bytes: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        ctx::POOL_LIMIT.store(bytes, Ordering::Relaxed);
        trim_pool(bytes);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = bytes;
}

/// Free recycled buffers until at most `keep` bytes stay pooled (e.g. when the app goes idle).
pub fn trim_pool(keep: u64) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(g) = GPU.get().and_then(|g| g.as_ref()) {
        g.trim(keep);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = keep;
}

/// Device bytes held by a view's GPU stages (kept with its [`StageCache`]); 0 when it has none.
pub fn stage_bytes(stages: &StageCache) -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        stages.peek_extension::<GpuStages>().map(|g| g.bytes()).unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = stages;
        0
    }
}

/// Render `src` with `s` on the GPU, reusing the device-resident stages kept with `stages` (the
/// view's CPU stage cache). `None`: render on the CPU instead.
pub fn render(src: &Arc<Rgb32f>, info: &SourceInfo, s: &DevelopSettings, req: &RenderRequest, stages: Option<&StageCache>) -> Option<Rendered> {
    render_with(src, info, s, req, stages, &lightcraft_pipeline::Segmentations::NONE)
}

/// [`render`] with the photo's AI segmentations for its Sky / Subject / Background / Object masks.
pub fn render_with(
    src: &Arc<Rgb32f>,
    info: &SourceInfo,
    s: &DevelopSettings,
    req: &RenderRequest,
    stages: Option<&StageCache>,
    seg: &lightcraft_pipeline::Segmentations,
) -> Option<Rendered> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        // the kernel writes 8-bit output: high-bit-depth exports render on the CPU
        if !enabled() || req.depth != lightcraft_pipeline::OutputDepth::U8 {
            return None;
        }
        let gpu = device()?;
        let ext = stages.map(|c| c.extension::<GpuStages>());
        let r = {
            let _scope = ctx::RenderScope::new(gpu);
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render::render(gpu, src, info, s, req, ext.as_deref(), seg)))
        };
        match r {
            // a device error during the render: its result is not trustworthy
            Ok(_) if BROKEN.load(Ordering::Relaxed) => None,
            Ok(r) => r,
            Err(_) => {
                log::error!("gpu: render failed; using the CPU pipeline from now on");
                BROKEN.store(true, Ordering::Relaxed);
                None
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (src, info, s, req, stages, seg);
        None
    }
}
