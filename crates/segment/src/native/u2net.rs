//! U²-Net (Qin et al., "U²-Net: Going Deeper with Nested U-Structure for Salient Object
//! Detection", 2020; reference implementation Apache-2.0): the Subject and Sky models.
//!
//! Weights come from `cargo xtask models` with every batch norm folded into its convolution and
//! the layers named `<stage>.<layer>.weight|bias` ([`layers`] lists them in execution order).

use std::collections::HashMap;

use candle_core::{D, DType, Device, Tensor};
use candle_nn::VarBuilder;
use lightcraft_raster::{Plane, Rgba8};

use crate::prep::{crop_resize, fit_long_edge, resized};

/// The model's input side (the photo is resized to a square, ignoring its aspect).
pub const SIZE: usize = 320;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// One convolution layer: its name, input and output channels, kernel size and dilation (padding
/// equals the dilation for 3 × 3 kernels).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub name: String,
    pub in_ch: usize,
    pub out_ch: usize,
    pub kernel: usize,
    pub dilation: usize,
}

/// A residual U-block: `height` levels (RSU7 … RSU4), or the dilated, unpooled RSU4F.
struct Rsu {
    name: &'static str,
    height: usize,
    dilated: bool,
    in_ch: usize,
    mid: usize,
    out: usize,
}

const STAGES: [Rsu; 11] = [
    Rsu { name: "stage1", height: 7, dilated: false, in_ch: 3, mid: 32, out: 64 },
    Rsu { name: "stage2", height: 6, dilated: false, in_ch: 64, mid: 32, out: 128 },
    Rsu { name: "stage3", height: 5, dilated: false, in_ch: 128, mid: 64, out: 256 },
    Rsu { name: "stage4", height: 4, dilated: false, in_ch: 256, mid: 128, out: 512 },
    Rsu { name: "stage5", height: 4, dilated: true, in_ch: 512, mid: 256, out: 512 },
    Rsu { name: "stage6", height: 4, dilated: true, in_ch: 512, mid: 256, out: 512 },
    Rsu { name: "stage5d", height: 4, dilated: true, in_ch: 1024, mid: 256, out: 512 },
    Rsu { name: "stage4d", height: 4, dilated: false, in_ch: 1024, mid: 128, out: 256 },
    Rsu { name: "stage3d", height: 5, dilated: false, in_ch: 512, mid: 64, out: 128 },
    Rsu { name: "stage2d", height: 6, dilated: false, in_ch: 256, mid: 32, out: 64 },
    Rsu { name: "stage1d", height: 7, dilated: false, in_ch: 128, mid: 16, out: 64 },
];

/// Side outputs: (name, input channels).
const SIDES: [(&str, usize); 6] = [("side1", 64), ("side2", 64), ("side3", 128), ("side4", 256), ("side5", 512), ("side6", 512)];

impl Rsu {
    /// Dilation of encoder level `i` (1-based; level `height` is the bottom).
    fn dil(&self, i: usize) -> usize {
        if self.dilated {
            1 << (i - 1)
        } else if i == self.height {
            2
        } else {
            1
        }
    }

    /// The block's layers in execution order: `rebnconvin`, `rebnconv1..height`, then the decoder
    /// `rebnconv(height-1)d .. rebnconv1d`.
    fn layers(&self) -> Vec<Layer> {
        let l = |n: String, i, o, d| Layer { name: format!("{}.{n}", self.name), in_ch: i, out_ch: o, kernel: 3, dilation: d };
        let mut v = vec![l("rebnconvin".into(), self.in_ch, self.out, 1), l("rebnconv1".into(), self.out, self.mid, 1)];
        for i in 2..=self.height {
            v.push(l(format!("rebnconv{i}"), self.mid, self.mid, self.dil(i)));
        }
        for i in (1..self.height).rev() {
            v.push(l(format!("rebnconv{i}d"), 2 * self.mid, if i == 1 { self.out } else { self.mid }, self.dil(i)));
        }
        v
    }
}

/// Every convolution of U²-Net in execution order (the order an ONNX export lists them in).
pub fn layers() -> Vec<Layer> {
    let mut v: Vec<Layer> = STAGES.iter().flat_map(Rsu::layers).collect();
    v.extend(SIDES.iter().map(|(n, c)| Layer { name: n.to_string(), in_ch: *c, out_ch: 1, kernel: 3, dilation: 1 }));
    v.push(Layer { name: "outconv".into(), in_ch: 6, out_ch: 1, kernel: 1, dilation: 1 });
    v
}

/// How a model wants its input scaled before the ImageNet normalization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scale {
    /// Divide by 255 (the sky model).
    Fixed,
    /// Divide by the image's brightest value (U²-Net's reference preprocessing).
    Max,
}

pub struct U2Net {
    convs: HashMap<String, (Tensor, Tensor, usize)>,
    scale: Scale,
}

impl U2Net {
    pub fn load(vb: VarBuilder, scale: Scale) -> candle_core::Result<U2Net> {
        let mut convs = HashMap::new();
        for l in layers() {
            let w = vb.get((l.out_ch, l.in_ch, l.kernel, l.kernel), &format!("{}.weight", l.name))?.to_dtype(DType::F32)?;
            let b = vb.get(l.out_ch, &format!("{}.bias", l.name))?.to_dtype(DType::F32)?.reshape((1, l.out_ch, 1, 1))?;
            convs.insert(l.name, (w, b, l.dilation));
        }
        Ok(U2Net { convs, scale })
    }

    fn conv(&self, name: &str, x: &Tensor) -> candle_core::Result<Tensor> {
        let (w, b, d) = self.convs.get(name).ok_or_else(|| candle_core::Error::Msg(format!("u2net: no layer {name}")))?;
        let pad = if w.dim(3)? == 1 { 0 } else { *d };
        x.conv2d(w, pad, 1, *d, 1)?.broadcast_add(b)
    }

    /// conv → (folded batch norm) → ReLU.
    fn rebn(&self, name: &str, x: &Tensor) -> candle_core::Result<Tensor> {
        self.conv(name, x)?.relu()
    }

    fn rsu(&self, r: &Rsu, x: &Tensor) -> candle_core::Result<Tensor> {
        let n = |s: &str| format!("{}.{s}", r.name);
        let hxin = self.rebn(&n("rebnconvin"), x)?;
        let mut enc = vec![self.rebn(&n("rebnconv1"), &hxin)?];
        for i in 2..=r.height {
            let prev = enc.last().expect("level 1");
            // RSU4F and the bottom level don't pool
            let input = if r.dilated || i == r.height { prev.clone() } else { pool(prev)? };
            enc.push(self.rebn(&n(&format!("rebnconv{i}")), &input)?);
        }
        let mut d = enc[r.height - 1].clone();
        for i in (1..r.height).rev() {
            let skip = &enc[i - 1];
            let up = if d.dims()[2..] == skip.dims()[2..] { d } else { upsample_like(&d, skip)? };
            d = self.rebn(&n(&format!("rebnconv{i}d")), &Tensor::cat(&[&up, skip], 1)?)?;
        }
        d + hxin
    }

    /// The fused output (probability, 0..1) for a normalized `1 × 3 × SIZE × SIZE` input.
    fn forward(&self, x: &Tensor) -> candle_core::Result<Tensor> {
        let mut enc: Vec<Tensor> = Vec::with_capacity(6);
        let mut h = x.clone();
        for (i, st) in STAGES[..6].iter().enumerate() {
            if i > 0 {
                h = pool(&h)?;
            }
            h = self.rsu(st, &h)?;
            enc.push(h.clone());
        }
        // decoder: stage5d … stage1d, each on (the level below upsampled, this level's encoder)
        let mut dec = vec![enc[5].clone()];
        let mut d = enc[5].clone();
        for (k, st) in STAGES[6..].iter().enumerate() {
            let skip = &enc[4 - k];
            d = self.rsu(st, &Tensor::cat(&[&upsample_like(&d, skip)?, skip], 1)?)?;
            dec.push(d.clone());
        }
        // side outputs from stage1d (finest) … stage6 (coarsest), upsampled and fused
        dec.reverse();
        let d1 = self.conv("side1", &dec[0])?;
        let mut sides = vec![d1.clone()];
        for (i, (name, _)) in SIDES.iter().enumerate().skip(1) {
            sides.push(upsample_like(&self.conv(name, &dec[i])?, &d1)?);
        }
        candle_nn::ops::sigmoid(&self.conv("outconv", &Tensor::cat(&sides, 1)?)?)
    }

    /// The probability of each pixel of `img` (at a SIZE long-edge map with `img`'s aspect).
    pub fn segment(&self, img: &Rgba8) -> candle_core::Result<Plane> {
        let r = resized(img, SIZE, SIZE);
        let k = match self.scale {
            Scale::Fixed => 1.0,
            Scale::Max => 1.0 / r.data.iter().flatten().fold(1e-6f32, |m, v| m.max(*v)),
        };
        let mut chw = vec![0f32; 3 * SIZE * SIZE];
        for (i, p) in r.data.iter().enumerate() {
            for c in 0..3 {
                chw[c * SIZE * SIZE + i] = (p[c] * k - MEAN[c]) / STD[c];
            }
        }
        let x = Tensor::from_vec(chw, (1, 3, SIZE, SIZE), &Device::Cpu)?;
        let y: Vec<f32> = self.forward(&x)?.flatten_all()?.to_vec1()?;
        let square = Plane { width: SIZE, height: SIZE, data: y };
        let (w, h) = fit_long_edge(img.width, img.height, SIZE);
        Ok(crop_resize(&square, SIZE, SIZE, w, h))
    }
}

/// 2 × 2 max pooling, stride 2, rounding the output size up (`ceil_mode`): odd sizes get their
/// last row / column repeated first, which leaves the maxima unchanged.
fn pool(x: &Tensor) -> candle_core::Result<Tensor> {
    let (_, _, h, w) = x.dims4()?;
    let mut x = x.clone();
    if h % 2 == 1 {
        x = Tensor::cat(&[&x, &x.narrow(2, h - 1, 1)?], 2)?;
    }
    if w % 2 == 1 {
        x = Tensor::cat(&[&x, &x.narrow(3, w - 1, 1)?], 3)?;
    }
    x.max_pool2d(2)
}

/// `x` resized (bilinear, half-pixel centres, like PyTorch's `align_corners=False`) to `like`'s
/// spatial size.
fn upsample_like(x: &Tensor, like: &Tensor) -> candle_core::Result<Tensor> {
    let (h, w) = (like.dim(D::Minus2)?, like.dim(D::Minus1)?);
    x.upsample_bilinear2d(h, w, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_list_matches_the_reference_architecture() {
        let l = layers();
        // 119 convolutions: 60 in the encoder, 52 in the decoder, 6 sides, the fusion
        assert_eq!(l.len(), 119);
        assert_eq!(l[0], Layer { name: "stage1.rebnconvin".into(), in_ch: 3, out_ch: 64, kernel: 3, dilation: 1 });
        assert_eq!(l[1].name, "stage1.rebnconv1");
        // RSU7's bottom is dilated, its decoder starts at 6d and ends at 1d back to 64 channels
        let s1: Vec<&Layer> = l.iter().filter(|x| x.name.starts_with("stage1.")).collect();
        assert_eq!(s1.len(), 14);
        assert_eq!((s1[7].name.as_str(), s1[7].dilation), ("stage1.rebnconv7", 2));
        assert_eq!((s1[8].name.as_str(), s1[8].in_ch), ("stage1.rebnconv6d", 64));
        assert_eq!((s1[13].name.as_str(), s1[13].out_ch), ("stage1.rebnconv1d", 64));
        // RSU4F dilates 1, 2, 4, 8 and back
        let s5: Vec<usize> = l.iter().filter(|x| x.name.starts_with("stage5.")).map(|x| x.dilation).collect();
        assert_eq!(s5, [1, 1, 2, 4, 8, 4, 2, 1]);
        // the tail: six sides, then the 1 × 1 fusion of their six maps
        assert_eq!(l[112].name, "side1");
        assert_eq!((l[118].name.as_str(), l[118].in_ch, l[118].kernel), ("outconv", 6, 1));
        // every name is unique
        let mut names: Vec<&str> = l.iter().map(|x| x.name.as_str()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 119);
    }

    /// A synthetic 320 × 320 landscape, normalized: sky above a wavy horizon at y ≈ 140, textured
    /// ground below, a red disc (radius 55) at (200, 190).
    fn landscape() -> Tensor {
        let mut chw = vec![0f32; 3 * SIZE * SIZE];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let (xf, yf) = (x as f64, y as f64);
                let horizon = 140.0 + 20.0 * (xf / 30.0).sin();
                let mut p = if yf < horizon {
                    let t = yf / horizon;
                    [0.35 + 0.25 * t, 0.55 + 0.2 * t, 0.95 - 0.1 * t]
                } else {
                    let tex = 0.08 * (xf * 0.7).sin() * (yf * 0.3).cos();
                    [0.32 + tex, 0.27 + tex, 0.16 + tex]
                };
                if (xf - 200.0).powi(2) + (yf - 190.0).powi(2) < 55.0f64.powi(2) {
                    p = [0.75, 0.2, 0.15];
                }
                for c in 0..3 {
                    chw[c * SIZE * SIZE + y * SIZE + x] = ((p[c] - MEAN[c] as f64) / STD[c] as f64) as f32;
                }
            }
        }
        Tensor::from_vec(chw, (1, 3, SIZE, SIZE), &Device::Cpu).unwrap()
    }

    /// The installed weights through this implementation against the reference implementations'
    /// outputs on [`landscape`] (onnxruntime 1.30 on `skyseg.onnx`; PyTorch 2.14 running the U²-Net
    /// repository's `U2NET` on `u2net.pth`): the mean, the share above 0.5, and pixels on the
    /// edges, where any mistake in the architecture shows first. The tolerances allow for the f16
    /// weights (the largest difference anywhere measured 0.0003 for sky, 0.019 for subject).
    #[test]
    #[ignore = "needs the models: cargo xtask models --download"]
    fn matches_the_reference_implementations() {
        let dir = crate::find_models_dir(None).expect("models directory");
        // (model, mean, share above 0.5, (x, y, value) on edges)
        type Case = (&'static str, f32, f32, [(usize, usize, f32); 8]);
        let cases: [Case; 2] = [
            (
                "sky",
                0.43982,
                0.43957,
                [
                    (112, 129, 0.163),
                    (280, 142, 0.1965),
                    (228, 143, 0.2971),
                    (84, 147, 0.1959),
                    (19, 152, 0.2154),
                    (71, 154, 0.2445),
                    (25, 155, 0.3215),
                    (69, 155, 0.2366),
                ],
            ),
            (
                "subject",
                0.09252,
                0.09249,
                [
                    (237, 150, 0.7437),
                    (252, 208, 0.3399),
                    (249, 214, 0.5833),
                    (155, 222, 0.193),
                    (235, 232, 0.8059),
                    (171, 237, 0.4281),
                    (222, 240, 0.7454),
                    (183, 242, 0.7896),
                ],
            ),
        ];
        let x = landscape();
        for (name, mean, above, points) in cases {
            let t = candle_core::safetensors::load(dir.join(format!("{name}.safetensors")), &Device::Cpu).expect(name);
            let net = U2Net::load(VarBuilder::from_tensors(t, DType::F32, &Device::Cpu), Scale::Fixed).unwrap();
            let y: Vec<f32> = net.forward(&x).unwrap().flatten_all().unwrap().to_vec1().unwrap();
            let m = y.iter().sum::<f32>() / y.len() as f32;
            let a = y.iter().filter(|v| **v > 0.5).count() as f32 / y.len() as f32;
            assert!((m - mean).abs() < 0.002, "{name}: mean {m} (reference {mean})");
            assert!((a - above).abs() < 0.002, "{name}: share above 0.5 {a} (reference {above})");
            for (px, py, want) in points {
                let got = y[py * SIZE + px];
                assert!((got - want).abs() < 0.03, "{name} at ({px}, {py}): {got} (reference {want})");
            }
        }
    }

    #[test]
    fn pooling_rounds_up_and_upsampling_matches_sizes() {
        let x = Tensor::arange(0f32, 25.0, &Device::Cpu).unwrap().reshape((1, 1, 5, 5)).unwrap();
        let p = pool(&x).unwrap();
        assert_eq!(p.dims(), &[1, 1, 3, 3]);
        let v: Vec<f32> = p.flatten_all().unwrap().to_vec1().unwrap();
        // window maxima, the last row / column alone
        assert_eq!(v, [6.0, 8.0, 9.0, 16.0, 18.0, 19.0, 21.0, 23.0, 24.0]);
        let u = upsample_like(&p, &x).unwrap();
        assert_eq!(u.dims(), &[1, 1, 5, 5]);
    }
}
