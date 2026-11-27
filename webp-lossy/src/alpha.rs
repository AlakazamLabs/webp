//! The ALPH chunk (RFC 9649 Section 2.7.1.2): the alpha plane, raw or,
//! with the `vp8l` feature, filtered and compressed with the WebP lossless
//! format.

/// The ALPH payload for `alpha` (one byte per pixel, scan order).
/// `quality` below 100 reduces the number of alpha levels first, which
/// helps only the compressed form.
pub(crate) fn alph_payload(
    width: usize,
    height: usize,
    alpha: &[u8],
    quality: u8,
    method: u8,
) -> Result<Vec<u8>, crate::EncodeError> {
    #[cfg(feature = "vp8l")]
    {
        compressed(width, height, alpha, quality, method)
    }
    #[cfg(not(feature = "vp8l"))]
    {
        let _ = (width, height, quality, method);
        let mut out = Vec::with_capacity(alpha.len() + 1);
        out.push(0); // no preprocessing, no filter, no compression
        out.extend_from_slice(alpha);
        Ok(out)
    }
}

/// Filtering methods (RFC 9649 Section 2.7.1.2).
#[cfg_attr(not(feature = "vp8l"), allow(dead_code))]
pub(crate) fn filter(width: usize, height: usize, alpha: &[u8], method: u8) -> Vec<u8> {
    let mut out = vec![0u8; alpha.len()];
    let at = |x: usize, y: usize| alpha[y * width + x] as i32;
    for y in 0..height {
        for x in 0..width {
            let pred = if (x == 0 && y == 0) || method == 0 {
                0
            } else if y == 0 {
                at(x - 1, 0)
            } else if x == 0 {
                at(0, y - 1)
            } else {
                match method {
                    1 => at(x - 1, y),
                    2 => at(x, y - 1),
                    _ => (at(x - 1, y) + at(x, y - 1) - at(x - 1, y - 1)).clamp(0, 255),
                }
            };
            out[y * width + x] = (at(x, y) - pred).rem_euclid(256) as u8;
        }
    }
    out
}

/// The inverse of [`filter`], as a decoder applies it.
#[cfg(test)]
pub(crate) fn unfilter(width: usize, height: usize, data: &[u8], method: u8) -> Vec<u8> {
    let mut out = vec![0u8; data.len()];
    for y in 0..height {
        for x in 0..width {
            let at = |out: &Vec<u8>, x: usize, y: usize| out[y * width + x] as i32;
            let pred = if (x == 0 && y == 0) || method == 0 {
                0
            } else if y == 0 {
                at(&out, x - 1, 0)
            } else if x == 0 {
                at(&out, 0, y - 1)
            } else {
                match method {
                    1 => at(&out, x - 1, y),
                    2 => at(&out, x, y - 1),
                    _ => (at(&out, x - 1, y) + at(&out, x, y - 1) - at(&out, x - 1, y - 1)).clamp(0, 255),
                }
            };
            out[y * width + x] = ((data[y * width + x] as i32 + pred) % 256) as u8;
        }
    }
    out
}

/// Reduces alpha to fewer evenly spaced levels (0 and 255 kept exactly).
#[cfg_attr(not(feature = "vp8l"), allow(dead_code))]
pub(crate) fn reduce_levels(alpha: &[u8], quality: u8) -> Vec<u8> {
    let levels = 2 + (quality.min(100) as u32 * 254) / 100;
    if levels >= 256 {
        return alpha.to_vec();
    }
    let step = 255.0 / (levels - 1) as f64;
    alpha
        .iter()
        .map(|&a| ((a as f64 / step).round() * step).round().clamp(0.0, 255.0) as u8)
        .collect()
}

#[cfg(feature = "vp8l")]
fn compressed(
    width: usize,
    height: usize,
    alpha: &[u8],
    quality: u8,
    method: u8,
) -> Result<Vec<u8>, crate::EncodeError> {
    let reduced;
    let (src, pre) = if quality < 100 {
        reduced = reduce_levels(alpha, quality);
        (&reduced[..], 1u8)
    } else {
        (alpha, 0u8)
    };
    let filters: &[u8] = if method >= 5 {
        &[0, 1, 2, 3]
    } else {
        &[pick_filter(width, height, src)]
    };
    let mut best: Option<Vec<u8>> = None;
    for &f in filters {
        let filtered = filter(width, height, src, f);
        let stream = crate::lossless::alpha_stream(width as u32, height as u32, &filtered)?;
        let mut out = Vec::with_capacity(stream.len() + 1);
        out.push((pre << 4) | (f << 2) | 1);
        out.extend_from_slice(&stream);
        if best.as_ref().map_or(true, |b| out.len() < b.len()) {
            best = Some(out);
        }
    }
    Ok(best.expect("at least one filter"))
}

/// The filter whose residuals have the smallest sum of magnitudes.
#[cfg(feature = "vp8l")]
fn pick_filter(width: usize, height: usize, alpha: &[u8]) -> u8 {
    let mut best = (u64::MAX, 0u8);
    for f in 0..4u8 {
        let r = filter(width, height, alpha, f);
        let cost: u64 = r.iter().map(|&v| (v as i8).unsigned_abs() as u64).sum();
        if cost < best.0 {
            best = (cost, f);
        }
    }
    best.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_invert() {
        let (w, h) = (13, 7);
        let alpha: Vec<u8> = (0..w * h).map(|i| ((i * 37) % 256) as u8).collect();
        for m in 0..4 {
            let f = filter(w, h, &alpha, m);
            assert_eq!(unfilter(w, h, &f, m), alpha, "method {m}");
        }
    }

    #[test]
    fn level_reduction_keeps_the_ends() {
        let a: Vec<u8> = (0..=255).collect();
        let r = reduce_levels(&a, 0);
        assert_eq!(r[0], 0);
        assert_eq!(r[255], 255);
        let mut distinct = r.clone();
        distinct.dedup();
        assert_eq!(distinct.len(), 2);
        assert_eq!(reduce_levels(&a, 100), a);
    }
}
