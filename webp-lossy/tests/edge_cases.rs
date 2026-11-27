//! Inputs at the edges of the format, kept as regression tests (and the
//! home of any fuzzing finding): every size up to two macroblocks a side,
//! flat and extreme pictures, all-transparent RGBA, quality 0 and 100.

use webp_lossy::{AnimationConfig, AnimationEncoder, Config, EncodeError, Encoder, Picture};

fn walk_chunks(f: &[u8]) -> Vec<[u8; 4]> {
    assert_eq!(&f[..4], b"RIFF");
    assert_eq!(&f[8..12], b"WEBP");
    assert_eq!(u32::from_le_bytes(f[4..8].try_into().unwrap()) as usize + 8, f.len());
    let mut pos = 12;
    let mut names = Vec::new();
    while pos < f.len() {
        names.push(f[pos..pos + 4].try_into().unwrap());
        let len = u32::from_le_bytes(f[pos + 4..pos + 8].try_into().unwrap()) as usize;
        pos += 8 + len + (len & 1);
    }
    assert_eq!(pos, f.len());
    names
}

#[test]
fn every_small_size_at_the_quality_extremes() {
    for w in 1..=33u32 {
        for h in [1u32, 2, 15, 16, 17, 32, 33] {
            for (q, m) in [(0.0f32, 0u8), (100.0, 6), (50.0, 3)] {
                let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i * 89 % 256) as u8).collect();
                let out = Encoder::new(Config::new(q).with_method(m))
                    .encode(Picture::Rgb {
                        width: w,
                        height: h,
                        data: &rgb,
                    })
                    .unwrap();
                assert_eq!(walk_chunks(&out.data), [*b"VP8 "]);
                assert_eq!(out.reconstruction.y.len(), (w * h) as usize);
            }
        }
    }
}

#[test]
fn flat_extreme_and_transparent_pictures() {
    let (w, h) = (40u32, 24u32);
    for fill in [
        [0u8, 0, 0, 255],
        [255, 255, 255, 255],
        [0, 0, 0, 0],
        [255, 0, 255, 0],
        [10, 200, 30, 128],
    ] {
        let rgba: Vec<u8> = (0..w * h).flat_map(|_| fill).collect();
        for exact in [false, true] {
            let mut c = Config::new(75.0);
            c.exact = exact;
            let f = Encoder::new(c).encode_rgba(w, h, &rgba).unwrap();
            let names = walk_chunks(&f);
            if fill[3] == 255 {
                assert_eq!(names, [*b"VP8 "]);
            } else {
                assert_eq!(names, [*b"VP8X", *b"ALPH", *b"VP8 "]);
            }
        }
    }
    // Alternating black and white pixels: the largest residuals.
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            if (i + i / w) % 2 == 0 {
                [0u8, 0, 0, 255]
            } else {
                [255; 4]
            }
        })
        .collect();
    for m in 0..=6 {
        Encoder::new(Config::new(100.0).with_method(m))
            .encode_rgba(w, h, &rgba)
            .unwrap();
    }
}

#[test]
fn the_largest_size_is_accepted_and_one_more_refused() {
    let e = Encoder::new(Config::default());
    assert_eq!(
        e.encode_rgb(16384, 1, &[0; 16384 * 3]),
        Err(EncodeError::InvalidDimensions {
            width: 16384,
            height: 1
        })
    );
    let f = Encoder::new(Config::new(50.0).with_method(0))
        .encode_rgb(16383, 2, &vec![7; 16383 * 2 * 3])
        .unwrap();
    assert_eq!(u16::from_le_bytes([f[26], f[27]]), 16383);
}

#[test]
fn an_animation_of_one_unchanged_frame_repeated() {
    let mut a = AnimationEncoder::new(3, 5, Config::new(10.0), AnimationConfig::default()).unwrap();
    let frame = vec![9u8; 3 * 5 * 4];
    for t in [0, 10, 20, 30] {
        a.add_frame(&frame, t).unwrap();
    }
    let f = a.finish().unwrap();
    assert_eq!(walk_chunks(&f), [*b"VP8X", *b"ANIM", *b"ANMF"]);
}
