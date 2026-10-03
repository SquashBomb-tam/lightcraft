//! Kernels candle's CPU backend runs slowly: pointwise (1×1) convolution as one matrix product,
//! depthwise 3×3 convolution directly (a channel per task), and GELU on all cores.

use candle_core::{Result, Tensor};
use rayon::prelude::*;

/// A 1×1 convolution of `xs` (1 × C × H × W) with `w` (O × C × 1 × 1) plus `b` (1 × O × 1 × 1):
/// `w · x` over the flattened pixels.
pub fn pointwise(xs: &Tensor, w: &Tensor, b: &Tensor) -> Result<Tensor> {
    let (n, c, h, wd) = xs.dims4()?;
    let o = w.dim(0)?;
    if n != 1 {
        return xs.conv2d(w, 0, 1, 1, 1)?.broadcast_add(b);
    }
    let x = xs.reshape((c, h * wd))?;
    w.reshape((o, c))?.matmul(&x)?.reshape((1, o, h, wd))?.broadcast_add(b)
}

/// The error function (Abramowitz & Stegun 7.1.26, absolute error < 1.5e-7).
fn erf(x: f32) -> f32 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let y = 1.0 - (((((1.061_405_4 * t - 1.453_152_1) * t) + 1.421_413_8) * t - 0.284_496_74) * t + 0.254_829_6) * t * (-x * x).exp();
    y.copysign(x)
}

/// GELU (the exact, erf form): `x · Φ(x)`.
#[inline]
fn gelu1(x: f32) -> f32 {
    0.5 * x * (1.0 + erf(x * std::f32::consts::FRAC_1_SQRT_2))
}

/// GELU of every element, on all cores.
pub fn gelu(xs: &Tensor) -> Result<Tensor> {
    let mut v = xs.flatten_all()?.to_vec1::<f32>()?;
    v.par_chunks_mut(1 << 14).for_each(|c| c.iter_mut().for_each(|x| *x = gelu1(*x)));
    Tensor::from_vec(v, xs.shape(), xs.device())
}

/// A depthwise 3×3 convolution of `xs` (1 × C × H × W): channel `c` with the 9 weights at
/// `w[9c..9c + 9]` plus `b[c]`, at `stride` with `pad` zero padding. `gelu_in` / `gelu_out` apply
/// GELU to the input / the result on the way (saving a pass over the data each).
pub fn depthwise3x3(xs: &Tensor, w: &[f32], b: &[f32], stride: usize, pad: usize, gelu_in: bool, gelu_out: bool) -> Result<Tensor> {
    let (n, c, h, wd) = xs.dims4()?;
    if n != 1 || w.len() != 9 * c || b.len() != c || stride == 0 {
        candle_core::bail!("depthwise3x3: input {:?}, {} weights, {} biases, stride {stride}", xs.dims(), w.len(), b.len());
    }
    let (ho, wo) = ((h + 2 * pad).saturating_sub(3) / stride + 1, (wd + 2 * pad).saturating_sub(3) / stride + 1);
    let mut x = xs.flatten_all()?.to_vec1::<f32>()?;
    if gelu_in {
        x.par_chunks_mut(1 << 14).for_each(|c| c.iter_mut().for_each(|v| *v = gelu1(*v)));
    }
    let mut out = vec![0f32; c * ho * wo];
    out.par_chunks_mut(ho * wo).enumerate().for_each(|(ch, o)| {
        let src = &x[ch * h * wd..(ch + 1) * h * wd];
        let k = &w[9 * ch..9 * ch + 9];
        for oy in 0..ho {
            let row = &mut o[oy * wo..(oy + 1) * wo];
            row.fill(b[ch]);
            for ky in 0..3 {
                let iy = (oy * stride + ky) as isize - pad as isize;
                if iy < 0 || iy >= h as isize {
                    continue;
                }
                let line = &src[iy as usize * wd..(iy as usize + 1) * wd];
                for kx in 0..3 {
                    let kv = k[ky * 3 + kx];
                    for (ox, v) in row.iter_mut().enumerate() {
                        let ix = (ox * stride + kx) as isize - pad as isize;
                        if ix >= 0 && (ix as usize) < wd {
                            *v += kv * line[ix as usize];
                        }
                    }
                }
            }
            if gelu_out {
                row.iter_mut().for_each(|v| *v = gelu1(*v));
            }
        }
    });
    Tensor::from_vec(out, (1, c, ho, wo), xs.device())
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::Device;

    fn max_diff(a: &Tensor, b: &Tensor) -> f32 {
        assert_eq!(a.dims(), b.dims());
        let (a, b) = (a.flatten_all().unwrap().to_vec1::<f32>().unwrap(), b.flatten_all().unwrap().to_vec1::<f32>().unwrap());
        a.iter().zip(&b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn pointwise_matches_candle() {
        let d = Device::Cpu;
        let x = Tensor::randn(0f32, 1.0, (1, 6, 9, 7), &d).unwrap();
        let w = Tensor::randn(0f32, 1.0, (5, 6, 1, 1), &d).unwrap();
        let b = Tensor::randn(0f32, 1.0, (1, 5, 1, 1), &d).unwrap();
        let reference = x.conv2d(&w, 0, 1, 1, 1).unwrap().broadcast_add(&b).unwrap();
        assert!(max_diff(&pointwise(&x, &w, &b).unwrap(), &reference) < 1e-4);
    }

    #[test]
    fn depthwise_matches_candle() {
        let d = Device::Cpu;
        // odd and even sizes, stride 1 and 2, padding 0 and 1
        for (h, w, stride, pad) in [(9, 7, 1, 1), (8, 8, 2, 1), (10, 5, 1, 0), (11, 13, 2, 0)] {
            let x = Tensor::randn(0f32, 1.0, (1, 4, h, w), &d).unwrap();
            let k = Tensor::randn(0f32, 1.0, (4, 1, 3, 3), &d).unwrap();
            let b = vec![0.5f32, -1.0, 0.0, 2.0];
            let bt = Tensor::from_vec(b.clone(), (1, 4, 1, 1), &d).unwrap();
            let reference = x.conv2d(&k, pad, stride, 1, 4).unwrap().broadcast_add(&bt).unwrap();
            let kv = k.flatten_all().unwrap().to_vec1::<f32>().unwrap();
            let ours = depthwise3x3(&x, &kv, &b, stride, pad, false, false).unwrap();
            assert!(max_diff(&ours, &reference) < 1e-4, "{h}×{w} stride {stride} pad {pad}");
            // the fused GELUs equal separate ones
            let fused = depthwise3x3(&x, &kv, &b, stride, pad, true, true).unwrap();
            let gx = x.gelu_erf().unwrap();
            let separate = gx.conv2d(&k, pad, stride, 1, 4).unwrap().broadcast_add(&bt).unwrap().gelu_erf().unwrap();
            assert!(max_diff(&fused, &separate) < 1e-4, "fused GELU, {h}×{w}");
        }
    }

    #[test]
    fn depthwise_refuses_mismatched_weights() {
        let x = Tensor::zeros((1, 4, 8, 8), candle_core::DType::F32, &Device::Cpu).unwrap();
        assert!(depthwise3x3(&x, &[0.0; 9], &[0.0; 4], 1, 1, false, false).is_err());
        assert!(depthwise3x3(&x, &[0.0; 36], &[0.0; 4], 0, 1, false, false).is_err());
    }

    #[test]
    fn gelu_matches_candle() {
        let x = (Tensor::randn(0f32, 1.0, (3, 1000), &Device::Cpu).unwrap() * 4.0).unwrap();
        assert!(max_diff(&gelu(&x).unwrap(), &x.gelu_erf().unwrap()) < 1e-5);
        assert!((erf(0.0)).abs() < 1e-7 && (erf(3.0) - 0.999_977_9).abs() < 1e-6 && (erf(-1.0) + 0.842_700_8).abs() < 1e-6);
    }
}
