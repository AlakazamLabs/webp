//! RGB to Y'CbCr 4:2:0, with the Rec. 601 matrix in studio range (what
//! WebP decoders assume, RFC 9649 Section 2.5): luma 16 to 235, chroma
//! 16 to 240 around 128. Chroma is the mean of each 2x2 block (edge pixels
//! repeat for odd sizes).

// The matrix rows scaled by 2^16.
const YR: i32 = 16829;
const YG: i32 = 33039;
const YB: i32 = 6416;
const UR: i32 = -9714;
const UG: i32 = -19070;
const UB: i32 = 28784;
const VR: i32 = 28784;
const VG: i32 = -24103;
const VB: i32 = -4681;

/// Planes of a 4:2:0 picture, tightly packed (chroma stride is half the
/// width, rounded up).
pub(crate) struct Yuv420 {
    pub(crate) y: Vec<u8>,
    pub(crate) u: Vec<u8>,
    pub(crate) v: Vec<u8>,
}

/// Converts packed 8-bit RGB (`bpp` = 3) or RGBA (`bpp` = 4) to 4:2:0.
/// `hide` (RGBA only): pixels with zero alpha are invisible, so their
/// colour is replaced with values that are cheaper to code.
pub(crate) fn rgb_to_yuv420(width: usize, height: usize, data: &[u8], bpp: usize, hide: bool) -> Yuv420 {
    let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
    let mut out = Yuv420 {
        y: vec![0; width * height],
        u: vec![0; cw * ch],
        v: vec![0; cw * ch],
    };
    for r in 0..height {
        for c in 0..width {
            let p = &data[(r * width + c) * bpp..];
            let (rr, gg, bb) = (p[0] as i32, p[1] as i32, p[2] as i32);
            out.y[r * width + c] = ((YR * rr + YG * gg + YB * bb + (16 << 16) + (1 << 15)) >> 16) as u8;
        }
    }
    for r in 0..ch {
        for c in 0..cw {
            let (mut sr, mut sg, mut sb) = (0i32, 0i32, 0i32);
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let y = (2 * r + dy).min(height - 1);
                let x = (2 * c + dx).min(width - 1);
                let p = &data[(y * width + x) * bpp..];
                sr += p[0] as i32;
                sg += p[1] as i32;
                sb += p[2] as i32;
            }
            let off = (128 << 18) + (1 << 17);
            out.u[r * cw + c] = ((UR * sr + UG * sg + UB * sb + off) >> 18).clamp(0, 255) as u8;
            out.v[r * cw + c] = ((VR * sr + VG * sg + VB * sb + off) >> 18).clamp(0, 255) as u8;
        }
    }
    if hide && bpp == 4 {
        hide_transparent(width, height, data, &mut out);
    }
    out
}

/// Replaces the colour under fully transparent pixels: in each 16x16 block
/// (8x8 for chroma), with the mean of the block's visible samples, or, in a
/// block with none, by repeating the samples above it (or to its left).
fn hide_transparent(width: usize, height: usize, rgba: &[u8], yuv: &mut Yuv420) {
    let alpha = |x: usize, y: usize| rgba[(y.min(height - 1) * width + x.min(width - 1)) * 4 + 3];
    fill_hidden(&mut yuv.y, width, height, 16, |x, y| alpha(x, y) == 0);
    let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
    let hidden_c = |x: usize, y: usize| {
        alpha(2 * x, 2 * y) == 0
            && alpha(2 * x + 1, 2 * y) == 0
            && alpha(2 * x, 2 * y + 1) == 0
            && alpha(2 * x + 1, 2 * y + 1) == 0
    };
    fill_hidden(&mut yuv.u, cw, ch, 8, hidden_c);
    fill_hidden(&mut yuv.v, cw, ch, 8, hidden_c);
}

fn fill_hidden(plane: &mut [u8], w: usize, h: usize, n: usize, hidden: impl Fn(usize, usize) -> bool) {
    for by in (0..h).step_by(n) {
        for bx in (0..w).step_by(n) {
            let (x1, y1) = ((bx + n).min(w), (by + n).min(h));
            let mut sum = 0u32;
            let mut count = 0u32;
            let mut any_hidden = false;
            for y in by..y1 {
                for x in bx..x1 {
                    if hidden(x, y) {
                        any_hidden = true;
                    } else {
                        sum += plane[y * w + x] as u32;
                        count += 1;
                    }
                }
            }
            if !any_hidden {
                continue;
            }
            if let Some(mean) = (sum + count / 2).checked_div(count) {
                let mean = mean as u8;
                for y in by..y1 {
                    for x in bx..x1 {
                        if hidden(x, y) {
                            plane[y * w + x] = mean;
                        }
                    }
                }
            } else {
                for y in by..y1 {
                    for x in bx..x1 {
                        plane[y * w + x] = if by > 0 {
                            plane[(by - 1) * w + x]
                        } else if bx > 0 {
                            plane[y * w + bx - 1]
                        } else {
                            128
                        };
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_range_extremes() {
        let px = [0u8, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0, 255];
        let out = rgb_to_yuv420(4, 1, &px, 3, false);
        assert_eq!(out.y[0], 16);
        assert_eq!(out.y[1], 235);
        // Grey has neutral chroma.
        let grey = [77u8; 12];
        let g = rgb_to_yuv420(2, 2, &grey, 3, false);
        assert_eq!((g.u[0], g.v[0]), (128, 128));
    }

    #[test]
    fn hidden_pixels_take_the_visible_mean() {
        let mut rgba = vec![0u8; 16 * 16 * 4];
        for (i, p) in rgba.chunks_mut(4).enumerate() {
            if i % 2 == 0 {
                p.copy_from_slice(&[200, 200, 200, 255]);
            } else {
                p.copy_from_slice(&[0, 255, 0, 0]);
            }
        }
        let out = rgb_to_yuv420(16, 16, &rgba, 4, true);
        let first = out.y[0];
        assert!(out.y.iter().all(|&v| v == first));
    }
}
