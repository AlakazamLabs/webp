//! Coefficient tokens (RFC 6386 Section 13): the exact inverse of the
//! decoder's token reader, used once to count branch statistics and once
//! to write.

use crate::bool_encoder::BoolEncoder;
use crate::consts::*;

/// Where token bits go.
pub(crate) trait TokenSink {
    /// A bit coded with the probability at `[plane][band][ctx][node]`.
    fn node(&mut self, plane: usize, band: usize, ctx: usize, node: usize, bit: bool);
    /// A bit with a fixed probability (extra bits, sign).
    fn fixed(&mut self, prob: u8, bit: bool);
}

/// Zeros and ones seen per coefficient probability.
pub(crate) type Counts = [[[[[u32; 2]; 11]; 3]; 8]; 4];

/// Counts zeros and ones per probability, for choosing updates.
pub(crate) struct Counter {
    pub(crate) counts: Box<Counts>,
}

impl Counter {
    pub(crate) fn new() -> Self {
        Counter {
            counts: Box::new([[[[[0; 2]; 11]; 3]; 8]; 4]),
        }
    }
}

impl TokenSink for Counter {
    #[inline]
    fn node(&mut self, plane: usize, band: usize, ctx: usize, node: usize, bit: bool) {
        self.counts[plane][band][ctx][node][bit as usize] += 1;
    }
    #[inline]
    fn fixed(&mut self, _prob: u8, _bit: bool) {}
}

/// Writes token bits with the frame's probabilities.
pub(crate) struct Writer<'a> {
    pub(crate) enc: &'a mut BoolEncoder,
    pub(crate) probs: &'a CoeffProbs,
}

impl TokenSink for Writer<'_> {
    #[inline]
    fn node(&mut self, plane: usize, band: usize, ctx: usize, node: usize, bit: bool) {
        self.enc.write(self.probs[plane][band][ctx][node], bit);
    }
    #[inline]
    fn fixed(&mut self, prob: u8, bit: bool) {
        self.enc.write(prob, bit);
    }
}

fn extra<S: TokenSink>(sink: &mut S, probs: &[u8], v: i32) {
    let n = probs.len();
    for (i, &p) in probs.iter().enumerate() {
        sink.fixed(p, (v >> (n - 1 - i)) & 1 == 1);
    }
}

/// Codes one block's levels (natural order). `eob` is one past the last
/// non-zero coefficient in zigzag order, at least `first`.
pub(crate) fn write_block<S: TokenSink>(
    sink: &mut S,
    plane: usize,
    ctx: usize,
    first: usize,
    levels: &[i16],
    eob: usize,
) {
    let mut c = first;
    let mut ctx = ctx;
    if eob <= first {
        sink.node(plane, COEFF_BANDS[c], ctx, 0, false);
        return;
    }
    let mut check_eob = true;
    loop {
        let band = COEFF_BANDS[c];
        if check_eob {
            sink.node(plane, band, ctx, 0, true);
        }
        let v = levels[ZIGZAG[c]] as i32;
        if v == 0 {
            sink.node(plane, band, ctx, 1, false);
            c += 1;
            ctx = 0;
            check_eob = false;
            // A zero is never the last token before EOB, so c < eob <= 16.
            continue;
        }
        sink.node(plane, band, ctx, 1, true);
        let a = v.abs();
        if a == 1 {
            sink.node(plane, band, ctx, 2, false);
            ctx = 1;
        } else {
            sink.node(plane, band, ctx, 2, true);
            if a <= 4 {
                sink.node(plane, band, ctx, 3, false);
                if a == 2 {
                    sink.node(plane, band, ctx, 4, false);
                } else {
                    sink.node(plane, band, ctx, 4, true);
                    sink.node(plane, band, ctx, 5, a == 4);
                }
            } else {
                sink.node(plane, band, ctx, 3, true);
                if a <= 10 {
                    sink.node(plane, band, ctx, 6, false);
                    if a <= 6 {
                        sink.node(plane, band, ctx, 7, false);
                        extra(sink, &PCAT1, a - 5);
                    } else {
                        sink.node(plane, band, ctx, 7, true);
                        extra(sink, &PCAT2, a - 7);
                    }
                } else {
                    sink.node(plane, band, ctx, 6, true);
                    if a <= 34 {
                        sink.node(plane, band, ctx, 8, false);
                        if a <= 18 {
                            sink.node(plane, band, ctx, 9, false);
                            extra(sink, &PCAT3, a - 11);
                        } else {
                            sink.node(plane, band, ctx, 9, true);
                            extra(sink, &PCAT4, a - 19);
                        }
                    } else {
                        sink.node(plane, band, ctx, 8, true);
                        if a <= 66 {
                            sink.node(plane, band, ctx, 10, false);
                            extra(sink, &PCAT5, a - 35);
                        } else {
                            sink.node(plane, band, ctx, 10, true);
                            extra(sink, &PCAT6, a - 67);
                        }
                    }
                }
            }
            ctx = 2;
        }
        sink.fixed(128, v < 0);
        c += 1;
        if c == 16 {
            return;
        }
        check_eob = true;
        if c == eob {
            sink.node(plane, COEFF_BANDS[c], ctx, 0, false);
            return;
        }
    }
}

/// The end of block for `levels` (natural order): one past the last
/// non-zero in zigzag order, and at least `first`.
pub(crate) fn eob_of(levels: &[i16], first: usize) -> usize {
    let mut eob = first;
    for c in first..16 {
        if levels[ZIGZAG[c]] != 0 {
            eob = c + 1;
        }
    }
    eob
}

/// Codes all blocks of a macroblock in decoder order (Y2, Y, U, V) and
/// updates the token contexts. `levels` holds 25 blocks of 16, Y2 last.
pub(crate) fn write_mb<S: TokenSink>(
    sink: &mut S,
    levels: &[i16; 400],
    has_y2: bool,
    above: &mut [u8; 9],
    left: &mut [u8; 9],
) {
    let (y_plane, y_first) = if has_y2 {
        let ctx = (above[8] + left[8]) as usize;
        let lv = &levels[384..400];
        let eob = eob_of(lv, 0);
        write_block(sink, 1, ctx, 0, lv, eob);
        let nz = (eob > 0) as u8;
        above[8] = nz;
        left[8] = nz;
        (0, 1)
    } else {
        (3, 0)
    };
    for b in 0..16 {
        let ctx = (above[b & 3] + left[b >> 2]) as usize;
        let lv = &levels[b * 16..b * 16 + 16];
        let eob = eob_of(lv, y_first);
        write_block(sink, y_plane, ctx, y_first, lv, eob);
        let nz = (eob > y_first) as u8;
        above[b & 3] = nz;
        left[b >> 2] = nz;
    }
    for b in 16..24 {
        let base = if b < 20 { 4 } else { 6 };
        let j = b & 3;
        let ai = base + (j & 1);
        let li = base + (j >> 1);
        let ctx = (above[ai] + left[li]) as usize;
        let lv = &levels[b * 16..b * 16 + 16];
        let eob = eob_of(lv, 0);
        write_block(sink, 2, ctx, 0, lv, eob);
        let nz = (eob > 0) as u8;
        above[ai] = nz;
        left[li] = nz;
    }
}

/// The token contexts after a skipped macroblock (as the decoder resets them).
pub(crate) fn skip_mb(has_y2: bool, above: &mut [u8; 9], left: &mut [u8; 9]) {
    for k in 0..8 {
        above[k] = 0;
        left[k] = 0;
    }
    if has_y2 {
        above[8] = 0;
        left[8] = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bool_encoder::tree_path;

    /// Records the node bits a block writes.
    struct Trace(Vec<(usize, bool)>);
    impl TokenSink for Trace {
        fn node(&mut self, _plane: usize, _band: usize, _ctx: usize, node: usize, bit: bool) {
            self.0.push((node, bit));
        }
        fn fixed(&mut self, _prob: u8, _bit: bool) {}
    }

    fn token(level: i32) -> u8 {
        match level {
            0..=4 => level as u8,
            5..=6 => 5,
            7..=10 => 6,
            11..=18 => 7,
            19..=34 => 8,
            35..=66 => 9,
            _ => 10,
        }
    }

    #[test]
    fn the_writer_follows_coeff_tree() {
        // A non-zero level at position 0 then the end of block: the tree
        // path of its token, then the path of dct_eob.
        for level in [1i32, 2, 3, 4, 5, 6, 7, 10, 11, 18, 19, 34, 35, 66, 67, 2114] {
            let mut lv = [0i16; 16];
            lv[0] = level as i16;
            let mut t = Trace(Vec::new());
            write_block(&mut t, 3, 0, 0, &lv, 1);
            let mut path = [(0usize, false); 16];
            let n = tree_path(&COEFF_TREE, token(level), &mut path);
            let want: Vec<(usize, bool)> = path[..n].iter().map(|&(node, b)| (node >> 1, b)).collect();
            assert_eq!(&t.0[..n], &want[..], "level {level}");
            let n2 = tree_path(&COEFF_TREE, 11, &mut path);
            let eob: Vec<(usize, bool)> = path[..n2].iter().map(|&(node, b)| (node >> 1, b)).collect();
            assert_eq!(&t.0[n..], &eob[..], "end of block after level {level}");
        }
        // After a zero, the end-of-block branch is skipped.
        let mut lv = [0i16; 16];
        lv[1] = 1;
        let mut t = Trace(Vec::new());
        write_block(&mut t, 3, 0, 0, &lv, 2);
        assert_eq!(&t.0[..2], &[(0, true), (1, false)]); // DCT_0 after "not eob"
        assert_eq!(t.0[2], (1, true), "no end-of-block node after a zero");
    }
}
