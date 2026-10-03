# AI mask models

The Sky, Subject, Background and Object masks use three neural networks. Their weights are not
committed (about 217 MB in total); `cargo xtask models --download` fetches the upstream files into
`models/download/`, checks each against its pinned SHA-256 (`xtask/src/models.rs`), and converts
them into the files the app loads:

| File | Used for | Model | Upstream file | Licence |
|---|---|---|---|---|
| `object.safetensors` (40.6 MB, f32) | Select Object (click or box) | MobileSAM: Segment Anything's prompt encoder and mask decoder with a TinyViT-5M image encoder (Zhang et al., 2023) | `weights/mobile_sam.pt`, github.com/ChaoningZhang/MobileSAM | Apache-2.0 (`licenses/Apache-2.0.txt`); the TinyViT architecture is MIT (`licenses/MIT-TinyViT.txt`) |
| `subject.safetensors` (88.0 MB, f16) | Select Subject, Select Background | U²-Net (Qin et al., 2020) | `u2net.pth`, github.com/xuebinqin/U-2-Net (byte-identical mirror pinned on Hugging Face) | Apache-2.0 (`licenses/Apache-2.0.txt`) |
| `sky.safetensors` (88.0 MB, f16) | Select Sky | U²-Net trained for sky segmentation | `skyseg.onnx`, github.com/xiongzhu666/Sky-Segmentation-and-Post-processing (mirror pinned on Hugging Face) | MIT (`licenses/MIT-Sky-Segmentation.txt`) |

The conversion changes no weights beyond storage: batch norms are folded into their convolutions,
U²-Net layers are given their architectural names (the sky model's ONNX convolutions are mapped
onto them in execution order, every shape and dilation checked), and the U²-Net weights are stored
as 16-bit floats. `crates/segment/src/native/u2net.rs` checks the result against the reference
implementations (onnxruntime and PyTorch) on a fixed test image.

Inference is pure Rust ([candle](https://github.com/huggingface/candle), CPU). Without these files
the masks fall back to LightCraft's classical estimates and Select Object is unavailable.

**Installed applications** keep the files in a `models/` folder beside the executable (Windows
installer and portable zip), in `Contents/Resources/models/` (macOS) or
`share/lightcraft/models/` (Linux); `LIGHTCRAFT_MODELS` overrides the location.

**Training data.** The licences above cover the weights as their authors published them. The
datasets they were trained on carry their own terms: MobileSAM was distilled on part of Meta's
SA-1B; U²-Net was trained on DUTS-TR, a research dataset of web images; the sky model's training
data is not documented. The weights' authors released them under the permissive licences listed;
whether a dataset's terms reach a model trained on it is not settled law.
