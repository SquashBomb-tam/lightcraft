# Asset attribution

Every asset shipped in this repository is listed here (see the asset rules in `AGENTS.md`). No asset may come from
Adobe products. Add an entry in the same commit as the asset.
`cargo xtask assets` (part of `cargo xtask ci`) fails if any image, icon, font, sound, video, raw file or colour profile
in the repository is not matched by a path pattern in the first column (`*` = any characters except `/`; a trailing `/`
covers a whole directory), or if a licence file referenced in the Licence column is missing.

| Path | Asset | Creator | Source | Licence | Added | Modifications |
|---|---|---|---|---|---|---|
| `assets/fonts/Inter-*.ttf` (Regular, Medium, SemiBold) | Inter | Rasmus Andersson | https://github.com/rsms/inter | SIL Open Font License 1.1 (`assets/fonts/OFL-Inter.txt`) | 2026-09-30 | none |
| `assets/app-icon/` (`lightcraft.svg` and the PNG/ICNS/ICO/SVG files derived from it) | LightCraft app icon (lynx) | LightCraft project owner (drawn in ArtCraft) | original work (vectorised from the owner's drawing; see `assets/app-icon/README.md`) | MIT OR Apache-2.0 (`assets/app-icon/LICENSE.txt`) | 2026-10-02 | vectorised to SVG; rendered to PNG/ICNS/ICO by `packaging/icons.sh` |
| `crates/scenes/` (generated images) | Procedural demo photographs | LightCraft contributors | original work (generated at runtime, no source imagery) | MIT OR Apache-2.0 | 2026-09-30 | n/a |
| `crates/ui-egui/src/icons.rs` | UI icons drawn as vector paths in code | LightCraft contributors | original work | MIT OR Apache-2.0 | 2026-09-30 | n/a |
| `docs/images/*-tetons.jpg`, `docs/images/grid-pd.jpg` (screenshots containing it) | "The Tetons and the Snake River" | Ansel Adams (1942), U.S. National Archives | https://commons.wikimedia.org/wiki/File:Adams_The_Tetons_and_the_Snake_River.jpg | Public domain (U.S. federal government work) | 2026-09-30 | developed in LightCraft; shown inside app screenshots |
| `docs/images/*-migrant-mother.jpg`, `docs/images/grid-pd.jpg` | "Migrant Mother" | Dorothea Lange (1936), Farm Security Administration / Library of Congress | https://commons.wikimedia.org/wiki/File:Lange-MigrantMother02.jpg | Public domain (U.S. federal government work) | 2026-09-30 | developed in LightCraft; shown inside app screenshots |
| `docs/images/*-earthrise.jpg`, `docs/images/grid-pd.jpg` | "Earthrise" (AS08-14-2383) | NASA / Bill Anders, Apollo 8 (1968) | https://commons.wikimedia.org/wiki/File:NASA-Apollo8-Dec24-Earthrise.jpg | Public domain (NASA) | 2026-09-30 | developed in LightCraft; shown inside app screenshots |
| `docs/images/*-blue-marble.jpg`, `docs/images/grid-pd.jpg` | "The Blue Marble" (AS17-148-22727) | NASA, Apollo 17 crew (1972) | https://commons.wikimedia.org/wiki/File:The_Earth_seen_from_Apollo_17.jpg | Public domain (NASA) | 2026-09-30 | developed in LightCraft; shown inside app screenshots |
| `models/object.safetensors` (not committed; `cargo xtask models`) | MobileSAM weights (Segment Anything prompt encoder and mask decoder, TinyViT-5M image encoder) | Chaoning Zhang et al. (Kyung Hee University); Segment Anything by Meta AI; TinyViT architecture by Microsoft | https://github.com/ChaoningZhang/MobileSAM (`weights/mobile_sam.pt`, commit c12dd83) | Apache-2.0 (`models/licenses/Apache-2.0.txt`); TinyViT code MIT (`models/licenses/MIT-TinyViT.txt`) | 2026-10-03 | converted to safetensors (f32), no weight changes |
| `models/subject.safetensors` (not committed; `cargo xtask models`) | U²-Net salient object detection weights | Xuebin Qin et al. | https://github.com/xuebinqin/U-2-Net (`u2net.pth`) | Apache-2.0 (`models/licenses/Apache-2.0.txt`) | 2026-10-03 | batch norms folded into convolutions, stored as f16 |
| `models/sky.safetensors` (not committed; `cargo xtask models`) | Sky segmentation weights (U²-Net architecture) | xiongzhu666 | https://github.com/xiongzhu666/Sky-Segmentation-and-Post-processing (`skyseg.onnx`) | MIT (`models/licenses/MIT-Sky-Segmentation.txt`) | 2026-10-03 | read from ONNX, mapped onto named U²-Net layers, stored as f16 |
| `docs/images/*.jpg` (UI) | LightCraft application screenshots | LightCraft contributors | original work (captured with `docs/showcase/`) | MIT OR Apache-2.0 | 2026-09-30 | n/a |

Notes
- Fonts authored or published by Adobe (Source Sans/Serif/Code, Source Han, …) are not used, even though some are
  OFL-licensed: the asset rule excludes anything from Adobe. Source Sans 3 was removed on 2026-09-30 and replaced by Inter.
