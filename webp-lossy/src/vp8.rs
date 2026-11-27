//! The VP8 key-frame encoder: one still picture in, one VP8 frame out.
//!
//! Every macroblock gets its modes by rate-distortion optimisation: the
//! four whole-block luma modes, the ten subblock modes for each 4x4 luma
//! block (B_PRED) and the four chroma modes are each predicted,
//! transformed, quantized, costed with the frame's token probabilities and
//! reconstructed exactly as a decoder will, and the cheapest
//! distortion-plus-rate wins. Optional extras by method: trellis
//! quantization, a second pass with the probabilities of the first, up to
//! four segments with their own quantizer and filter level, and a search
//! for the loop-filter levels that bring the filtered picture closest to
//! the source. The design is this crate's own; RFC 6386 specifies only
//! the decoder.

use crate::bool_encoder::BoolEncoder;
use crate::consts::*;
use crate::cost::{bit_cost, ModeCosts, TokenCosts};
use crate::idct::{idct_add, iwht4x4};
use crate::loop_filter::{filter_frame, MbFilter, PlanesMut};
use crate::predict::{
    fill_edges, predict_block, predict_subblock, spread_above_right, subblock_pos, CWS_SIZE, LWS, LWS_SIZE,
};
use crate::tables::{AC_QUANT, COEFF_UPDATE_PROBS, DEFAULT_COEFF_PROBS, KF_BMODE_PROBS};
use crate::tokens::{self, Counter, TokenSink, Writer};
use crate::transform::{fdct4x4, fwht4x4, quantize, Quant, MAX_LEVEL};

/// How strongly spatial noise shaping scales the step per unit of log
/// activity, at strength 100.
const SNS_SCALE: f64 = 0.6;
/// Lambda's factor of the squared step (SSE per bit).
const LAMBDA_SCALE: f64 = 0.06;

/// The largest first partition the 19-bit size field can describe.
const MAX_FIRST_PARTITION: usize = (1 << 19) - 1;

/// Macroblock-aligned planes: luma `aw` x `ah`, chroma half that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Planes {
    pub(crate) y: Vec<u8>,
    pub(crate) u: Vec<u8>,
    pub(crate) v: Vec<u8>,
    pub(crate) aw: usize,
    pub(crate) ah: usize,
}

impl Planes {
    /// Copies a picture into macroblock-aligned planes, repeating the last
    /// column and row into the padding.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_picture(
        width: usize,
        height: usize,
        y: &[u8],
        y_stride: usize,
        u: &[u8],
        v: &[u8],
        uv_stride: usize,
    ) -> Planes {
        let aw = width.div_ceil(16) * 16;
        let ah = height.div_ceil(16) * 16;
        let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
        let mut p = Planes {
            y: vec![0; aw * ah],
            u: vec![0; aw * ah / 4],
            v: vec![0; aw * ah / 4],
            aw,
            ah,
        };
        copy_padded(&mut p.y, aw, ah, y, y_stride, width, height);
        copy_padded(&mut p.u, aw / 2, ah / 2, u, uv_stride, cw, ch);
        copy_padded(&mut p.v, aw / 2, ah / 2, v, uv_stride, cw, ch);
        p
    }
}

fn copy_padded(dst: &mut [u8], dw: usize, dh: usize, src: &[u8], stride: usize, w: usize, h: usize) {
    for r in 0..dh {
        let s = &src[r.min(h - 1) * stride..r.min(h - 1) * stride + w];
        let d = &mut dst[r * dw..r * dw + dw];
        d[..w].copy_from_slice(s);
        let last = s[w - 1];
        d[w..].fill(last);
    }
}

/// Settings for one frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Params {
    /// The quantizer index of the frame (0 to 127).
    pub(crate) q: u8,
    /// Speed and effort, 0 (fastest) to 6.
    pub(crate) method: u8,
    /// Segments, 1 to 4.
    pub(crate) segments: u8,
    /// Spatial noise shaping, 0 to 100: how much busier segments get a
    /// coarser quantizer.
    pub(crate) sns: u8,
    /// A fixed loop-filter level (0 to 63) for every segment, or `None` to
    /// choose them.
    pub(crate) filter_level: Option<u8>,
    /// The simple loop filter instead of the normal one.
    pub(crate) simple_filter: bool,
    /// Loop-filter sharpness, 0 to 7.
    pub(crate) sharpness: u8,
}

/// The encoded frame, and the picture a decoder makes of it.
pub(crate) struct Encoded {
    pub(crate) data: Vec<u8>,
    pub(crate) recon: Planes,
}

/// Why a frame could not be written.
#[derive(Debug)]
pub(crate) enum FrameError {
    /// The first partition outgrew its 19-bit size field.
    FirstPartitionTooLarge,
}

#[derive(Clone)]
struct Mb {
    y_mode: u8,
    uv_mode: u8,
    bmodes: [u8; 16],
    segment: u8,
    skip: bool,
    /// 25 blocks of 16 levels (natural order): Y 0-15, U 16-19, V 20-23, Y2 24.
    levels: [i16; 400],
}

impl Default for Mb {
    fn default() -> Self {
        Mb {
            y_mode: DC_PRED,
            uv_mode: DC_PRED,
            bmodes: [B_DC_PRED; 16],
            segment: 0,
            skip: true,
            levels: [0; 400],
        }
    }
}

/// Rate-distortion cost: distortion (SSE) in 1/65536, plus lambda (SSE per
/// bit, in 1/256) times the rate (in 1/256 bits).
#[inline]
fn rd(dist: u32, rate: u32, lambda: u64) -> u64 {
    ((dist as u64) << 16) + lambda * rate as u64
}

#[inline]
fn sse(a: &[u8], b: &[u8]) -> u32 {
    a.iter()
        .zip(b)
        .map(|(&x, &y)| (x as i32 - y as i32).pow(2) as u32)
        .sum()
}

#[inline]
fn sad(a: &[u8], b: &[u8]) -> u32 {
    a.iter().zip(b).map(|(&x, &y)| x.abs_diff(y) as u32).sum()
}

/// One segment's quantizer and rate-distortion weights.
#[derive(Clone, Copy)]
struct SegmentQ {
    q: u8,
    quant: Quant,
    /// Lambda for integer costs (see [`rd`]).
    lambda: u64,
    /// The same, as SSE per 1/256 bit, for the trellis.
    lambda_f: f64,
}

impl SegmentQ {
    fn new(q: u8) -> Self {
        let quant = Quant::new(q);
        let step = AC_QUANT[q as usize] as f64;
        // SSE per bit. Pixel-domain step is about half the coefficient step.
        let lambda = LAMBDA_SCALE * step * step / 4.0;
        SegmentQ {
            q,
            quant,
            lambda: (lambda * 256.0).round().max(1.0) as u64,
            lambda_f: lambda / 256.0,
        }
    }
}

/// Neighbour state while coding a row of macroblocks.
struct Contexts {
    /// Token contexts above (per macroblock column) and to the left.
    nz_above: Vec<[u8; 9]>,
    nz_left: [u8; 9],
}

/// Whole-block luma coding results.
struct Luma16 {
    levels: [i16; 400],
    recon: [u8; 256],
    rate: u32,
    dist: u32,
    nz_above: [u8; 9],
    nz_left: [u8; 9],
}

/// Chroma coding results.
struct Chroma {
    levels: [i16; 128],
    recon: [[u8; 64]; 2],
    rate: u32,
    dist: u32,
    nz_above: [u8; 9],
    nz_left: [u8; 9],
}

struct Enc<'a> {
    src: &'a Planes,
    rec: Planes,
    mb_cols: usize,
    mb_rows: usize,
    segs: [SegmentQ; 4],
    seg_map: Vec<u8>,
    mbs: Vec<Mb>,
    method: u8,
    trellis_search: bool,
    trellis_final: bool,
}

/// Encodes one key frame.
pub(crate) fn encode_frame(src: &Planes, width: u32, height: u32, p: &Params) -> Result<Encoded, FrameError> {
    let mb_cols = src.aw / 16;
    let mb_rows = src.ah / 16;
    let method = p.method.min(6);
    let mut enc = Enc {
        src,
        rec: Planes {
            y: vec![0; src.y.len()],
            u: vec![0; src.u.len()],
            v: vec![0; src.v.len()],
            aw: src.aw,
            ah: src.ah,
        },
        mb_cols,
        mb_rows,
        segs: [SegmentQ::new(p.q); 4],
        seg_map: vec![0; mb_cols * mb_rows],
        mbs: vec![Mb::default(); mb_cols * mb_rows],
        method,
        trellis_search: method >= 5,
        trellis_final: method >= 3,
    };
    let segments = if p.segments > 1 && mb_cols * mb_rows >= 4 {
        enc.segment(p.segments as usize, p.q, p.sns)
    } else {
        1
    };
    let passes = match method {
        0..=3 => 1,
        4 | 5 => 2,
        _ => 3,
    };
    let mut probs = DEFAULT_COEFF_PROBS;
    let mut updates = [[[[false; 11]; 3]; 8]; 4];
    for _ in 0..passes {
        let costs = TokenCosts::new(&probs);
        enc.code_all(&costs);
        (probs, updates) = enc.fit_probabilities();
    }
    let levels = match p.filter_level {
        Some(l) => [l.min(63); 4],
        None if method <= 1 => std::array::from_fn(|s| heuristic_filter_level(&enc.segs[s])),
        None => enc.search_filter_levels(segments, p.simple_filter, p.sharpness),
    };
    let data = enc.write(width, height, segments, &probs, &updates, &levels, p)?;
    let mut recon = enc.rec.clone();
    enc.filter_into(&mut recon, &levels, p.simple_filter, p.sharpness);
    Ok(Encoded { data, recon })
}

/// A loop-filter level from the quantizer alone (the fast methods).
fn heuristic_filter_level(s: &SegmentQ) -> u8 {
    ((s.q as u32 * 3) / 8).min(63) as u8
}

impl Enc<'_> {
    /// Splits the macroblocks into up to `n` segments by how busy their luma
    /// is, and gives busier segments a coarser quantizer by `sns`. Returns
    /// the number of segments used.
    fn segment(&mut self, n: usize, q: u8, sns: u8) -> usize {
        let aw = self.src.aw;
        let count = self.mb_cols * self.mb_rows;
        let mut act = vec![0f64; count];
        for mby in 0..self.mb_rows {
            for mbx in 0..self.mb_cols {
                let mut sum = 0u32;
                for b in 0..16 {
                    let x = mbx * 16 + (b & 3) * 4;
                    let y = mby * 16 + (b >> 2) * 4;
                    let mut px = [0u8; 16];
                    for r in 0..4 {
                        px[r * 4..r * 4 + 4].copy_from_slice(&self.src.y[(y + r) * aw + x..(y + r) * aw + x + 4]);
                    }
                    let mean = (px.iter().map(|&v| v as u32).sum::<u32>() + 8) / 16;
                    sum += px.iter().map(|&v| (v as i32 - mean as i32).unsigned_abs()).sum::<u32>();
                }
                act[mby * self.mb_cols + mbx] = (1.0 + sum as f64).ln();
            }
        }
        let mut sorted = act.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let mut centers: Vec<f64> = (0..n).map(|i| sorted[((2 * i + 1) * count) / (2 * n)]).collect();
        centers.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        if centers.len() < 2 {
            return 1;
        }
        let k = centers.len();
        let mut assign = vec![0u8; count];
        for _ in 0..12 {
            let mut sums = vec![0f64; k];
            let mut ns = vec![0usize; k];
            for (i, &a) in act.iter().enumerate() {
                let mut best = 0;
                for c in 1..k {
                    if (a - centers[c]).abs() < (a - centers[best]).abs() {
                        best = c;
                    }
                }
                assign[i] = best as u8;
                sums[best] += a;
                ns[best] += 1;
            }
            for c in 0..k {
                if ns[c] > 0 {
                    centers[c] = sums[c] / ns[c] as f64;
                }
            }
        }
        let mean = act.iter().sum::<f64>() / count as f64;
        // Each segment's step is the frame's scaled by 2^(strength x how much
        // busier it is than the mean), then rounded to the nearest index.
        let base = AC_QUANT[q as usize] as f64;
        for (s, &c) in centers.iter().enumerate() {
            let target = base * (SNS_SCALE * sns as f64 / 100.0 * (c - mean)).exp2();
            let sq = (0..128u8)
                .min_by(|&a, &b| {
                    let da = (AC_QUANT[a as usize] as f64 - target).abs();
                    let db = (AC_QUANT[b as usize] as f64 - target).abs();
                    da.total_cmp(&db)
                })
                .expect("indices");
            self.segs[s] = SegmentQ::new(sq);
        }
        self.seg_map = assign;
        for (m, &s) in self.mbs.iter_mut().zip(&self.seg_map) {
            m.segment = s;
        }
        k
    }

    /// Codes every macroblock (mode decisions, quantization and the
    /// unfiltered reconstruction) with the given token costs.
    fn code_all(&mut self, costs: &TokenCosts) {
        let mut ctx = Contexts {
            nz_above: vec![[0u8; 9]; self.mb_cols],
            nz_left: [0; 9],
        };
        for mby in 0..self.mb_rows {
            ctx.nz_left = [0; 9];
            for mbx in 0..self.mb_cols {
                self.code_mb(mbx, mby, costs, &mut ctx);
            }
        }
    }

    fn bmode_context(&self, mbx: usize, mby: usize) -> ([u8; 4], [u8; 4]) {
        let above = if mby > 0 {
            let m = &self.mbs[(mby - 1) * self.mb_cols + mbx];
            [m.bmodes[12], m.bmodes[13], m.bmodes[14], m.bmodes[15]]
        } else {
            [B_DC_PRED; 4]
        };
        let left = if mbx > 0 {
            let m = &self.mbs[mby * self.mb_cols + mbx - 1];
            [m.bmodes[3], m.bmodes[7], m.bmodes[11], m.bmodes[15]]
        } else {
            [B_DC_PRED; 4]
        };
        (above, left)
    }

    fn code_mb(&mut self, mbx: usize, mby: usize, costs: &TokenCosts, ctx: &mut Contexts) {
        let k = mby * self.mb_cols + mbx;
        let seg = self.segs[self.seg_map[k] as usize];
        let mc = ModeCosts::get();
        let aw = self.src.aw;
        let caw = aw / 2;
        let top = mby == 0;
        let left = mbx == 0;
        let mut s16 = [0u8; 256];
        for r in 0..16 {
            let o = (mby * 16 + r) * aw + mbx * 16;
            s16[r * 16..r * 16 + 16].copy_from_slice(&self.src.y[o..o + 16]);
        }
        let mut ws = [0u8; LWS_SIZE];
        fill_edges(&mut ws, 16, &self.rec.y, aw, mbx, mby, self.mb_cols);
        let above_nz = ctx.nz_above[mbx];
        let left_nz = ctx.nz_left;

        // Whole-block luma modes.
        let mut best16: Option<(u64, u8, Luma16)> = None;
        let mut preds = [[0u8; 256]; 4];
        for mode in [DC_PRED, V_PRED, H_PRED, TM_PRED] {
            predict_block(&ws, 16, mode, top, left, &mut preds[mode as usize]);
        }
        if self.method == 0 {
            let mut best = (u64::MAX, DC_PRED);
            for mode in [DC_PRED, V_PRED, H_PRED, TM_PRED] {
                let j = rd(
                    sad(&s16, &preds[mode as usize]) * 16,
                    mc.ymode[mode as usize],
                    seg.lambda,
                );
                if j < best.0 {
                    best = (j, mode);
                }
            }
            let l = self.luma16(
                &s16,
                &preds[best.1 as usize],
                &seg,
                &above_nz,
                &left_nz,
                costs,
                self.trellis_final,
            );
            best16 = Some((rd(l.dist, l.rate + mc.ymode[best.1 as usize], seg.lambda), best.1, l));
        } else {
            for mode in [DC_PRED, V_PRED, H_PRED, TM_PRED] {
                let l = self.luma16(
                    &s16,
                    &preds[mode as usize],
                    &seg,
                    &above_nz,
                    &left_nz,
                    costs,
                    self.trellis_search,
                );
                let j = rd(l.dist, l.rate + mc.ymode[mode as usize], seg.lambda);
                if best16.as_ref().map_or(true, |b| j < b.0) {
                    best16 = Some((j, mode, l));
                }
            }
            if self.trellis_final && !self.trellis_search {
                let (_, mode, _) = best16.as_ref().expect("a mode");
                let mode = *mode;
                let l = self.luma16(&s16, &preds[mode as usize], &seg, &above_nz, &left_nz, costs, true);
                best16 = Some((rd(l.dist, l.rate + mc.ymode[mode as usize], seg.lambda), mode, l));
            }
        }
        let (j16, mode16, l16) = best16.expect("a mode");

        // Subblock modes.
        let mut use_b = None;
        if self.method >= 1 {
            let (bm_above, bm_left) = self.bmode_context(mbx, mby);
            let mut w = ws;
            spread_above_right(&mut w);
            let mut modes = [B_DC_PRED; 16];
            let mut levels = [0i16; 400];
            let mut a_nz = above_nz;
            let mut l_nz = left_nz;
            let mut total = rd(0, mc.ymode[B_PRED as usize], seg.lambda);
            let mut ok = true;
            for b in 0..16 {
                let pos = subblock_pos(b);
                let am = if b < 4 { bm_above[b] } else { modes[b - 4] };
                let lm = if b & 3 == 0 { bm_left[b >> 2] } else { modes[b - 1] };
                let mcost = &mc.bmode[am as usize][lm as usize];
                let tctx = (a_nz[b & 3] + l_nz[b >> 2]) as usize;
                let mut src4 = [0u8; 16];
                for r in 0..4 {
                    src4[r * 4..r * 4 + 4].copy_from_slice(&s16[((b >> 2) * 4 + r) * 16 + (b & 3) * 4..][..4]);
                }
                let mut best: Option<(u64, u8, Sub)> = None;
                if self.method == 1 {
                    let mut pick = (u64::MAX, B_DC_PRED);
                    for mode in 0..NUM_BMODES as u8 {
                        let mut pred = [0u8; 16];
                        predict_subblock(&w, pos, mode, &mut pred);
                        let j = rd(sad(&src4, &pred) * 16, mcost[mode as usize], seg.lambda);
                        if j < pick.0 {
                            pick = (j, mode);
                        }
                    }
                    let mut pred = [0u8; 16];
                    predict_subblock(&w, pos, pick.1, &mut pred);
                    let s = code_sub(&src4, &pred, &seg, tctx, costs, self.trellis_final);
                    best = Some((rd(s.dist, s.rate + mcost[pick.1 as usize], seg.lambda), pick.1, s));
                } else {
                    // Full rate-distortion on the modes whose prediction
                    // error and mode cost look best (all ten at method 5
                    // and up).
                    let keep = match self.method {
                        2 => 3,
                        3 => 4,
                        4 => 5,
                        _ => NUM_BMODES,
                    };
                    let mut preds = [[0u8; 16]; NUM_BMODES];
                    let mut order: [(u64, u8); NUM_BMODES] = [(0, 0); NUM_BMODES];
                    for mode in 0..NUM_BMODES {
                        predict_subblock(&w, pos, mode as u8, &mut preds[mode]);
                        order[mode] = (rd(sse(&src4, &preds[mode]), mcost[mode], seg.lambda), mode as u8);
                    }
                    if keep < NUM_BMODES {
                        order.sort_unstable();
                    }
                    for &(_, mode) in &order[..keep] {
                        let pred = preds[mode as usize];
                        let s = code_sub(&src4, &pred, &seg, tctx, costs, self.trellis_search);
                        let j = rd(s.dist, s.rate + mcost[mode as usize], seg.lambda);
                        if best.as_ref().map_or(true, |x| j < x.0) {
                            best = Some((j, mode, s));
                        }
                    }
                    if self.trellis_final && !self.trellis_search {
                        let mode = best.as_ref().expect("a mode").1;
                        let mut pred = [0u8; 16];
                        predict_subblock(&w, pos, mode, &mut pred);
                        let s = code_sub(&src4, &pred, &seg, tctx, costs, true);
                        best = Some((rd(s.dist, s.rate + mcost[mode as usize], seg.lambda), mode, s));
                    }
                }
                let (j, mode, s) = best.expect("a mode");
                total += j;
                if total >= j16 {
                    ok = false;
                    break;
                }
                modes[b] = mode;
                levels[b * 16..b * 16 + 16].copy_from_slice(&s.levels);
                let nz = s.levels.iter().any(|&v| v != 0) as u8;
                a_nz[b & 3] = nz;
                l_nz[b >> 2] = nz;
                for r in 0..4 {
                    w[pos + r * LWS..pos + r * LWS + 4].copy_from_slice(&s.recon[r * 4..r * 4 + 4]);
                }
            }
            if ok {
                use_b = Some((modes, levels, w, a_nz, l_nz));
            }
        }

        let mut mb = Mb {
            segment: self.seg_map[k],
            ..Mb::default()
        };
        let (mut nz_above, mut nz_left);
        if let Some((modes, levels, w, a_nz, l_nz)) = use_b {
            mb.y_mode = B_PRED;
            mb.bmodes = modes;
            mb.levels = levels;
            for r in 0..16 {
                let o = (mby * 16 + r) * aw + mbx * 16;
                self.rec.y[o..o + 16].copy_from_slice(&w[(r + 1) * LWS + 1..(r + 1) * LWS + 17]);
            }
            nz_above = a_nz;
            nz_left = l_nz;
        } else {
            mb.y_mode = mode16;
            mb.bmodes = [implied_bmode(mode16); 16];
            mb.levels = l16.levels;
            for r in 0..16 {
                let o = (mby * 16 + r) * aw + mbx * 16;
                self.rec.y[o..o + 16].copy_from_slice(&l16.recon[r * 16..r * 16 + 16]);
            }
            nz_above = l16.nz_above;
            nz_left = l16.nz_left;
        }

        // Chroma.
        let mut cws = [[0u8; CWS_SIZE]; 2];
        fill_edges(&mut cws[0], 8, &self.rec.u, caw, mbx, mby, self.mb_cols);
        fill_edges(&mut cws[1], 8, &self.rec.v, caw, mbx, mby, self.mb_cols);
        let mut src_c = [[0u8; 64]; 2];
        for (p, plane) in [&self.src.u, &self.src.v].into_iter().enumerate() {
            for r in 0..8 {
                let o = (mby * 8 + r) * caw + mbx * 8;
                src_c[p][r * 8..r * 8 + 8].copy_from_slice(&plane[o..o + 8]);
            }
        }
        let mut best_c: Option<(u64, u8, Chroma)> = None;
        let mut cpreds = [[[0u8; 64]; 2]; 4];
        for mode in [DC_PRED, V_PRED, H_PRED, TM_PRED] {
            for p in 0..2 {
                predict_block(&cws[p], 8, mode, top, left, &mut cpreds[mode as usize][p]);
            }
        }
        if self.method <= 1 {
            let mut pick = (u64::MAX, DC_PRED);
            for mode in [DC_PRED, V_PRED, H_PRED, TM_PRED] {
                let d = sad(&src_c[0], &cpreds[mode as usize][0]) + sad(&src_c[1], &cpreds[mode as usize][1]);
                let j = rd(d * 16, mc.uv_mode[mode as usize], seg.lambda);
                if j < pick.0 {
                    pick = (j, mode);
                }
            }
            let c = code_chroma(
                &src_c,
                &cpreds[pick.1 as usize],
                &seg,
                &nz_above,
                &nz_left,
                costs,
                self.trellis_final,
            );
            best_c = Some((0, pick.1, c));
        } else {
            for mode in [DC_PRED, V_PRED, H_PRED, TM_PRED] {
                let c = code_chroma(
                    &src_c,
                    &cpreds[mode as usize],
                    &seg,
                    &nz_above,
                    &nz_left,
                    costs,
                    self.trellis_search,
                );
                let j = rd(c.dist, c.rate + mc.uv_mode[mode as usize], seg.lambda);
                if best_c.as_ref().map_or(true, |b| j < b.0) {
                    best_c = Some((j, mode, c));
                }
            }
            if self.trellis_final && !self.trellis_search {
                let mode = best_c.as_ref().expect("a mode").1;
                let c = code_chroma(&src_c, &cpreds[mode as usize], &seg, &nz_above, &nz_left, costs, true);
                best_c = Some((0, mode, c));
            }
        }
        let (_, uv_mode, c) = best_c.expect("a mode");
        mb.uv_mode = uv_mode;
        mb.levels[256..384].copy_from_slice(&c.levels);
        for (p, plane) in [&mut self.rec.u, &mut self.rec.v].into_iter().enumerate() {
            for r in 0..8 {
                let o = (mby * 8 + r) * caw + mbx * 8;
                plane[o..o + 8].copy_from_slice(&c.recon[p][r * 8..r * 8 + 8]);
            }
        }
        nz_above[4..8].copy_from_slice(&c.nz_above[4..8]);
        nz_left[4..8].copy_from_slice(&c.nz_left[4..8]);
        mb.skip = mb.levels.iter().all(|&v| v == 0);
        if mb.skip {
            // A skipped macroblock leaves zero contexts (and keeps the Y2
            // ones when it has no Y2 block), as decoders do.
            tokens::skip_mb(mb.y_mode != B_PRED, &mut nz_above, &mut nz_left);
        }
        ctx.nz_above[mbx] = nz_above;
        ctx.nz_left = nz_left;
        self.mbs[k] = mb;
    }

    /// Codes the 16x16 luma block with a Y2 block, from prediction `pred`.
    #[allow(clippy::too_many_arguments)]
    fn luma16(
        &self,
        src: &[u8; 256],
        pred: &[u8; 256],
        seg: &SegmentQ,
        above: &[u8; 9],
        left: &[u8; 9],
        costs: &TokenCosts,
        trellis: bool,
    ) -> Luma16 {
        let q = &seg.quant;
        let mut coeffs = [[0i32; 16]; 16];
        let mut dcs = [0i32; 16];
        for b in 0..16 {
            let mut res = [0i32; 16];
            for r in 0..4 {
                for c in 0..4 {
                    let i = ((b >> 2) * 4 + r) * 16 + (b & 3) * 4 + c;
                    res[r * 4 + c] = src[i] as i32 - pred[i] as i32;
                }
            }
            fdct4x4(&res, &mut coeffs[b]);
            dcs[b] = coeffs[b][0];
        }
        let mut y2 = [0i32; 16];
        fwht4x4(&dcs, &mut y2);
        let mut out = Luma16 {
            levels: [0; 400],
            recon: *pred,
            rate: 0,
            dist: 0,
            nz_above: *above,
            nz_left: *left,
        };
        let y2ctx = (above[8] + left[8]) as usize;
        let mut y2l = [0i16; 16];
        quantize_block(
            &y2,
            0,
            q.y2,
            128,
            PLANE_Y2,
            y2ctx,
            costs,
            seg.lambda_f,
            1.0 / 16.0,
            trellis,
            &mut y2l,
        );
        out.rate += costs.block(PLANE_Y2, y2ctx, 0, &y2l);
        let y2nz = y2l.iter().any(|&v| v != 0) as u8;
        out.nz_above[8] = y2nz;
        out.nz_left[8] = y2nz;
        out.levels[384..400].copy_from_slice(&y2l);
        let y2deq: [i16; 16] = std::array::from_fn(|i| (y2l[i] as i32 * q.y2[(i > 0) as usize]) as i16);
        let dc = if y2nz != 0 { iwht4x4(&y2deq) } else { [0; 16] };
        for b in 0..16 {
            let ctx = (out.nz_above[b & 3] + out.nz_left[b >> 2]) as usize;
            let mut lv = [0i16; 16];
            quantize_block(
                &coeffs[b],
                1,
                q.y1,
                112,
                PLANE_Y_AFTER_Y2,
                ctx,
                costs,
                seg.lambda_f,
                0.25,
                trellis,
                &mut lv,
            );
            out.rate += costs.block(PLANE_Y_AFTER_Y2, ctx, 1, &lv);
            let nz = lv[1..].iter().any(|&v| v != 0) as u8;
            out.nz_above[b & 3] = nz;
            out.nz_left[b >> 2] = nz;
            let mut deq = [0i16; 16];
            deq[0] = dc[b];
            for i in 1..16 {
                deq[i] = (lv[i] as i32 * q.y1[1]) as i16;
            }
            if deq.iter().any(|&v| v != 0) {
                idct_add(&deq, &mut out.recon, ((b >> 2) * 4) * 16 + (b & 3) * 4, 16);
            }
            out.levels[b * 16..b * 16 + 16].copy_from_slice(&lv);
        }
        out.dist = sse(src, &out.recon);
        out
    }

    /// Re-derives the best probabilities from the coded levels: each
    /// probability is updated when the bits saved pay for the update.
    #[allow(clippy::type_complexity)]
    fn fit_probabilities(&self) -> (CoeffProbs, [[[[bool; 11]; 3]; 8]; 4]) {
        let mut counter = Counter::new();
        self.walk_tokens(&mut counter);
        let mut probs = DEFAULT_COEFF_PROBS;
        let mut updates = [[[[false; 11]; 3]; 8]; 4];
        for i in 0..4 {
            for j in 0..8 {
                for k in 0..3 {
                    for l in 0..11 {
                        let [n0, n1] = counter.counts[i][j][k][l];
                        let total = n0 + n1;
                        if total == 0 {
                            continue;
                        }
                        let new = ((n0 as u64 * 256 + total as u64 / 2) / total as u64).clamp(1, 255) as u8;
                        let old = probs[i][j][k][l];
                        let cost = |p: u8| n0 as u64 * bit_cost(p, false) as u64 + n1 as u64 * bit_cost(p, true) as u64;
                        let up = COEFF_UPDATE_PROBS[i][j][k][l];
                        let overhead = 8 * 256 + bit_cost(up, true) as u64 - bit_cost(up, false) as u64;
                        if cost(new) + overhead < cost(old) {
                            probs[i][j][k][l] = new;
                            updates[i][j][k][l] = true;
                        }
                    }
                }
            }
        }
        (probs, updates)
    }

    /// Runs every macroblock's tokens through `sink` with the decoder's contexts.
    fn walk_tokens<S: TokenSink>(&self, sink: &mut S) {
        let mut above = vec![[0u8; 9]; self.mb_cols];
        for row in 0..self.mb_rows {
            let mut left = [0u8; 9];
            for (col, above) in above.iter_mut().enumerate() {
                let mb = &self.mbs[row * self.mb_cols + col];
                let has_y2 = mb.y_mode != B_PRED;
                if mb.skip {
                    tokens::skip_mb(has_y2, above, &mut left);
                } else {
                    tokens::write_mb(sink, &mb.levels, has_y2, above, &mut left);
                }
            }
        }
    }

    /// Per-macroblock filter settings for segment levels `levels`.
    fn mb_filters(&self, levels: &[u8; 4]) -> Vec<MbFilter> {
        self.mbs
            .iter()
            .map(|m| MbFilter {
                level: levels[m.segment as usize],
                inner: m.y_mode == B_PRED || !m.skip,
            })
            .collect()
    }

    fn filter_into(&self, planes: &mut Planes, levels: &[u8; 4], simple: bool, sharpness: u8) {
        let mbs = self.mb_filters(levels);
        let stride = planes.aw;
        filter_frame(
            PlanesMut {
                y: &mut planes.y,
                u: &mut planes.u,
                v: &mut planes.v,
                stride,
            },
            self.mb_cols,
            self.mb_rows,
            &mbs,
            simple,
            sharpness,
        );
    }

    /// The squared error of the filtered picture per segment.
    fn segment_sse(&self, levels: &[u8; 4], simple: bool, sharpness: u8, work: &mut Planes) -> [u64; 4] {
        work.y.copy_from_slice(&self.rec.y);
        work.u.copy_from_slice(&self.rec.u);
        work.v.copy_from_slice(&self.rec.v);
        self.filter_into(work, levels, simple, sharpness);
        let aw = self.src.aw;
        let caw = aw / 2;
        let mut out = [0u64; 4];
        for mby in 0..self.mb_rows {
            for mbx in 0..self.mb_cols {
                let s = self.seg_map[mby * self.mb_cols + mbx] as usize;
                let mut e = 0u64;
                for r in 0..16 {
                    let o = (mby * 16 + r) * aw + mbx * 16;
                    e += sse(&self.src.y[o..o + 16], &work.y[o..o + 16]) as u64;
                }
                for r in 0..8 {
                    let o = (mby * 8 + r) * caw + mbx * 8;
                    e += sse(&self.src.u[o..o + 8], &work.u[o..o + 8]) as u64;
                    e += sse(&self.src.v[o..o + 8], &work.v[o..o + 8]) as u64;
                }
                out[s] += e;
            }
        }
        out
    }

    /// Searches each segment's loop-filter level for the least squared
    /// error after filtering.
    fn search_filter_levels(&self, segments: usize, simple: bool, sharpness: u8) -> [u8; 4] {
        let mut work = self.rec.clone();
        let mut best: [u8; 4] = std::array::from_fn(|s| heuristic_filter_level(&self.segs[s]));
        let mut best_err = self.segment_sse(&best, simple, sharpness, &mut work);
        let steps: &[i32] = if self.method >= 6 {
            &[16, 8, 4, 2, 1]
        } else {
            &[16, 8, 4, 2]
        };
        for &step in steps {
            for dir in [-1, 1] {
                let trial: [u8; 4] = std::array::from_fn(|s| (best[s] as i32 + dir * step).clamp(0, 63) as u8);
                let err = self.segment_sse(&trial, simple, sharpness, &mut work);
                for s in 0..segments {
                    if err[s] < best_err[s] {
                        best[s] = trial[s];
                        best_err[s] = err[s];
                    }
                }
            }
        }
        best
    }

    /// Writes the frame: frame tag, key-frame header, first partition and
    /// one token partition.
    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        width: u32,
        height: u32,
        segments: usize,
        probs: &CoeffProbs,
        updates: &[[[[bool; 11]; 3]; 8]; 4],
        levels: &[u8; 4],
        p: &Params,
    ) -> Result<Vec<u8>, FrameError> {
        let n = self.mbs.len() as u64;
        let prob_of = |zeros: u64, total: u64| -> u8 {
            match (zeros * 256 + total / 2).checked_div(total) {
                Some(p) => p.clamp(1, 255) as u8,
                None => 255,
            }
        };
        let n_skip = self.mbs.iter().filter(|m| m.skip).count() as u64;
        let use_skip = n_skip > 0;
        let prob_skip = prob_of(n - n_skip, n);
        let mut seg_counts = [0u64; 4];
        for m in &self.mbs {
            seg_counts[m.segment as usize] += 1;
        }
        let seg_probs = [
            prob_of(seg_counts[0] + seg_counts[1], n),
            prob_of(seg_counts[0], seg_counts[0] + seg_counts[1]),
            prob_of(seg_counts[2], seg_counts[2] + seg_counts[3]),
        ];

        let mut h = BoolEncoder::with_capacity(self.mbs.len() * 2 + 1024);
        h.write_bit(false); // colour space: YUV
        h.write_bit(false); // decoders clamp
        h.write_bit(segments > 1);
        if segments > 1 {
            h.write_bit(true); // update the segment map
            h.write_bit(true); // update the segment data
            h.write_bit(true); // absolute values
            for s in 0..4 {
                h.write_bit(true);
                h.write_signed(self.segs[s].q as i32, 7);
            }
            for &level in levels {
                h.write_bit(true);
                h.write_signed(level as i32, 6);
            }
            for &sp in &seg_probs {
                let send = sp != 255;
                h.write_bit(send);
                if send {
                    h.write_literal(sp as u32, 8);
                }
            }
        }
        h.write_bit(p.simple_filter);
        // Decoders skip the loop filter for the whole frame when the
        // frame-level level is 0 (RFC 6386 Section 15), so with segments it
        // must be non-zero whenever any segment filters; each segment's own
        // level then applies.
        let frame_level = if segments > 1 {
            levels[..segments].iter().copied().max().unwrap_or(0)
        } else {
            levels[0]
        };
        h.write_literal(frame_level as u32, 6);
        h.write_literal(p.sharpness.min(7) as u32, 3);
        h.write_bit(false); // no mode or reference filter deltas
        h.write_literal(0, 2); // one token partition
        h.write_literal(self.segs[0].q as u32, 7);
        for _ in 0..5 {
            h.write_bit(false); // no quantizer deltas
        }
        h.write_bit(false); // refresh_entropy_probs
        for i in 0..4 {
            for j in 0..8 {
                for k in 0..3 {
                    for l in 0..11 {
                        let up = updates[i][j][k][l];
                        h.write(COEFF_UPDATE_PROBS[i][j][k][l], up);
                        if up {
                            h.write_literal(probs[i][j][k][l] as u32, 8);
                        }
                    }
                }
            }
        }
        h.write_bit(use_skip);
        if use_skip {
            h.write_literal(prob_skip as u32, 8);
        }
        for mby in 0..self.mb_rows {
            for mbx in 0..self.mb_cols {
                let mb = &self.mbs[mby * self.mb_cols + mbx];
                if segments > 1 {
                    h.write_tree(&SEGMENT_TREE, &seg_probs, mb.segment);
                }
                if use_skip {
                    h.write(prob_skip, mb.skip);
                }
                h.write_tree(&KF_YMODE_TREE, &KF_YMODE_PROBS, mb.y_mode);
                if mb.y_mode == B_PRED {
                    let (above, left) = self.bmode_context(mbx, mby);
                    for b in 0..16 {
                        let a = if b < 4 { above[b] } else { mb.bmodes[b - 4] };
                        let l = if b & 3 == 0 { left[b >> 2] } else { mb.bmodes[b - 1] };
                        h.write_tree(&BMODE_TREE, &KF_BMODE_PROBS[a as usize][l as usize], mb.bmodes[b]);
                    }
                }
                h.write_tree(&UV_MODE_TREE, &KF_UV_MODE_PROBS, mb.uv_mode);
            }
            if h.len() > MAX_FIRST_PARTITION {
                return Err(FrameError::FirstPartitionTooLarge);
            }
        }
        let mut part1 = h.finish();
        // One more zero byte: incremental decoders (libwebp's incremental
        // decoder, and Chromium, which uses it) read one byte further into
        // the first partition than the flushed bits need, and refuse a
        // partition that ends there. Found by testing single-macroblock
        // pictures against `dwebp -incremental` and Chromium.
        part1.push(0);
        if part1.len() > MAX_FIRST_PARTITION {
            return Err(FrameError::FirstPartitionTooLarge);
        }
        let mut t = BoolEncoder::with_capacity(self.mbs.len() * 16);
        {
            let mut w = Writer { enc: &mut t, probs };
            self.walk_tokens(&mut w);
        }
        let part2 = t.finish();
        let mut out = Vec::with_capacity(10 + part1.len() + part2.len());
        // Key frame (bit 0 clear), version 0, shown, first partition size.
        let tag = (1u32 << 4) | ((part1.len() as u32) << 5);
        out.extend_from_slice(&tag.to_le_bytes()[..3]);
        out.extend_from_slice(&[0x9d, 0x01, 0x2a]);
        out.extend_from_slice(&(width as u16).to_le_bytes());
        out.extend_from_slice(&(height as u16).to_le_bytes());
        out.extend_from_slice(&part1);
        out.extend_from_slice(&part2);
        Ok(out)
    }
}

/// A coded 4x4 luma subblock (no Y2).
struct Sub {
    levels: [i16; 16],
    recon: [u8; 16],
    rate: u32,
    dist: u32,
}

fn code_sub(src: &[u8; 16], pred: &[u8; 16], seg: &SegmentQ, ctx: usize, costs: &TokenCosts, trellis: bool) -> Sub {
    let res: [i32; 16] = std::array::from_fn(|i| src[i] as i32 - pred[i] as i32);
    let mut co = [0i32; 16];
    fdct4x4(&res, &mut co);
    let q = seg.quant.y1;
    let mut lv = [0i16; 16];
    quantize_block(
        &co,
        0,
        q,
        112,
        PLANE_Y_NO_Y2,
        ctx,
        costs,
        seg.lambda_f,
        0.25,
        trellis,
        &mut lv,
    );
    let rate = costs.block(PLANE_Y_NO_Y2, ctx, 0, &lv);
    let mut recon = *pred;
    if lv.iter().any(|&v| v != 0) {
        let deq: [i16; 16] = std::array::from_fn(|i| (lv[i] as i32 * q[(i > 0) as usize]) as i16);
        idct_add(&deq, &mut recon, 0, 4);
    }
    Sub {
        levels: lv,
        dist: sse(src, &recon),
        recon,
        rate,
    }
}

#[allow(clippy::too_many_arguments)]
fn code_chroma(
    src: &[[u8; 64]; 2],
    pred: &[[u8; 64]; 2],
    seg: &SegmentQ,
    above: &[u8; 9],
    left: &[u8; 9],
    costs: &TokenCosts,
    trellis: bool,
) -> Chroma {
    let q = seg.quant.uv;
    let mut out = Chroma {
        levels: [0; 128],
        recon: *pred,
        rate: 0,
        dist: 0,
        nz_above: *above,
        nz_left: *left,
    };
    for p in 0..2 {
        let base = 4 + 2 * p;
        for j in 0..4 {
            let off = (j >> 1) * 4 * 8 + (j & 1) * 4;
            let mut res = [0i32; 16];
            for r in 0..4 {
                for c in 0..4 {
                    res[r * 4 + c] = src[p][off + r * 8 + c] as i32 - pred[p][off + r * 8 + c] as i32;
                }
            }
            let mut co = [0i32; 16];
            fdct4x4(&res, &mut co);
            let ai = base + (j & 1);
            let li = base + (j >> 1);
            let ctx = (out.nz_above[ai] + out.nz_left[li]) as usize;
            let mut lv = [0i16; 16];
            quantize_block(
                &co,
                0,
                q,
                112,
                PLANE_UV,
                ctx,
                costs,
                seg.lambda_f,
                0.25,
                trellis,
                &mut lv,
            );
            out.rate += costs.block(PLANE_UV, ctx, 0, &lv);
            let nz = lv.iter().any(|&v| v != 0) as u8;
            out.nz_above[ai] = nz;
            out.nz_left[li] = nz;
            if nz != 0 {
                let deq: [i16; 16] = std::array::from_fn(|i| (lv[i] as i32 * q[(i > 0) as usize]) as i16);
                idct_add(&deq, &mut out.recon[p], off, 8);
            }
            let b = p * 4 + j;
            out.levels[b * 16..b * 16 + 16].copy_from_slice(&lv);
        }
        out.dist += sse(&src[p], &out.recon[p]);
    }
    out
}

/// Quantizes one block's coefficients (raster order) from position
/// `first`: plainly, with rounding offset `ac_bias / 256` (the DC rounds to
/// nearest), or by trellis search for the least distortion plus
/// `lambda` x rate, where `dist_scale` converts squared coefficient error
/// into squared pixel error.
#[allow(clippy::too_many_arguments)]
fn quantize_block(
    co: &[i32; 16],
    first: usize,
    q: [i32; 2],
    ac_bias: i32,
    plane: usize,
    ctx: usize,
    costs: &TokenCosts,
    lambda: f64,
    dist_scale: f64,
    trellis: bool,
    out: &mut [i16; 16],
) {
    *out = [0; 16];
    if !trellis {
        for (i, o) in out.iter_mut().enumerate().skip(first) {
            let (step, bias) = if i == 0 { (q[0], 128) } else { (q[1], ac_bias) };
            *o = quantize(co[i], step, bias) as i16;
        }
        return;
    }
    // Scan order.
    let step = |n: usize| if n == 0 { q[0] } else { q[1] };
    let mut tail = [0f64; 17];
    for n in (first..16).rev() {
        let c = co[ZIGZAG[n]] as f64;
        tail[n] = tail[n + 1] + c * c * dist_scale;
    }
    // State: the context entering position n (0, 1, 2); at n > first,
    // context 0 means the previous coefficient was zero.
    const INF: f64 = f64::INFINITY;
    let mut cur = [INF; 3];
    cur[ctx] = 0.0;
    let mut back = [[(0u16, 0u8); 3]; 16];
    let mut best = (tail[first] + lambda * costs.eob(plane, first, ctx) as f64, first, ctx);
    for n in first..16 {
        let s_step = step(n);
        let c = co[ZIGZAG[n]];
        let a = c.unsigned_abs() as i32;
        let base = quantize(a, s_step, 0);
        let mut cands = [base, base + 1, 0];
        let ncand = if base == 0 {
            2
        } else if base <= 2 {
            3
        } else {
            2
        };
        for v in cands.iter_mut() {
            *v = (*v).min(MAX_LEVEL);
        }
        let mut next = [INF; 3];
        for (s, &here) in cur.iter().enumerate() {
            if here == INF {
                continue;
            }
            let after_zero = n > first && s == 0;
            if !after_zero && n > first {
                let total = here + lambda * costs.eob(plane, n, s) as f64 + tail[n];
                if total < best.0 {
                    best = (total, n, s);
                }
            }
            for &l in &cands[..ncand] {
                let e = (a - l * s_step) as f64;
                let r = costs.level(plane, n, s, after_zero, l as u32) as f64;
                let total = here + lambda * r + e * e * dist_scale;
                let ns = (l as usize).min(2);
                if total < next[ns] {
                    next[ns] = total;
                    back[n][ns] = (l as u16, s as u8);
                }
            }
        }
        cur = next;
    }
    for (s, &c) in cur.iter().enumerate().skip(1) {
        if c < best.0 {
            best = (c, 16, s);
        }
    }
    let (_, end, mut s) = best;
    for n in (first..end).rev() {
        let (l, ps) = back[n][s];
        let raster = ZIGZAG[n];
        out[raster] = if co[raster] < 0 { -(l as i16) } else { l as i16 };
        s = ps as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bool_encoder::test_decoder::BoolDecoder;

    fn test_picture(w: usize, h: usize) -> Planes {
        let mut y = vec![0u8; w * h];
        for r in 0..h {
            for c in 0..w {
                y[r * w + c] = ((r * 7 + c * 3) % 256) as u8 ^ ((c / 9) as u8 * 31);
            }
        }
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let u = (0..cw * ch).map(|i| (i % 200) as u8).collect::<Vec<_>>();
        let v = (0..cw * ch).map(|i| (255 - i % 180) as u8).collect::<Vec<_>>();
        Planes::from_picture(w, h, &y, w, &u, &v, cw)
    }

    fn params(method: u8) -> Params {
        Params {
            q: 40,
            method,
            segments: 4,
            sns: 50,
            filter_level: None,
            simple_filter: false,
            sharpness: 0,
        }
    }

    #[test]
    fn header_fields_read_back() {
        let pic = test_picture(37, 21);
        let e = encode_frame(&pic, 37, 21, &params(4)).expect("encodes");
        let d = &e.data;
        assert_eq!(d[0] & 1, 0, "key frame");
        assert_eq!((d[0] >> 4) & 1, 1, "shown");
        assert_eq!(&d[3..6], &[0x9d, 0x01, 0x2a]);
        assert_eq!(u16::from_le_bytes([d[6], d[7]]), 37);
        assert_eq!(u16::from_le_bytes([d[8], d[9]]), 21);
        let size = (u32::from_le_bytes([d[0], d[1], d[2], 0]) >> 5) as usize;
        let mut b = BoolDecoder::new(&d[10..10 + size]);
        assert_eq!(b.read_literal(2), 0);
        assert!(b.read(128), "segmentation on");
    }

    #[test]
    fn every_method_encodes() {
        let pic = test_picture(64, 48);
        for m in 0..=6 {
            let e = encode_frame(&pic, 64, 48, &params(m)).expect("encodes");
            let err: u64 = pic
                .y
                .iter()
                .zip(&e.recon.y)
                .map(|(&a, &b)| (a as i64 - b as i64).pow(2) as u64)
                .sum();
            let mse = err as f64 / pic.y.len() as f64;
            assert!(mse < 60.0, "method {m}: luma MSE {mse}");
        }
    }

    #[test]
    fn trellis_never_costs_more_than_plain() {
        let costs = TokenCosts::new(&DEFAULT_COEFF_PROBS);
        let seg = SegmentQ::new(60);
        let mut seed = 3u32;
        for _ in 0..500 {
            let mut co = [0i32; 16];
            for c in co.iter_mut() {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                *c = (seed % 400) as i32 - 200;
            }
            let cost = |lv: &[i16; 16]| {
                let mut d = 0f64;
                for i in 0..16 {
                    let step = seg.quant.y1[(i > 0) as usize];
                    let e = (co[i] - lv[i] as i32 * step) as f64;
                    d += e * e * 0.25;
                }
                d + seg.lambda_f * costs.block(PLANE_Y_NO_Y2, 1, 0, lv) as f64
            };
            let mut plain = [0i16; 16];
            let mut tr = [0i16; 16];
            quantize_block(
                &co,
                0,
                seg.quant.y1,
                112,
                PLANE_Y_NO_Y2,
                1,
                &costs,
                seg.lambda_f,
                0.25,
                false,
                &mut plain,
            );
            quantize_block(
                &co,
                0,
                seg.quant.y1,
                112,
                PLANE_Y_NO_Y2,
                1,
                &costs,
                seg.lambda_f,
                0.25,
                true,
                &mut tr,
            );
            assert!(cost(&tr) <= cost(&plain) + 1e-6, "{tr:?} vs {plain:?}");
        }
    }
}
