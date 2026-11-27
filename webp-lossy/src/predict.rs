//! Intra prediction (RFC 6386 Section 12), written from the RFC's prose and
//! its inline description of each mode.
//!
//! Predictions read their edge pixels from a small workspace around the
//! block: row 0 holds the pixel above and to the left (P), then the row
//! above (A), then, for luma, the four pixels above and to the right;
//! column 0 of the rows below holds the column to the left (L). The block
//! itself sits at row 1, column 1. Pixels outside the frame take the RFC's
//! values: 127 above the top row (corner included), 129 left of the left
//! column.

use crate::consts::*;

/// Luma workspace stride: P, 16 above, 4 above-right.
pub(crate) const LWS: usize = 21;
/// Luma workspace size: the edge row and 16 rows.
pub(crate) const LWS_SIZE: usize = LWS * 17;
/// Chroma workspace stride: P and 8 above.
pub(crate) const CWS: usize = 9;
/// Chroma workspace size.
pub(crate) const CWS_SIZE: usize = CWS * 9;

/// Fills the edges of a workspace for the `n` x `n` block of macroblock
/// (`mbx`, `mby`) from the unfiltered reconstruction `rec` (stride
/// `stride`). Luma (`n` = 16) also gets the above-right pixels: from the
/// macroblock above and to the right, or, for the rightmost macroblock, the
/// last pixel of the row above repeated (Section 12.3); 127 on the top row.
pub(crate) fn fill_edges(ws: &mut [u8], n: usize, rec: &[u8], stride: usize, mbx: usize, mby: usize, mb_cols: usize) {
    let ws_stride = n + 1 + if n == 16 { 4 } else { 0 };
    let x = mbx * n;
    let y = mby * n;
    if mby == 0 {
        ws[..ws_stride].fill(127);
    } else {
        let above = &rec[(y - 1) * stride..];
        ws[0] = if mbx == 0 { 129 } else { above[x - 1] };
        ws[1..=n].copy_from_slice(&above[x..x + n]);
        if n == 16 {
            if mbx + 1 < mb_cols {
                ws[17..21].copy_from_slice(&above[x + 16..x + 20]);
            } else {
                let last = above[x + 15];
                ws[17..21].fill(last);
            }
        }
    }
    for r in 0..n {
        ws[(r + 1) * ws_stride] = if mbx == 0 { 129 } else { rec[(y + r) * stride + x - 1] };
    }
}

/// Predicts the whole `n` x `n` block (16 luma or 8 chroma) with a
/// whole-block mode, into `out` (stride `n`). `top` and `left`: the block is
/// on the frame's top row or left column, which changes DC_PRED only.
pub(crate) fn predict_block(ws: &[u8], n: usize, mode: u8, top: bool, left: bool, out: &mut [u8]) {
    let s = n + 1 + if n == 16 { 4 } else { 0 };
    let a = &ws[1..=n];
    let l = |r: usize| ws[(r + 1) * s];
    match mode {
        DC_PRED => {
            let shift = if n == 16 { 3 } else { 2 };
            let v = match (top, left) {
                (true, true) => 128,
                (true, false) => {
                    let sum: u32 = (0..n).map(|r| l(r) as u32).sum();
                    (sum + (1 << shift)) >> (shift + 1)
                }
                (false, true) => {
                    let sum: u32 = a.iter().map(|&p| p as u32).sum();
                    (sum + (1 << shift)) >> (shift + 1)
                }
                (false, false) => {
                    let sum: u32 = a.iter().map(|&p| p as u32).sum::<u32>() + (0..n).map(|r| l(r) as u32).sum::<u32>();
                    (sum + (1 << (shift + 1))) >> (shift + 2)
                }
            };
            out[..n * n].fill(v as u8);
        }
        V_PRED => {
            for r in 0..n {
                out[r * n..r * n + n].copy_from_slice(a);
            }
        }
        H_PRED => {
            for r in 0..n {
                out[r * n..r * n + n].fill(l(r));
            }
        }
        _ => {
            // TM_PRED: L[r] + A[c] - P, clamped.
            let p = ws[0] as i32;
            for r in 0..n {
                let lr = l(r) as i32 - p;
                for c in 0..n {
                    out[r * n + c] = (lr + a[c] as i32).clamp(0, 255) as u8;
                }
            }
        }
    }
}

#[inline]
fn avg3(x: u8, y: u8, z: u8) -> u8 {
    ((x as u32 + 2 * y as u32 + z as u32 + 2) >> 2) as u8
}

#[inline]
fn avg2(x: u8, y: u8) -> u8 {
    ((x as u32 + y as u32 + 1) >> 1) as u8
}

/// Predicts the 4x4 luma subblock whose top-left pixel is at workspace
/// offset `pos` (stride [`LWS`]) with subblock mode `mode`, into `out`
/// (row-major 4x4). The workspace holds the reconstruction of the earlier
/// subblocks and the macroblock's edges; the right-column subblocks below
/// the first row must see the macroblock's above-right pixels in their
/// above-right positions (see [`spread_above_right`]).
pub(crate) fn predict_subblock(ws: &[u8], pos: usize, mode: u8, out: &mut [u8; 16]) {
    let s = LWS;
    // A[-1..8]: P, the 4 pixels above and the 4 above-right.
    let mut a = [0u8; 9];
    a.copy_from_slice(&ws[pos - s - 1..pos - s + 8]);
    let p = a[0];
    let above = |i: usize| a[i + 1];
    let l = [ws[pos - 1], ws[pos + s - 1], ws[pos + 2 * s - 1], ws[pos + 3 * s - 1]];
    // The 9 edge pixels from bottom-left to top-right: L[3..0], P, A[0..4].
    let e = [l[3], l[2], l[1], l[0], p, above(0), above(1), above(2), above(3)];
    let mut b = [[0u8; 4]; 4];
    match mode {
        B_DC_PRED => {
            let sum: u32 = (0..4).map(|i| above(i) as u32 + l[i] as u32).sum();
            let v = ((sum + 4) >> 3) as u8;
            b = [[v; 4]; 4];
        }
        B_TM_PRED => {
            for (r, row) in b.iter_mut().enumerate() {
                for (c, px) in row.iter_mut().enumerate() {
                    *px = (l[r] as i32 + above(c) as i32 - p as i32).clamp(0, 255) as u8;
                }
            }
        }
        B_VE_PRED => {
            for c in 0..4 {
                let left_of = if c == 0 { p } else { above(c - 1) };
                let v = avg3(left_of, above(c), above(c + 1));
                for row in b.iter_mut() {
                    row[c] = v;
                }
            }
        }
        B_HE_PRED => {
            let v = [
                avg3(p, l[0], l[1]),
                avg3(l[0], l[1], l[2]),
                avg3(l[1], l[2], l[3]),
                avg3(l[2], l[3], l[3]),
            ];
            for (r, row) in b.iter_mut().enumerate() {
                *row = [v[r]; 4];
            }
        }
        B_LD_PRED => {
            // Down-left diagonals: pixel (r, c) lies on diagonal r + c,
            // predicted from the smoothed above row at column r + c + 1.
            for (r, row) in b.iter_mut().enumerate() {
                for (c, px) in row.iter_mut().enumerate() {
                    let d = r + c;
                    *px = if d == 6 {
                        avg3(above(6), above(7), above(7))
                    } else {
                        avg3(above(d), above(d + 1), above(d + 2))
                    };
                }
            }
        }
        B_RD_PRED => {
            // Down-right diagonals: (r, c) on diagonal c - r, from the edge
            // pixel e[4 + c - r] smoothed with its neighbours.
            for (r, row) in b.iter_mut().enumerate() {
                for (c, px) in row.iter_mut().enumerate() {
                    let i = 4 + c - r;
                    *px = avg3(e[i - 1], e[i], e[i + 1]);
                }
            }
        }
        B_VR_PRED => {
            b[3][0] = avg3(e[1], e[2], e[3]);
            b[2][0] = avg3(e[2], e[3], e[4]);
            b[3][1] = avg3(e[3], e[4], e[5]);
            b[1][0] = b[3][1];
            b[2][1] = avg2(e[4], e[5]);
            b[0][0] = b[2][1];
            b[3][2] = avg3(e[4], e[5], e[6]);
            b[1][1] = b[3][2];
            b[2][2] = avg2(e[5], e[6]);
            b[0][1] = b[2][2];
            b[3][3] = avg3(e[5], e[6], e[7]);
            b[1][2] = b[3][3];
            b[2][3] = avg2(e[6], e[7]);
            b[0][2] = b[2][3];
            b[1][3] = avg3(e[6], e[7], e[8]);
            b[0][3] = avg2(e[7], e[8]);
        }
        B_VL_PRED => {
            let ab = |i: usize| above(i);
            b[0][0] = avg2(ab(0), ab(1));
            b[1][0] = avg3(ab(0), ab(1), ab(2));
            b[2][0] = avg2(ab(1), ab(2));
            b[0][1] = b[2][0];
            b[1][1] = avg3(ab(1), ab(2), ab(3));
            b[3][0] = b[1][1];
            b[2][1] = avg2(ab(2), ab(3));
            b[0][2] = b[2][1];
            b[3][1] = avg3(ab(2), ab(3), ab(4));
            b[1][2] = b[3][1];
            b[2][2] = avg2(ab(3), ab(4));
            b[0][3] = b[2][2];
            b[3][2] = avg3(ab(3), ab(4), ab(5));
            b[1][3] = b[3][2];
            b[2][3] = avg3(ab(4), ab(5), ab(6));
            b[3][3] = avg3(ab(5), ab(6), ab(7));
        }
        B_HD_PRED => {
            b[3][0] = avg2(e[0], e[1]);
            b[3][1] = avg3(e[0], e[1], e[2]);
            b[2][0] = avg2(e[1], e[2]);
            b[3][2] = b[2][0];
            b[2][1] = avg3(e[1], e[2], e[3]);
            b[3][3] = b[2][1];
            b[2][2] = avg2(e[2], e[3]);
            b[1][0] = b[2][2];
            b[2][3] = avg3(e[2], e[3], e[4]);
            b[1][1] = b[2][3];
            b[1][2] = avg2(e[3], e[4]);
            b[0][0] = b[1][2];
            b[1][3] = avg3(e[3], e[4], e[5]);
            b[0][1] = b[1][3];
            b[0][2] = avg3(e[4], e[5], e[6]);
            b[0][3] = avg3(e[5], e[6], e[7]);
        }
        _ => {
            // B_HU_PRED.
            b[0][0] = avg2(l[0], l[1]);
            b[0][1] = avg3(l[0], l[1], l[2]);
            b[0][2] = avg2(l[1], l[2]);
            b[1][0] = b[0][2];
            b[0][3] = avg3(l[1], l[2], l[3]);
            b[1][1] = b[0][3];
            b[1][2] = avg2(l[2], l[3]);
            b[2][0] = b[1][2];
            b[1][3] = avg3(l[2], l[3], l[3]);
            b[2][1] = b[1][3];
            b[2][2] = l[3];
            b[2][3] = l[3];
            b[3] = [l[3]; 4];
        }
    }
    for r in 0..4 {
        out[r * 4..r * 4 + 4].copy_from_slice(&b[r]);
    }
}

/// Copies the macroblock's above-right pixels (workspace row 0, columns
/// 17 to 20) to the above-right positions of subblocks 7, 11 and 15, which
/// use them too (Section 12.3).
pub(crate) fn spread_above_right(ws: &mut [u8]) {
    let (top, rest) = ws.split_at_mut(LWS);
    for k in 1..4 {
        let row = (4 * k - 1) * LWS + 17;
        rest[row..row + 4].copy_from_slice(&top[17..21]);
    }
}

/// Workspace offset of the top-left pixel of luma subblock `b` (0 to 15).
#[inline]
pub(crate) fn subblock_pos(b: usize) -> usize {
    (1 + (b >> 2) * 4) * LWS + 1 + (b & 3) * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_rounding_follows_the_rfc() {
        // 16 pixels of 1 and 16 of 2: (48 + 16) >> 5 = 2.
        let mut ws = [0u8; LWS_SIZE];
        ws[1..17].fill(1);
        for r in 0..16 {
            ws[(r + 1) * LWS] = 2;
        }
        let mut out = [0u8; 256];
        predict_block(&ws, 16, DC_PRED, false, false, &mut out);
        assert!(out.iter().all(|&v| v == 2));
        predict_block(&ws, 16, DC_PRED, true, false, &mut out);
        assert!(out.iter().all(|&v| v == 2));
        predict_block(&ws, 16, DC_PRED, false, true, &mut out);
        assert!(out.iter().all(|&v| v == 1));
        predict_block(&ws, 16, DC_PRED, true, true, &mut out);
        assert!(out.iter().all(|&v| v == 128));
    }

    #[test]
    fn edges_outside_the_frame() {
        let rec = vec![50u8; 32 * 32];
        let mut ws = [0u8; LWS_SIZE];
        fill_edges(&mut ws, 16, &rec, 32, 0, 0, 2);
        assert!(ws[..LWS].iter().all(|&v| v == 127));
        assert!((1..17).all(|r| ws[r * LWS] == 129));
        fill_edges(&mut ws, 16, &rec, 32, 0, 1, 2);
        assert_eq!(ws[0], 129);
        assert!(ws[1..21].iter().all(|&v| v == 50));
        // TM on the left column with the 129 corner reduces to the row above.
        let mut out = [0u8; 256];
        predict_block(&ws, 16, TM_PRED, false, true, &mut out);
        assert!(out.iter().all(|&v| v == 50));
    }
}
