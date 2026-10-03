//! Select Object: MobileSAM (Segment Anything with a TinyViT-5M image encoder; Apache-2.0) on
//! candle-transformers' implementation. The image encoder runs once per photo ([`Sam::embed`]);
//! each prompt (points and/or a box) then decodes in a fraction of a second ([`Sam::object`]).

use candle_core::{DType, Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::segment_anything::mask_decoder::MaskDecoder;
use candle_transformers::models::segment_anything::prompt_encoder::PromptEncoder;
use lightcraft_raster::{Plane, Rgba8};

use super::tiny_vit::{TinyViT, tiny_vit_5m};
use crate::Prompt;
use crate::prep::{crop_resize, letterbox, sigmoid};

/// The encoder's input side; the image is scaled to fit and padded at the bottom right.
const SIZE: usize = 1024;
/// The image embedding's grid (SIZE / 16).
const GRID: usize = 64;
const EMBED_DIM: usize = 256;
/// ImageNet statistics in 0..255, as the model was trained.
const MEAN: [f32; 3] = [123.675, 116.28, 103.53];
const STD: [f32; 3] = [58.395, 57.12, 57.375];

pub struct Sam {
    encoder: TinyViT,
    prompt: PromptEncoder,
    decoder: MaskDecoder,
    sparse: Sparse,
}

/// The prompt encoder's sparse tokens (points and box corners), computed here from its weights:
/// candle-transformers 0.9's box path builds a tensor of the wrong rank. Same maths as the
/// reference implementation: a random-Fourier position encoding plus a learned embedding per
/// label (0 off the object, 1 on it, 2/3 the box's corners), and a "not a point" token padding
/// point-only prompts.
struct Sparse {
    /// `positional_encoding_gaussian_matrix`, 2 × 128.
    gauss: [[f32; EMBED_DIM / 2]; 2],
    not_a_point: Vec<f32>,
    labels: [Vec<f32>; 4],
}

impl Sparse {
    fn load(vb: VarBuilder) -> candle_core::Result<Sparse> {
        let g: Vec<Vec<f32>> = vb.pp("pe_layer").get((2, EMBED_DIM / 2), "positional_encoding_gaussian_matrix")?.to_vec2()?;
        let mut gauss = [[0f32; EMBED_DIM / 2]; 2];
        for (row, src) in gauss.iter_mut().zip(&g) {
            row.copy_from_slice(src);
        }
        let row = |vb: VarBuilder| -> candle_core::Result<Vec<f32>> { vb.get((1, EMBED_DIM), "weight")?.flatten_all()?.to_vec1() };
        let pe = vb.pp("point_embeddings");
        Ok(Sparse { gauss, not_a_point: row(vb.pp("not_a_point_embed"))?, labels: [row(pe.pp(0))?, row(pe.pp(1))?, row(pe.pp(2))?, row(pe.pp(3))?] })
    }

    /// The token of a point at `(x, y)` (encoder input pixels) with `label`.
    fn token(&self, x: f32, y: f32, label: usize) -> Vec<f32> {
        // pixel centres, normalized to -1..1
        let (u, v) = (2.0 * (x + 0.5) / SIZE as f32 - 1.0, 2.0 * (y + 0.5) / SIZE as f32 - 1.0);
        let half = EMBED_DIM / 2;
        let mut t = vec![0f32; EMBED_DIM];
        for j in 0..half {
            let a = std::f32::consts::TAU * (u * self.gauss[0][j] + v * self.gauss[1][j]);
            t[j] = a.sin() + self.labels[label][j];
            t[half + j] = a.cos() + self.labels[label][half + j];
        }
        t
    }

    /// `(1, n, 256)` tokens for `points` (`(x, y, on the object)`) and a `bbox` (x0, y0, x1, y1),
    /// all in encoder input pixels.
    fn tokens(&self, points: &[(f32, f32, bool)], bbox: Option<[f32; 4]>) -> candle_core::Result<Tensor> {
        let mut t: Vec<f32> = points.iter().flat_map(|&(x, y, on)| self.token(x, y, on as usize)).collect();
        match bbox {
            Some([x0, y0, x1, y1]) => {
                t.extend(self.token(x0, y0, 2));
                t.extend(self.token(x1, y1, 3));
            }
            None => t.extend_from_slice(&self.not_a_point),
        }
        let n = t.len() / EMBED_DIM;
        Tensor::from_vec(t, (1, n, EMBED_DIM), &Device::Cpu)
    }
}

/// An image's features and the size it was scaled to inside the encoder's square input.
#[derive(Clone)]
pub struct Embedding {
    features: Tensor,
    /// The scaled image's size in input pixels.
    scaled: (usize, usize),
    /// The original image's size.
    original: (usize, usize),
}

impl Sam {
    pub fn load(vb: VarBuilder) -> candle_core::Result<Sam> {
        let encoder = tiny_vit_5m(vb.pp("image_encoder"))?;
        let prompt = PromptEncoder::new(EMBED_DIM, (GRID, GRID), (SIZE, SIZE), 16, vb.pp("prompt_encoder"))?;
        let decoder = MaskDecoder::new(EMBED_DIM, 3, 3, 256, vb.pp("mask_decoder"))?;
        let sparse = Sparse::load(vb.pp("prompt_encoder"))?;
        Ok(Sam { encoder, prompt, decoder, sparse })
    }

    pub fn embed(&self, img: &Rgba8) -> candle_core::Result<Embedding> {
        use candle_core::Module;
        // pad with the mean colour: it normalizes to 0, like the reference implementation's
        // zero padding after normalization
        let (canvas, scaled) = letterbox(img, SIZE, MEAN.map(|v| v / 255.0));
        let mut chw = vec![0f32; 3 * SIZE * SIZE];
        for (i, p) in canvas.data.iter().enumerate() {
            for c in 0..3 {
                chw[c * SIZE * SIZE + i] = (p[c] * 255.0 - MEAN[c]) / STD[c];
            }
        }
        let x = Tensor::from_vec(chw, (1, 3, SIZE, SIZE), &Device::Cpu)?;
        let features = self.encoder.forward(&x)?;
        Ok(Embedding { features, scaled, original: (img.width, img.height) })
    }

    /// The probability map (original image size) of the object `prompt` (normalized coordinates)
    /// selects.
    pub fn object(&self, emb: &Embedding, prompt: &Prompt) -> candle_core::Result<Plane> {
        let (sw, sh) = (emb.scaled.0 as f32, emb.scaled.1 as f32);
        // normalized → the encoder's input pixels (pixel indices: the token adds the half pixel);
        // prompts off the photo are clamped onto it
        let to_input = |x: f64, y: f64| ((x.clamp(0.0, 1.0) as f32 * sw - 0.5).max(0.0), (y.clamp(0.0, 1.0) as f32 * sh - 0.5).max(0.0));
        let points: Vec<(f32, f32, bool)> = prompt
            .points
            .iter()
            .map(|&(x, y, on)| {
                let (x, y) = to_input(x, y);
                (x, y, on)
            })
            .collect();
        let bbox = prompt.bbox.map(|[x0, y0, x1, y1]| {
            let ((a, b), (c, d)) = (to_input(x0, y0), to_input(x1, y1));
            [a, b, c, d]
        });
        let sparse = self.sparse.tokens(&points, bbox)?;
        // the dense input of a prompt without a mask
        let (_, dense) = self.prompt.forward(None, None, None)?;
        let pe = self.prompt.get_dense_pe()?;
        // a single point is ambiguous (a shirt, the person, the group): ask for three candidates
        // and keep the one the model rates best; boxes and several points ask for one
        let multimask = prompt.bbox.is_none() && prompt.points.len() == 1;
        let (masks, iou) = self.decoder.forward(&emb.features, &pe, &sparse, &dense, multimask)?;
        let best = if multimask {
            let scores: Vec<f32> = iou.i(0)?.to_dtype(DType::F32)?.to_vec1()?;
            scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(i, _)| i)
        } else {
            0
        };
        // low-res logits over the padded square input → the scaled image's part → original size
        let logits = masks.i((0, best))?.to_dtype(DType::F32)?;
        let (lh, lw) = logits.dims2()?;
        let data: Vec<f32> = logits.flatten_all()?.to_vec1()?;
        let prob = Plane { width: lw, height: lh, data: data.into_iter().map(sigmoid).collect() };
        let (vw, vh) = (((sw / SIZE as f32) * lw as f32).round() as usize, ((sh / SIZE as f32) * lh as f32).round() as usize);
        Ok(crop_resize(&prob, vw, vh, emb.original.0, emb.original.1))
    }
}
