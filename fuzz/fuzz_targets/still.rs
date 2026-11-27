#![no_main]
//! Still images: any size up to 48x48 (odd sizes included), any settings,
//! RGBA, RGB or 4:2:0 input. Valid input must encode, and the output must be
//! a well-formed RIFF file whose reconstruction has the picture's size.

use libfuzzer_sys::fuzz_target;
use webp_lossy::{Encoder, Picture};

#[path = "common.rs"]
mod common;

fuzz_target!(|data: &[u8]| {
    if data.len() < 9 {
        return;
    }
    let (h, rest) = data.split_at(9);
    let w = 1 + (h[6] % 48) as u32;
    let hh = 1 + (h[7] % 48) as u32;
    let config = common::config(h);
    let enc = Encoder::new(config);
    let (cw, ch) = (w.div_ceil(2) as usize, hh.div_ceil(2) as usize);
    let n = (w * hh) as usize;
    let kind = h[8] >> 2;
    let out = match h[8] % 3 {
        0 => {
            let px = common::pixels(rest, n * 4, kind);
            enc.encode(Picture::Rgba { width: w, height: hh, data: &px })
        }
        1 => {
            let px = common::pixels(rest, n * 3, kind);
            enc.encode(Picture::Rgb { width: w, height: hh, data: &px })
        }
        _ => {
            let px = common::pixels(rest, n + 2 * cw * ch, kind);
            let (y, uv) = px.split_at(n);
            let (u, v) = uv.split_at(cw * ch);
            enc.encode(Picture::Yuv420 { width: w, height: hh, y, u, v })
        }
    }
    .expect("valid input encodes");
    common::check_riff(&out.data);
    assert_eq!(out.reconstruction.y.len(), n);
    assert_eq!(out.reconstruction.u.len(), cw * ch);
});
