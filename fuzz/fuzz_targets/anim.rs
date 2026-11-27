#![no_main]
//! Animations: canvases up to 40x40, up to 5 frames with fuzzed timestamps
//! (out-of-order ones must be refused, not panic), lossy, lossless or mixed
//! frames.

use libfuzzer_sys::fuzz_target;
use webp_lossy::{AnimationConfig, AnimationEncoder, EncodeError, FrameKind};

#[path = "common.rs"]
mod common;

fuzz_target!(|data: &[u8]| {
    if data.len() < 12 {
        return;
    }
    let (h, rest) = data.split_at(12);
    let w = 1 + (h[6] % 40) as u32;
    let hh = 1 + (h[7] % 40) as u32;
    let mut config = common::config(h);
    config.method = config.method.min(4);
    let mut anim = AnimationConfig::default();
    anim.frames = [FrameKind::Lossy, FrameKind::Lossless, FrameKind::Smaller, FrameKind::Lossy][(h[8] % 4) as usize];
    anim.loop_count = h[9] as u16;
    anim.background = [h[9], h[10], h[11], h[8]];
    let mut enc = AnimationEncoder::new(w, hh, config, anim).expect("valid canvas");
    let n = (w * hh * 4) as usize;
    let frames = 1 + (h[10] % 5) as usize;
    let mut ts = 0u32;
    let mut last_ok: Option<u32> = None;
    for f in 0..frames {
        let seed = rest.get(f).copied().unwrap_or(0);
        let start = (seed as usize * 7) % rest.len().max(1);
        let px = common::pixels(rest.get(start..).unwrap_or(&[]), n, h[11] >> (f % 4));
        // Mostly increasing, sometimes repeated or backwards.
        ts = match seed % 8 {
            0 => ts,
            1 => ts.saturating_sub(5),
            _ => ts + 1 + seed as u32 * 3,
        };
        match enc.add_frame(&px, ts) {
            Ok(()) => last_ok = Some(ts),
            Err(EncodeError::TimestampOrder { previous, got }) => assert!(got <= previous),
            Err(e) => panic!("unexpected error {e:?}"),
        }
    }
    let file = match last_ok {
        Some(_) => enc.finish().expect("finishes"),
        None => {
            assert_eq!(enc.finish().err(), Some(EncodeError::NoFrames));
            return;
        }
    };
    common::check_riff(&file);
});
