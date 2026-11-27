//! VP8's small constant tables and trees, typed from the prose of RFC 6386
//! (Sections 10, 11.2, 11.4, 13.2 and 13.3). The large tables are in
//! `tables.rs`, which a script extracts from the same prose.

// Whole-macroblock luma modes (Section 11.2). The first four are also the
// chroma modes.
pub(crate) const DC_PRED: u8 = 0;
pub(crate) const V_PRED: u8 = 1;
pub(crate) const H_PRED: u8 = 2;
pub(crate) const TM_PRED: u8 = 3;
pub(crate) const B_PRED: u8 = 4;

// Subblock modes (Section 11.2), in the RFC's numbering.
pub(crate) const B_DC_PRED: u8 = 0;
pub(crate) const B_TM_PRED: u8 = 1;
pub(crate) const B_VE_PRED: u8 = 2;
pub(crate) const B_HE_PRED: u8 = 3;
pub(crate) const B_LD_PRED: u8 = 4;
pub(crate) const B_RD_PRED: u8 = 5;
pub(crate) const B_VR_PRED: u8 = 6;
pub(crate) const B_VL_PRED: u8 = 7;
pub(crate) const B_HD_PRED: u8 = 8;
pub(crate) const B_HU_PRED: u8 = 9;
pub(crate) const NUM_BMODES: usize = 10;

/// `kf_ymode_tree` (Section 11.2).
pub(crate) const KF_YMODE_TREE: [i8; 8] = [
    -(B_PRED as i8),
    2,
    4,
    6,
    -(DC_PRED as i8),
    -(V_PRED as i8),
    -(H_PRED as i8),
    -(TM_PRED as i8),
];
/// `kf_ymode_prob` (Section 11.2).
pub(crate) const KF_YMODE_PROBS: [u8; 4] = [145, 156, 163, 128];

/// `bmode_tree` (Section 11.2).
pub(crate) const BMODE_TREE: [i8; 18] = [
    -(B_DC_PRED as i8),
    2,
    -(B_TM_PRED as i8),
    4,
    -(B_VE_PRED as i8),
    6,
    8,
    12,
    -(B_HE_PRED as i8),
    10,
    -(B_RD_PRED as i8),
    -(B_VR_PRED as i8),
    -(B_LD_PRED as i8),
    14,
    -(B_VL_PRED as i8),
    16,
    -(B_HD_PRED as i8),
    -(B_HU_PRED as i8),
];

/// `uv_mode_tree` (Section 11.4).
pub(crate) const UV_MODE_TREE: [i8; 6] = [
    -(DC_PRED as i8),
    2,
    -(V_PRED as i8),
    4,
    -(H_PRED as i8),
    -(TM_PRED as i8),
];
/// `kf_uv_mode_prob` (Section 11.4).
pub(crate) const KF_UV_MODE_PROBS: [u8; 3] = [142, 114, 183];

/// `mb_segment_tree` (Section 10).
pub(crate) const SEGMENT_TREE: [i8; 6] = [2, 4, -0, -1, -2, -3];

/// The subblock mode a whole-block luma mode implies for its neighbours'
/// contexts (Section 11.3, item 4).
pub(crate) fn implied_bmode(y_mode: u8) -> u8 {
    match y_mode {
        V_PRED => B_VE_PRED,
        H_PRED => B_HE_PRED,
        TM_PRED => B_TM_PRED,
        _ => B_DC_PRED,
    }
}

/// `coeff_bands` (Section 13.3), with a 17th entry for the position past
/// the last coefficient (where only an end of block can be coded; its band
/// is never used, as a 16th coefficient ends the block).
pub(crate) const COEFF_BANDS: [usize; 17] = [0, 1, 2, 3, 6, 4, 5, 6, 6, 6, 6, 6, 6, 6, 6, 7, 0];

/// The zig-zag scan of a 4x4 block (Section 13: coefficients follow "a
/// so-called zig-zag order"): position in scan order to raster index,
/// walking the anti-diagonals alternately up and down, starting right.
pub(crate) const ZIGZAG: [usize; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

// The extra-bit probabilities of dct_cat1..dct_cat6 (Section 13.2).
pub(crate) const PCAT1: [u8; 1] = [159];
pub(crate) const PCAT2: [u8; 2] = [165, 145];
pub(crate) const PCAT3: [u8; 3] = [173, 148, 140];
pub(crate) const PCAT4: [u8; 4] = [176, 155, 140, 135];
pub(crate) const PCAT5: [u8; 5] = [180, 157, 141, 134, 130];
pub(crate) const PCAT6: [u8; 11] = [254, 254, 243, 230, 196, 177, 153, 140, 133, 130, 129];

/// `coeff_tree` (Section 13.2): the token tree, with tokens numbered as in
/// the RFC's `dct_token` (DCT_0..DCT_4 = 0..4, dct_cat1..6 = 5..10,
/// dct_eob = 11). The token writer and the cost tables walk it in code; a
/// test checks that they follow it exactly.
#[cfg(test)]
pub(crate) const COEFF_TREE: [i8; 22] = [
    -11, 2, -0, 4, -1, 6, 8, 12, -2, 10, -3, -4, 14, 16, -5, -6, 18, 20, -7, -8, -9, -10,
];

/// Coefficient probabilities, `[plane][band][context][node]` (Section 13.3).
pub(crate) type CoeffProbs = [[[[u8; 11]; 3]; 8]; 4];

/// Plane types of the coefficient tables (Section 13.3).
pub(crate) const PLANE_Y_AFTER_Y2: usize = 0;
pub(crate) const PLANE_Y2: usize = 1;
pub(crate) const PLANE_UV: usize = 2;
pub(crate) const PLANE_Y_NO_Y2: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zigzag_walks_the_anti_diagonals() {
        // Rebuild the scan from its definition.
        let mut scan = Vec::new();
        for d in 0..7i32 {
            let mut cells: Vec<(i32, i32)> = (0..4)
                .filter_map(|r| {
                    let c = d - r;
                    (0..4).contains(&c).then_some((r, c))
                })
                .collect();
            // Even diagonals run up and to the right (rows decreasing).
            if d % 2 == 0 {
                cells.reverse();
            }
            scan.extend(cells.into_iter().map(|(r, c)| (r * 4 + c) as usize));
        }
        assert_eq!(scan, ZIGZAG);
    }

    #[test]
    fn trees_have_one_leaf_per_value() {
        fn leaves(tree: &[i8]) -> Vec<u8> {
            let mut v: Vec<u8> = tree.iter().filter(|&&t| t <= 0).map(|&t| (-t) as u8).collect();
            v.sort();
            v
        }
        assert_eq!(leaves(&KF_YMODE_TREE), vec![0, 1, 2, 3, 4]);
        assert_eq!(leaves(&BMODE_TREE), (0..10).collect::<Vec<u8>>());
        assert_eq!(leaves(&UV_MODE_TREE), vec![0, 1, 2, 3]);
        assert_eq!(leaves(&SEGMENT_TREE), vec![0, 1, 2, 3]);
        assert_eq!(leaves(&COEFF_TREE), (0..12).collect::<Vec<u8>>());
    }
}
