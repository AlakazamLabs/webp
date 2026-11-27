//! Bit costs for rate-distortion decisions, in 1/256 bits.

use crate::consts::*;
use crate::transform::MAX_LEVEL;
use std::sync::OnceLock;

/// The cost of a bool with probability `prob / 256` of being zero.
#[inline]
pub(crate) fn bit_cost(prob: u8, bit: bool) -> u32 {
    static TABLE: OnceLock<[u32; 257]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 257];
        for (p, c) in t.iter_mut().enumerate().skip(1) {
            *c = (-(p as f64 / 256.0).log2() * 256.0).round() as u32;
        }
        t[0] = t[1];
        t
    });
    table[if bit { 256 - prob as usize } else { prob as usize }]
}

/// The cost of tree-coded `value`.
pub(crate) fn tree_cost(tree: &[i8], probs: &[u8], value: u8) -> u32 {
    let mut path = [(0usize, false); 16];
    let n = crate::bool_encoder::tree_path(tree, value, &mut path);
    path[..n]
        .iter()
        .map(|&(node, bit)| bit_cost(probs[node >> 1], bit))
        .sum()
}

/// The token of a coefficient level (absolute value): 0 to 4 are DCT_0 to
/// DCT_4, 5 to 10 are dct_cat1 to dct_cat6.
#[inline]
pub(crate) fn token_of(level: u32) -> usize {
    match level {
        0..=4 => level as usize,
        5..=6 => 5,
        7..=10 => 6,
        11..=18 => 7,
        19..=34 => 8,
        35..=66 => 9,
        _ => 10,
    }
}

/// The end-of-block token's index in the cost tables.
pub(crate) const EOB: usize = 11;

/// Costs of the extra bits of each level (0 below 5), plus the sign bit for
/// every non-zero level.
fn extra_and_sign(level: u32) -> u32 {
    if level == 0 {
        return 0;
    }
    let (probs, base): (&[u8], u32) = match level {
        0..=4 => (&[], level),
        5..=6 => (&PCAT1, 5),
        7..=10 => (&PCAT2, 7),
        11..=18 => (&PCAT3, 11),
        19..=34 => (&PCAT4, 19),
        35..=66 => (&PCAT5, 35),
        _ => (&PCAT6, 67),
    };
    let v = level - base;
    let n = probs.len();
    let extra: u32 = probs
        .iter()
        .enumerate()
        .map(|(i, &p)| bit_cost(p, (v >> (n - 1 - i)) & 1 == 1))
        .sum();
    extra + 256
}

fn extra_table() -> &'static [u16] {
    static T: OnceLock<Vec<u16>> = OnceLock::new();
    T.get_or_init(|| (0..=MAX_LEVEL as u32).map(|l| extra_and_sign(l) as u16).collect())
}

/// The token-tree costs for one frame's coefficient probabilities.
pub(crate) struct TokenCosts {
    /// `[plane][band][ctx][token]`, the end of block allowed (token 11 is
    /// the end of block itself).
    with_eob: Box<[[[[u16; 12]; 3]; 8]; 4]>,
    /// The same after a zero, where the end-of-block branch is skipped.
    no_eob: Box<[[[[u16; 12]; 3]; 8]; 4]>,
    extra: &'static [u16],
}

impl TokenCosts {
    pub(crate) fn new(probs: &CoeffProbs) -> Self {
        let mut with_eob = Box::new([[[[0u16; 12]; 3]; 8]; 4]);
        let mut no_eob = Box::new([[[[0u16; 12]; 3]; 8]; 4]);
        for plane in 0..4 {
            for band in 0..8 {
                for ctx in 0..3 {
                    let p = &probs[plane][band][ctx];
                    let c = |n: usize, b: bool| bit_cost(p[n], b);
                    // Costs below the zero/non-zero node.
                    let mut rest = [0u32; 11];
                    rest[0] = c(1, false);
                    let nz = c(1, true);
                    rest[1] = nz + c(2, false);
                    let big = nz + c(2, true);
                    rest[2] = big + c(3, false) + c(4, false);
                    rest[3] = big + c(3, false) + c(4, true) + c(5, false);
                    rest[4] = big + c(3, false) + c(4, true) + c(5, true);
                    let cat = big + c(3, true);
                    rest[5] = cat + c(6, false) + c(7, false);
                    rest[6] = cat + c(6, false) + c(7, true);
                    rest[7] = cat + c(6, true) + c(8, false) + c(9, false);
                    rest[8] = cat + c(6, true) + c(8, false) + c(9, true);
                    rest[9] = cat + c(6, true) + c(8, true) + c(10, false);
                    rest[10] = cat + c(6, true) + c(8, true) + c(10, true);
                    for t in 0..11 {
                        no_eob[plane][band][ctx][t] = rest[t].min(u16::MAX as u32) as u16;
                        with_eob[plane][band][ctx][t] = (rest[t] + c(0, true)).min(u16::MAX as u32) as u16;
                    }
                    with_eob[plane][band][ctx][EOB] = c(0, false) as u16;
                    no_eob[plane][band][ctx][EOB] = u16::MAX;
                }
            }
        }
        TokenCosts {
            with_eob,
            no_eob,
            extra: extra_table(),
        }
    }

    /// The cost of a coefficient of `level` (absolute) at scan position `n`.
    #[inline]
    pub(crate) fn level(&self, plane: usize, n: usize, ctx: usize, after_zero: bool, level: u32) -> u32 {
        let band = COEFF_BANDS[n];
        let t = token_of(level);
        let table = if after_zero { &self.no_eob } else { &self.with_eob };
        table[plane][band][ctx][t] as u32 + self.extra[level.min(MAX_LEVEL as u32) as usize] as u32
    }

    /// The cost of the end of block at scan position `n` (after a non-zero,
    /// or at the first position).
    #[inline]
    pub(crate) fn eob(&self, plane: usize, n: usize, ctx: usize) -> u32 {
        self.with_eob[plane][COEFF_BANDS[n]][ctx][EOB] as u32
    }

    /// The cost of a block's levels (natural order) from position `first`,
    /// with the neighbour context `ctx`.
    pub(crate) fn block(&self, plane: usize, ctx: usize, first: usize, levels: &[i16]) -> u32 {
        let mut last = None;
        for n in (first..16).rev() {
            if levels[ZIGZAG[n]] != 0 {
                last = Some(n);
                break;
            }
        }
        let Some(last) = last else {
            return self.eob(plane, first, ctx);
        };
        let mut cost = 0;
        let mut ctx = ctx;
        let mut after_zero = false;
        for n in first..=last {
            let l = levels[ZIGZAG[n]].unsigned_abs() as u32;
            cost += self.level(plane, n, ctx, after_zero, l);
            ctx = (l as usize).min(2);
            after_zero = l == 0;
        }
        if last < 15 {
            cost += self.eob(plane, last + 1, ctx);
        }
        cost
    }
}

/// Mode and flag costs of a key frame.
pub(crate) struct ModeCosts {
    pub(crate) ymode: [u32; 5],
    /// `[above][left][mode]`.
    pub(crate) bmode: Box<[[[u32; NUM_BMODES]; NUM_BMODES]; NUM_BMODES]>,
    pub(crate) uv_mode: [u32; 4],
}

impl ModeCosts {
    pub(crate) fn get() -> &'static ModeCosts {
        static M: OnceLock<ModeCosts> = OnceLock::new();
        M.get_or_init(|| {
            let mut bmode = Box::new([[[0u32; NUM_BMODES]; NUM_BMODES]; NUM_BMODES]);
            for a in 0..NUM_BMODES {
                for l in 0..NUM_BMODES {
                    for m in 0..NUM_BMODES {
                        bmode[a][l][m] = tree_cost(&BMODE_TREE, &crate::tables::KF_BMODE_PROBS[a][l], m as u8);
                    }
                }
            }
            ModeCosts {
                ymode: std::array::from_fn(|m| tree_cost(&KF_YMODE_TREE, &KF_YMODE_PROBS, m as u8)),
                bmode,
                uv_mode: std::array::from_fn(|m| tree_cost(&UV_MODE_TREE, &KF_UV_MODE_PROBS, m as u8)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bool_encoder::BoolEncoder;
    use crate::tables::DEFAULT_COEFF_PROBS;
    use crate::tokens::{write_block, Writer};

    #[test]
    fn token_costs_follow_coeff_tree() {
        let probs = DEFAULT_COEFF_PROBS;
        let costs = TokenCosts::new(&probs);
        let p = &probs[2][3][1];
        for (tok, level) in [
            (0usize, 0u32),
            (1, 1),
            (2, 2),
            (3, 3),
            (4, 4),
            (5, 5),
            (6, 7),
            (7, 11),
            (8, 19),
            (9, 35),
            (10, 67),
        ] {
            let tree = tree_cost(&COEFF_TREE, p, tok as u8);
            let got = costs.level(2, 3, 1, false, level) - extra_and_sign(level);
            assert_eq!(got, tree, "token {tok}");
        }
        assert_eq!(costs.eob(2, 3, 1), tree_cost(&COEFF_TREE, p, 11));
    }

    #[test]
    #[allow(clippy::needless_range_loop)]
    fn block_cost_matches_the_written_size() {
        // Many random blocks: the estimated cost is close to the bits written.
        let costs = TokenCosts::new(&DEFAULT_COEFF_PROBS);
        let mut seed = 5u32;
        let mut est = 0u64;
        let mut e = BoolEncoder::new();
        for i in 0..4000 {
            let mut lv = [0i16; 16];
            for k in 0..16 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let r = seed % 100;
                lv[k] = match r {
                    0..=59 => 0,
                    60..=79 => 1,
                    80..=89 => -2,
                    90..=97 => (seed % 30) as i16 - 15,
                    _ => (seed % 3000) as i16 - 1500,
                };
                if k > 6 && r < 80 {
                    lv[k] = 0;
                }
            }
            let plane = i % 4;
            let first = if plane == 0 { 1 } else { 0 };
            let ctx = i % 3;
            est += costs.block(plane, ctx, first, &lv) as u64;
            let eob = crate::tokens::eob_of(&lv, first);
            let mut w = Writer {
                enc: &mut e,
                probs: &DEFAULT_COEFF_PROBS,
            };
            write_block(&mut w, plane, ctx, first, &lv, eob);
        }
        let real = e.finish().len() as u64 * 8 * 256;
        let ratio = est as f64 / real as f64;
        assert!((0.97..1.03).contains(&ratio), "estimate {est}, written {real}");
    }
}
