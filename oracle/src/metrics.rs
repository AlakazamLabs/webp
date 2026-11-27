//! Quality metrics and the Bjontegaard rate difference.

/// PSNR over R, G and B.
pub fn psnr_rgb(a: &[u8], b: &[u8]) -> f64 {
    let err: u64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| (x as i64 - y as i64).pow(2) as u64)
        .sum();
    if err == 0 {
        return 99.0;
    }
    10.0 * (255.0f64 * 255.0 * a.len() as f64 / err as f64).log10()
}

fn luma(rgb: &[u8]) -> Vec<f32> {
    rgb.chunks_exact(3)
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect()
}

/// Separable Gaussian blur (sigma 1.5, 11 taps), edges clamped.
fn blur(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    let k: Vec<f32> = (-5i32..=5)
        .map(|i| (-(i * i) as f32 / (2.0 * 1.5 * 1.5)).exp())
        .collect();
    let sum: f32 = k.iter().sum();
    let k: Vec<f32> = k.iter().map(|v| v / sum).collect();
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let xx = (x as i32 + i as i32 - 5).clamp(0, w as i32 - 1) as usize;
                s += kv * src[y * w + xx];
            }
            tmp[y * w + x] = s;
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (i, kv) in k.iter().enumerate() {
                let yy = (y as i32 + i as i32 - 5).clamp(0, h as i32 - 1) as usize;
                s += kv * tmp[yy * w + x];
            }
            out[y * w + x] = s;
        }
    }
    out
}

/// SSIM on luma (Gaussian window, the usual constants).
pub fn ssim_luma(a: &[u8], b: &[u8], w: usize, h: usize) -> f64 {
    let (x, y) = (luma(a), luma(b));
    let xx: Vec<f32> = x.iter().map(|v| v * v).collect();
    let yy: Vec<f32> = y.iter().map(|v| v * v).collect();
    let xy: Vec<f32> = x.iter().zip(&y).map(|(p, q)| p * q).collect();
    let (mx, my) = (blur(&x, w, h), blur(&y, w, h));
    let (sxx, syy, sxy) = (blur(&xx, w, h), blur(&yy, w, h), blur(&xy, w, h));
    let c1 = (0.01f64 * 255.0).powi(2);
    let c2 = (0.03f64 * 255.0).powi(2);
    let mut total = 0f64;
    for i in 0..w * h {
        let (mx, my) = (mx[i] as f64, my[i] as f64);
        let vx = sxx[i] as f64 - mx * mx;
        let vy = syy[i] as f64 - my * my;
        let cxy = sxy[i] as f64 - mx * my;
        total += ((2.0 * mx * my + c1) * (2.0 * cxy + c2)) / ((mx * mx + my * my + c1) * (vx + vy + c2));
    }
    total / (w * h) as f64
}

/// SSIMULACRA2 from the `ssimulacra2` crate.
pub fn ssimulacra2(a: &[u8], b: &[u8], w: usize, h: usize) -> f64 {
    use ssimulacra2::{ColorPrimaries, Rgb, TransferCharacteristic};
    let conv = |p: &[u8]| -> Vec<[f32; 3]> {
        p.chunks_exact(3)
            .map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0])
            .collect()
    };
    let sa = Rgb::new(conv(a), w, h, TransferCharacteristic::SRGB, ColorPrimaries::BT709).expect("rgb");
    let sb = Rgb::new(conv(b), w, h, TransferCharacteristic::SRGB, ColorPrimaries::BT709).expect("rgb");
    ssimulacra2::compute_frame_ssimulacra2(sa, sb).expect("ssimulacra2")
}

/// Least-squares cubic fit y = c0 + c1 x + c2 x^2 + c3 x^3.
fn cubic_fit(x: &[f64], y: &[f64]) -> [f64; 4] {
    let mut m = [[0f64; 5]; 4];
    for (&xi, &yi) in x.iter().zip(y) {
        let pw = [1.0, xi, xi * xi, xi * xi * xi];
        for r in 0..4 {
            for c in 0..4 {
                m[r][c] += pw[r] * pw[c];
            }
            m[r][4] += pw[r] * yi;
        }
    }
    for col in 0..4 {
        let piv = (col..4)
            .max_by(|&a, &b| m[a][col].abs().total_cmp(&m[b][col].abs()))
            .expect("rows");
        m.swap(col, piv);
        let d = m[col][col];
        for c in col..5 {
            m[col][c] /= d;
        }
        for r in 0..4 {
            if r != col {
                let f = m[r][col];
                for c in col..5 {
                    m[r][c] -= f * m[col][c];
                }
            }
        }
    }
    [m[0][4], m[1][4], m[2][4], m[3][4]]
}

fn integral(c: &[f64; 4], a: f64, b: f64) -> f64 {
    let f = |x: f64| c[0] * x + c[1] * x * x / 2.0 + c[2] * x.powi(3) / 3.0 + c[3] * x.powi(4) / 4.0;
    f(b) - f(a)
}

/// The Bjontegaard rate difference of `test` against `anchor` in percent:
/// log-rate fitted as a cubic of the metric, averaged over the metric range
/// both cover. `None` without overlap or with fewer than four points.
pub fn bd_rate(anchor_rate: &[f64], anchor_metric: &[f64], test_rate: &[f64], test_metric: &[f64]) -> Option<f64> {
    if anchor_rate.len() < 4 || test_rate.len() < 4 {
        return None;
    }
    let la: Vec<f64> = anchor_rate.iter().map(|r| r.ln()).collect();
    let lt: Vec<f64> = test_rate.iter().map(|r| r.ln()).collect();
    let ca = cubic_fit(anchor_metric, &la);
    let ct = cubic_fit(test_metric, &lt);
    let min = |v: &[f64]| v.iter().cloned().fold(f64::MAX, f64::min);
    let max = |v: &[f64]| v.iter().cloned().fold(f64::MIN, f64::max);
    let lo = min(anchor_metric).max(min(test_metric));
    let hi = max(anchor_metric).min(max(test_metric));
    if !(hi > lo) {
        return None;
    }
    let avg = (integral(&ct, lo, hi) - integral(&ca, lo, hi)) / (hi - lo);
    Some((avg.exp() - 1.0) * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_the_rate_is_minus_fifty_percent() {
        let m = [30.0, 33.0, 36.0, 39.0, 42.0];
        let a: Vec<f64> = m.iter().map(|x: &f64| (x / 6.0).exp()).collect();
        let t: Vec<f64> = a.iter().map(|r| r / 2.0).collect();
        let bd = bd_rate(&a, &m, &t, &m).expect("overlap");
        assert!((bd + 50.0).abs() < 1e-6, "{bd}");
    }
}
