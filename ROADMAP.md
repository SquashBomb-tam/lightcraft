# LightCraft roadmap

Milestones toward full Adobe Lightroom parity (cloud Lightroom first, then every Lightroom Classic module), with wall-clock
estimates for continuous (24/7) agent-driven development with 4–6 parallel agents. Estimates are calibrated on the sibling
projects (DrawCraft reached its first four milestones in ≈ 4½ h) and are revised as milestones land.

**Status legend:** ✅ done · 🚧 in progress · ⬜ not started

| # | Milestone | Scope (summary) | Estimate (h) | Status |
|---|---|---|---|---|
| M0 | Skeleton + visual shell | workspace, xtask CI + layering, geom/color/raster, develop model, pipeline v0, catalog v0, engine commands, Lightroom-look UI (grid, loupe, filmstrip, Edit panel), control channel, MCP, web build | 3–5 | ✅ |
| M1 | Library core | import (JPEG/PNG/TIFF/WebP), EXIF/XMP, persistent catalog (op log + snapshots), albums, ratings/flags/labels, filter/search/sort, thumbnail cache, 100k-photo grid | 6–10 | ✅ (100k-photo grid scale test pending) |
| M2 | Pipeline v1 (quality) | WB temp/tint, profiles, local tone mapping (highlights/shadows), curves, HSL, point colour, colour grading, texture/clarity/dehaze, vignette, grain, B&W, auto tone/WB, histogram, before/after | 10–15 | ✅ (look tuning vs our references ongoing) |
| M3 | RAW I | TIFF/DNG (LJ92, deflate, tiles, opcodes), demosaic (AHD/PPG/bilinear), highlight recovery, DNG colour model, CR2, NEF, ARW, embedded previews | 10–15 | 🚧 (DNG, CR2, ARW, NEF uncompressed, embedded previews ✅; NEF Huffman ⬜) |
| M4 | Crop, geometry, optics | crop tool + overlays, straighten, Upright (auto/level/vertical/full/guided), manual transforms, CA, defringe, manual lens corrections | 6–10 | ✅ |
| M5 | Performance | source pyramids, wgpu compute pipeline (CPU oracle), draft/full renders, prefetch, budgets (16 ms slider updates on 24 MP) | 10–15 | 🚧 (stage cache, source pyramid, wgpu pipeline, prefetch, memory budget ✅; colour NR at half resolution, GPU histogram ⬜) |
| M6 | Masking | brush, linear/radial gradients, colour/luminance/depth range, add/subtract/intersect/invert, all local adjustments, masks panel | 8–12 | 🚧 (brush/linear/radial/colour/luminance range, add/subtract/intersect, masks panel ✅; depth range, AI masks ⬜) |
| M7 | Detail | sharpening + masking preview, luminance/colour NR, Denoise, Raw Details, Super Resolution | 6–10 | 🚧 (sharpening, luminance/colour NR ✅; AI Denoise, Raw Details, Super Resolution ⬜) |
| M8 | Heal / Remove | content-aware remove (PatchMatch), heal, clone, brush spots, visualize spots, red/pet eye | 6–10 | 🚧 (heal, clone, auto source, visualize spots, red/pet eye ✅; PatchMatch remove ⬜) |
| M9 | Presets, profiles, versions, sync | preset browser + amount, create/import presets, profile browser, versions, history, copy/paste/sync settings | 5–8 | ✅ |
| M10 | Export & share | export dialog (JPEG/PNG/TIFF/DNG/AVIF/JXL/original), sizing, sharpening, metadata, watermark, naming, batch jobs, XMP sidecars, HDR export | 6–10 | 🚧 (all formats incl. DNG/original, sizing, presets, background jobs ✅; JXL encode, HDR export ⬜) |
| M11 | RAW II | CR3, RAF (X-Trans), ORF, RW2, PEF, SRW, 3FR, IIQ + long tail; camera calibration DB; HEIC/AVIF/JXL import | 20–35 | 🚧 (RAF uncompressed, RW2 packed, PEF, ORF uncompressed ✅; CR3, compressed NEF/ORF/RAF ⬜) |
| M12 | AI & smart features | subject/sky/background/people/object masks, semantic search, faces/People (permissively licensed models, pure-Rust inference) | 20–40 | ⬜ |
| M13 | Merge | HDR merge (deghost), panorama (projections, boundary warp, fill edges), HDR panorama | 10–15 | ✅ |
| M14 | Video | import/playback/trim via FilmCraft crates, global edits + presets on video, video export | 6–10 | ⬜ |
| M15 | Classic modules | Map, Book, Slideshow, Print, Web; smart collections, stacks, virtual copies, publish services, tethering | 25–40 | 🚧 (smart albums, stacks, virtual copies, compare/survey ✅; Map/Book/Slideshow/Print/Web ⬜) |
| M16 | 1.0 polish | preferences, shortcut editor, accessibility, localization, packaging (dmg/msi/AppImage/web), hardening | 10–20 | 🚧 (settings, keyboard shortcuts sheet, packaging basics ✅; accessibility, localisation ⬜) |

## Parity estimate (2026-10-02)

**By feature count** — from `docs/parity.md` (one row per Lightroom feature, menu item and shortcut; `cargo xtask
parity` prints this line on every run, so it stays current):

| Scope | Weighted completion | Rows |
|---|---:|---:|
| P0 (core) | **98.5%** | 198 |
| P1 (important parity) | **79.9%** | 144 |
| P2 (later / AI / niche, incl. Classic modules) | **11.7%** | 158 |
| **All in-scope rows** | **65.7%** | 500 |

✅ counts 1, 🟡 ½, ⬜ 0; out-of-scope rows (cloud sharing, Adobe accounts…) are left out.

**By remaining effort** — rows are not equal: a shortcut and the whole Book module are one row each, and what is
left is the heavy part (AI, video, Classic output modules, undocumented raw codecs). Remaining work in **Opus agent-hours**
(one agent working continuously; calibrated on this project — a single lead agent closed ≈ 45 tracker rows of
UI/feature work in ≈ 5 h on 2026-10-01, and four to six parallel agents built M0–M13 in ≈ 25 active hours):

| Work package | Tracker rows | Agent-hours | Risk |
|---|---|---:|---|
| Remaining P0/P1 UI and library features (version hover previews, filter presets, quick develop, missing-file relink, metadata presets, keyword sets…) | ≈ 30 | 20–35 | low |
| Raw codecs: CR3 (CRX), compressed NEF / ORF / RAF, RW2 v4, HEIC/AVIF decode | LR-IMP-FORMATS | 40–80 | **high** — clean-room black-box analysis, no permissive specs |
| Lens-profile database of our own (calibration targets, fitting, data) | LR-EDIT-OPTICS-PROFILE | 15–30 | data collection |
| Video: playback, trim, edits, export (pure-Rust decode, ideally shared with FilmCraft) | R. Video | 20–40 | medium |
| AI: subject / sky / background / people / object masks, object-aware remove, AI denoise, super resolution, lens blur, people & faces, natural-language search, culling | ≈ 30 | 80–150 | **high** — permissively licensed weights, pure-Rust inference, maybe training |
| HDR editing, display, visualisation and export | Q. HDR, LR-EXP-HDR | 15–25 | medium |
| Classic modules: Map, Book, Slideshow, Print, publish services, tethering, soft proofing | ≈ 50 | 60–100 | medium (large, well-understood) |
| Smaller P2 items: watched folders, Enhance dialog, external-editor round trip, export to Photos, help / what's new, accessibility, localisation | ≈ 25 | 20–35 | low |
| Look tuning, performance budgets (colour NR at half resolution on CPU + GPU, 100k-photo library), packaging, hardening | — | 25–45 | medium |
| **Total remaining** | | **≈ 295–540** | |

Spent so far ≈ 90–130 agent-hours, so **by effort the project is ≈ 20–30% of the way to complete Lightroom + Classic
parity**, and ≈ 40–55% of the way for cloud-Lightroom parity without AI and the Classic modules (remaining ≈ 135–235 h).

**Wall clock:** ≈ 300–540 h for one agent working alone; with 4–6 parallel agents (≈ 70% parallel efficiency, merges and
CI under load cost the rest) **≈ 75–130 h of continuous work**. The AI package and the raw codecs carry most of the
uncertainty: they can finish faster if suitable permissive models / documentation turn up, or stall on licensing.

## Totals

Remaining from 2026-10-02 (see *Parity estimate* above for the breakdown):

| Target | Remaining agent-hours | Wall clock, 4–6 parallel agents |
|---|---:|---:|
| Cloud-Lightroom parity without AI or Classic modules | ≈ 135–235 h | ≈ 35–60 h |
| Full parity incl. AI, video and the Classic modules | ≈ 295–540 h | ≈ 75–130 h |

The milestone estimates in the table above were made before work started and are kept for calibration (M0–M13 took
≈ 25 active hours with parallel agents against an estimate of ≈ 110–170 h for those milestones).

## Risks that coding hours alone don't retire

- **AI features** (subject/sky/people masks, generative remove) need model weights with licences we can ship; classical
  fallbacks first. No permissively licensed sky-segmentation or raw-denoise model was found — we may need to train our own.
- **Camera colour and lens data** is a data problem: we never use Adobe's matrices, DCPs or LCPs. DNG-embedded data first,
  then our own calibration; long-tail camera/lens coverage grows over time.
- **Legal decisions pending:** whether GPL-licensed *prose* format descriptions (e.g. the public CR3 write-up) may be read
  by a designated engineer to produce an internal spec; freedom-to-operate review for local Laplacian filters, PatchMatch and
  HEVC (HEIC).
- **Look parity** with Adobe's default rendering is subjective tuning against our own reference targets.

## Raw format coverage and known gaps

Decoded (CC0 corpus from raw.pixls.us, `cargo xtask corpus --download`, `crates/raw/tests/corpus.rs`): DNG (uncompressed,
LJ92, Deflate, float, linear), CR2, ARW (uncompressed, ARW2, LJ92), NEF/NRW uncompressed, RAF uncompressed (Bayer and
X-Trans), RW2 packed 12/14-bit, PEF (uncompressed and Huffman), ORF uncompressed (16-bit and 12-bit packed). Every
supported container also yields its embedded JPEG preview (CR3 too), and the engine shows that preview for raw variants
it can't decode yet.

Not decoded yet — preview only (no permissively licensed description; black-box analysis incomplete):
- **Nikon Huffman NEF** (lossless / lossy). Black-box findings so far: maker note `0x0096` holds per-parity predictor
  seeds (2048 for 14-bit lossless); ≈ 9 bits per pixel; the category-0 code word is a 6-bit rotation of `011111`. The
  Huffman tables are not stored in the files, and an automatic table search (beam search over prefix codes scored by
  smoothness and bit rate, validated on Pentax files whose tables *are* stored) did not converge.
- **Panasonic RW2 raw format 4** (quantised): the block layout is known (0x4000-byte chunks rotated by 0x1ff8; 128-bit
  blocks of two 12-bit seeds plus four groups of a 2-bit scale and three 8-bit codes; scales 0/1 are ×1/×2 differences
  from the same-colour pixel two to the left), the reconstruction rule for scales 2/3 is not established.
- **Olympus compressed ORF**, **Fujifilm compressed RAF**, **Canon CR3/CRX** (M11.1), **Canon sRAW/mRAW**, lossy DNG.

**Camera colour matrices:** non-DNG raws use our own calibration where one exists (`crates/raw/src/calibration.rs`:
Sony α7R IV / IVA so far, fitted to the camera's own Kelvin white-balance presets and the hues of its own JPEG
previews), else the documented neutral fallback (camera RGB ≈ linear sRGB, flagged `matrix_is_fallback`) with the
file's as-shot white-balance multipliers. Clean sources to evaluate next: the same preset-and-preview fit for more
models, manufacturer matrices stored in the files themselves (Olympus ImageProcessing `ColorMatrix`, Pentax/Panasonic
equivalents) and our own chart-based calibration (M11.4). Adobe matrices are never used.

## Log
- 2026-09-30: roadmap created; M0 in progress; research docs (Lightroom reference, Rust imaging ecosystem) complete.
- 2026-09-30 (later): app running with the full Lightroom-style UI; pipeline v0; DNG/CR2/ARW; README showcase. ≈ 8 h elapsed.
- 2026-10-02: parity estimate added (xtask parity prints weighted completion); milestone statuses refreshed.
- 2026-10-01: RAW II formats: RAF (uncompressed Bayer + X-Trans), RW2 (packed), PEF (incl. Huffman), ORF (uncompressed); embedded previews for every container incl. CR3; raw corpus test with 37 CC0 samples.
