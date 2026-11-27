#![no_main]
//! The bulk target: pictures of 1 to 17 pixels a side (one to four
//! macroblocks), every method and setting but the target searches, RGBA,
//! RGB or 4:2:0. Fast enough for the billion-execution budget; `still` and
//! `anim` cover larger sizes, the searches and animations.

use libfuzzer_sys::fuzz_target;
use webp_lossy::{Encoder, Picture};

#[path = "common.rs"]
mod common;

fuzz_target!(|data: &[u8]| {
    if data.len() < 9 {
        return;
    }
    let (h, rest) = data.split_at(9);
    let w = 1 + (h[6] % 17) as u32;
    let hh = 1 + (h[7] % 17) as u32;
    let mut config = common::config(h);
    // Mostly the fast methods, so the budget buys many inputs; all seven
    // still come up.
    config.method = [0, 0, 0, 0, 1, 1, 1, 2, 3, 4, 5, 6][(h[1] % 12) as usize];
    config.target_size = None;
    config.target_psnr = None;
    let enc = Encoder::new(config);
    let (cw, ch) = (w.div_ceil(2) as usize, hh.div_ceil(2) as usize);
    let n = (w * hh) as usize;
    let kind = h[8] >> 2;
    let out = match h[8] % 3 {
        0 => enc.encode(Picture::Rgba { width: w, height: hh, data: &common::pixels(rest, n * 4, kind) }),
        1 => enc.encode(Picture::Rgb { width: w, height: hh, data: &common::pixels(rest, n * 3, kind) }),
        _ => {
            let px = common::pixels(rest, n + 2 * cw * ch, kind);
            let (y, uv) = px.split_at(n);
            let (u, v) = uv.split_at(cw * ch);
            enc.encode(Picture::Yuv420 { width: w, height: hh, y, u, v })
        }
    }
    .expect("valid input encodes");
    common::check_riff(&out.data);
});
