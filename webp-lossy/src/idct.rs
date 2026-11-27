//! The inverse transforms (RFC 6386 Sections 14.3 and 14.4), written from
//! the RFC's description: two 1-D passes (columns, then rows), with the
//! inputs, the intermediate values and the results all held as 16-bit
//! signed integers, and the exact rounding the RFC gives.

// sqrt(2) * cos(pi / 8) - 1 and sqrt(2) * sin(pi / 8), in 16-bit fixed point.
const COS_MINUS_ONE: i32 = 20091;
const SIN: i32 = 35468;

#[inline(always)]
fn mul_cos(x: i32) -> i32 {
    x + ((x * COS_MINUS_ONE) >> 16)
}

#[inline(always)]
fn mul_sin(x: i32) -> i32 {
    (x * SIN) >> 16
}

/// One 1-D inverse DCT of `x0..x3`.
#[inline(always)]
fn idct1(x0: i32, x1: i32, x2: i32, x3: i32) -> [i32; 4] {
    let a = x0 + x2;
    let b = x0 - x2;
    let c = mul_sin(x1) - mul_cos(x3);
    let d = mul_cos(x1) + mul_sin(x3);
    [a + d, b + c, b - c, a - d]
}

/// The inverse DCT of `coeffs` (raster order, dequantized) into a 4x4
/// residual (raster order).
pub(crate) fn idct4x4(coeffs: &[i16; 16]) -> [i16; 16] {
    let mut tmp = [0i16; 16];
    for c in 0..4 {
        let v = idct1(
            coeffs[c] as i32,
            coeffs[4 + c] as i32,
            coeffs[8 + c] as i32,
            coeffs[12 + c] as i32,
        );
        for r in 0..4 {
            tmp[r * 4 + c] = v[r] as i16;
        }
    }
    let mut out = [0i16; 16];
    for r in 0..4 {
        let row = &tmp[r * 4..r * 4 + 4];
        let v = idct1(row[0] as i32, row[1] as i32, row[2] as i32, row[3] as i32);
        for c in 0..4 {
            out[r * 4 + c] = ((v[c] + 4) >> 3) as i16;
        }
    }
    out
}

/// Adds the inverse DCT of `coeffs` to the 4x4 prediction at `pos` in
/// `buf` (stride `stride`), clamping to 0..=255.
pub(crate) fn idct_add(coeffs: &[i16; 16], buf: &mut [u8], pos: usize, stride: usize) {
    let res = idct4x4(coeffs);
    for r in 0..4 {
        let row = &mut buf[pos + r * stride..pos + r * stride + 4];
        for c in 0..4 {
            row[c] = (row[c] as i32 + res[r * 4 + c] as i32).clamp(0, 255) as u8;
        }
    }
}

/// The inverse Walsh-Hadamard transform of the Y2 block: the 16 luma DC
/// coefficients, in subblock raster order.
pub(crate) fn iwht4x4(input: &[i16; 16]) -> [i16; 16] {
    let mut tmp = [0i16; 16];
    for c in 0..4 {
        let (i0, i1, i2, i3) = (
            input[c] as i32,
            input[4 + c] as i32,
            input[8 + c] as i32,
            input[12 + c] as i32,
        );
        let a = i0 + i3;
        let b = i1 + i2;
        let cc = i1 - i2;
        let d = i0 - i3;
        tmp[c] = (a + b) as i16;
        tmp[4 + c] = (cc + d) as i16;
        tmp[8 + c] = (a - b) as i16;
        tmp[12 + c] = (d - cc) as i16;
    }
    let mut out = [0i16; 16];
    for r in 0..4 {
        let t = &tmp[r * 4..r * 4 + 4];
        let (i0, i1, i2, i3) = (t[0] as i32, t[1] as i32, t[2] as i32, t[3] as i32);
        let a = i0 + i3;
        let b = i1 + i2;
        let cc = i1 - i2;
        let d = i0 - i3;
        out[r * 4] = ((a + b + 3) >> 3) as i16;
        out[r * 4 + 1] = ((cc + d + 3) >> 3) as i16;
        out[r * 4 + 2] = ((a - b + 3) >> 3) as i16;
        out[r * 4 + 3] = ((d - cc + 3) >> 3) as i16;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_only_spreads_evenly() {
        let mut c = [0i16; 16];
        c[0] = 80;
        assert!(idct4x4(&c).iter().all(|&v| v == 10));
        let mut y2 = [0i16; 16];
        y2[0] = 77;
        assert!(iwht4x4(&y2).iter().all(|&v| v == (77 + 3) >> 3));
    }
}
