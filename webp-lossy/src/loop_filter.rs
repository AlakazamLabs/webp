//! The loop filter (RFC 6386 Section 15), written from the RFC's prose and
//! its description of the simple, subblock and macroblock filters and of
//! the control parameters.
//!
//! The encoder runs it to choose filter levels and to produce the picture a
//! decoder will show (a still image is a single key frame, so the filter
//! affects only the displayed result).

/// Signed clamp to -128..=127 (the RFC's `c`).
#[inline(always)]
fn c8(v: i32) -> i32 {
    v.clamp(-128, 127)
}

#[inline(always)]
fn u2s(v: u8) -> i32 {
    v as i32 - 128
}

#[inline(always)]
fn s2u(v: i32) -> u8 {
    (c8(v) + 128) as u8
}

/// The filter parameters of one macroblock.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EdgeParams {
    mb_limit: i32,
    sub_limit: i32,
    interior: i32,
    hev: i32,
}

impl EdgeParams {
    /// Parameters for `level` (1 to 63) and `sharpness` (0 to 7) on a key
    /// frame (Section 15.4).
    pub(crate) fn new(level: u8, sharpness: u8) -> Self {
        let level = level as i32;
        let sharpness = sharpness as i32;
        let mut interior = level;
        if sharpness > 0 {
            interior >>= if sharpness > 4 { 2 } else { 1 };
            interior = interior.min(9 - sharpness);
        }
        interior = interior.max(1);
        let hev = if level >= 40 {
            2
        } else if level >= 15 {
            1
        } else {
            0
        };
        EdgeParams {
            mb_limit: (level + 2) * 2 + interior,
            sub_limit: level * 2 + interior,
            interior,
            hev,
        }
    }
}

/// The shared adjustment of the two pixels at the edge; returns the amount
/// taken from q0 (`common_adjust`).
#[inline(always)]
fn common_adjust(outer_taps: bool, p1: i32, p0: &mut i32, q0: &mut i32, q1: i32) -> i32 {
    let a = c8(if outer_taps { c8(p1 - q1) } else { 0 } + 3 * (*q0 - *p0));
    let b = c8(a + 3) >> 3;
    let a = c8(a + 4) >> 3;
    *q0 = c8(*q0 - a);
    *p0 = c8(*p0 + b);
    a
}

/// Filters the `len` segments across one edge. `pos` is the first pixel
/// after the edge (q0) of the first segment, `across` the step from p0 to
/// q0, `along` the step to the next segment.
#[allow(clippy::too_many_arguments)]
fn normal_edge(buf: &mut [u8], pos: usize, across: usize, along: usize, len: usize, p: &EdgeParams, mb_edge: bool) {
    let edge_limit = if mb_edge { p.mb_limit } else { p.sub_limit };
    for i in 0..len {
        let q = pos + i * along;
        let px = |k: usize| q - (k + 1) * across;
        let qx = |k: usize| q + k * across;
        let (p3, p2, p1, p0) = (
            buf[px(3)] as i32,
            buf[px(2)] as i32,
            buf[px(1)] as i32,
            buf[px(0)] as i32,
        );
        let (q0, q1, q2, q3) = (
            buf[qx(0)] as i32,
            buf[qx(1)] as i32,
            buf[qx(2)] as i32,
            buf[qx(3)] as i32,
        );
        let lim = p.interior;
        let pass = (p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 <= edge_limit
            && (p3 - p2).abs() <= lim
            && (p2 - p1).abs() <= lim
            && (p1 - p0).abs() <= lim
            && (q3 - q2).abs() <= lim
            && (q2 - q1).abs() <= lim
            && (q1 - q0).abs() <= lim;
        if !pass {
            continue;
        }
        let hev = (p1 - p0).abs() > p.hev || (q1 - q0).abs() > p.hev;
        let (sp2, mut sp1, mut sp0) = (u2s(buf[px(2)]), u2s(buf[px(1)]), u2s(buf[px(0)]));
        let (mut sq0, mut sq1, sq2) = (u2s(buf[qx(0)]), u2s(buf[qx(1)]), u2s(buf[qx(2)]));
        if mb_edge {
            if hev {
                common_adjust(true, sp1, &mut sp0, &mut sq0, sq1);
                buf[px(0)] = s2u(sp0);
                buf[qx(0)] = s2u(sq0);
            } else {
                let w = c8(c8(sp1 - sq1) + 3 * (sq0 - sp0));
                let a = c8((27 * w + 63) >> 7);
                buf[qx(0)] = s2u(sq0 - a);
                buf[px(0)] = s2u(sp0 + a);
                let a = c8((18 * w + 63) >> 7);
                buf[qx(1)] = s2u(sq1 - a);
                buf[px(1)] = s2u(sp1 + a);
                let a = c8((9 * w + 63) >> 7);
                buf[qx(2)] = s2u(sq2 - a);
                buf[px(2)] = s2u(sp2 + a);
            }
        } else {
            let a = (common_adjust(hev, sp1, &mut sp0, &mut sq0, sq1) + 1) >> 1;
            buf[px(0)] = s2u(sp0);
            buf[qx(0)] = s2u(sq0);
            if !hev {
                sq1 -= a;
                sp1 += a;
                buf[qx(1)] = s2u(sq1);
                buf[px(1)] = s2u(sp1);
            }
        }
    }
}

/// The simple filter across one luma edge.
fn simple_edge(buf: &mut [u8], pos: usize, across: usize, along: usize, len: usize, edge_limit: i32) {
    for i in 0..len {
        let q = pos + i * along;
        let (p1, p0, q0, q1) = (buf[q - 2 * across], buf[q - across], buf[q], buf[q + across]);
        if (p0 as i32 - q0 as i32).abs() * 2 + (p1 as i32 - q1 as i32).abs() / 2 <= edge_limit {
            let (mut sp0, mut sq0) = (u2s(p0), u2s(q0));
            common_adjust(true, u2s(p1), &mut sp0, &mut sq0, u2s(q1));
            buf[q - across] = s2u(sp0);
            buf[q] = s2u(sq0);
        }
    }
}

/// One macroblock's filter settings.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MbFilter {
    /// Filter level, 0 (off) to 63.
    pub(crate) level: u8,
    /// Whether the edges inside the macroblock are filtered (it is B_PRED,
    /// or it has coefficients).
    pub(crate) inner: bool,
}

/// The picture's three planes, macroblock-aligned.
pub(crate) struct PlanesMut<'a> {
    pub(crate) y: &'a mut [u8],
    pub(crate) u: &'a mut [u8],
    pub(crate) v: &'a mut [u8],
    /// Luma stride (16 x macroblock columns); chroma's is half.
    pub(crate) stride: usize,
}

/// Filters the whole frame in macroblock raster order (Section 15.1).
pub(crate) fn filter_frame(
    planes: PlanesMut<'_>,
    mb_cols: usize,
    mb_rows: usize,
    mbs: &[MbFilter],
    simple: bool,
    sharpness: u8,
) {
    let ys = planes.stride;
    let cs = ys / 2;
    let mut cache: [Option<EdgeParams>; 64] = [None; 64];
    for mby in 0..mb_rows {
        for mbx in 0..mb_cols {
            let mb = mbs[mby * mb_cols + mbx];
            if mb.level == 0 {
                continue;
            }
            let p = *cache[mb.level as usize].get_or_insert_with(|| EdgeParams::new(mb.level, sharpness));
            let y0 = mby * 16 * ys + mbx * 16;
            let c0 = mby * 8 * cs + mbx * 8;
            if simple {
                if mbx > 0 {
                    simple_edge(planes.y, y0, 1, ys, 16, p.mb_limit);
                }
                if mb.inner {
                    for x in [4, 8, 12] {
                        simple_edge(planes.y, y0 + x, 1, ys, 16, p.sub_limit);
                    }
                }
                if mby > 0 {
                    simple_edge(planes.y, y0, ys, 1, 16, p.mb_limit);
                }
                if mb.inner {
                    for y in [4, 8, 12] {
                        simple_edge(planes.y, y0 + y * ys, ys, 1, 16, p.sub_limit);
                    }
                }
                continue;
            }
            if mbx > 0 {
                normal_edge(planes.y, y0, 1, ys, 16, &p, true);
                normal_edge(planes.u, c0, 1, cs, 8, &p, true);
                normal_edge(planes.v, c0, 1, cs, 8, &p, true);
            }
            if mb.inner {
                for x in [4, 8, 12] {
                    normal_edge(planes.y, y0 + x, 1, ys, 16, &p, false);
                }
                normal_edge(planes.u, c0 + 4, 1, cs, 8, &p, false);
                normal_edge(planes.v, c0 + 4, 1, cs, 8, &p, false);
            }
            if mby > 0 {
                normal_edge(planes.y, y0, ys, 1, 16, &p, true);
                normal_edge(planes.u, c0, cs, 1, 8, &p, true);
                normal_edge(planes.v, c0, cs, 1, 8, &p, true);
            }
            if mb.inner {
                for y in [4, 8, 12] {
                    normal_edge(planes.y, y0 + y * ys, ys, 1, 16, &p, false);
                }
                normal_edge(planes.u, c0 + 4 * cs, cs, 1, 8, &p, false);
                normal_edge(planes.v, c0 + 4 * cs, cs, 1, 8, &p, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_parameters() {
        let p = EdgeParams::new(20, 0);
        assert_eq!((p.interior, p.mb_limit, p.sub_limit, p.hev), (20, 64, 60, 1));
        let p = EdgeParams::new(20, 5);
        assert_eq!(p.interior, 4);
        let p = EdgeParams::new(1, 7);
        assert_eq!(p.interior, 1);
    }

    #[test]
    fn a_small_step_is_smoothed_and_a_large_one_kept() {
        // 32x16 luma, one macroblock edge at x = 16.
        let stride = 32;
        let mut y = vec![0u8; stride * 16];
        for r in 0..16 {
            for c in 0..32 {
                y[r * stride + c] = if c < 16 { 100 } else { 108 };
            }
        }
        let mut u = vec![128u8; 16 * 8];
        let mut v = vec![128u8; 16 * 8];
        let mbs = [MbFilter {
            level: 20,
            inner: false,
        }; 2];
        filter_frame(
            PlanesMut {
                y: &mut y,
                u: &mut u,
                v: &mut v,
                stride,
            },
            2,
            1,
            &mbs,
            false,
            0,
        );
        assert!(y[15] > 100 && y[16] < 108, "{} {}", y[15], y[16]);
        let mut y2 = vec![0u8; stride * 16];
        for r in 0..16 {
            for c in 0..32 {
                y2[r * stride + c] = if c < 16 { 0 } else { 200 };
            }
        }
        filter_frame(
            PlanesMut {
                y: &mut y2,
                u: &mut u,
                v: &mut v,
                stride,
            },
            2,
            1,
            &mbs,
            false,
            0,
        );
        assert_eq!((y2[15], y2[16]), (0, 200));
    }
}
