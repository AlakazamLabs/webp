//! Shared input decoding for the fuzz targets: settings from the first
//! bytes, pixels from the rest (cycled, or a fixed pattern).

use webp_lossy::Config;

/// Settings from 6 bytes, covering every option and the extreme values.
pub fn config(h: &[u8]) -> Config {
    let mut c = Config::new(match h[0] {
        0 => 0.0,
        255 => 100.0,
        q => q as f32 * 100.0 / 255.0,
    });
    c.method = h[1] % 7;
    c.segments = 1 + h[2] % 4;
    c.sns_strength = [0, 50, 100, h[2] % 101][(h[2] >> 6) as usize];
    c.filter_level = if h[3] & 0x80 != 0 { Some(h[3] & 63) } else { None };
    c.filter_sharpness = (h[4] >> 5) & 7;
    c.simple_filter = h[4] & 1 != 0;
    c.exact = h[4] & 2 != 0;
    c.alpha_quality = [100, 0, 50, h[5] % 101][(h[5] >> 6) as usize];
    // Targets make many encodes: only now and then.
    if h[4] & 0x1c == 0x1c {
        if h[5] & 1 == 0 {
            c.target_size = Some(64 + (h[3] as usize) * 8);
        } else {
            c.target_psnr = Some(20.0 + (h[3] & 31) as f32);
        }
    }
    c
}

/// `len` bytes of pixels: `kind` 0 cycles `data`, 1 is flat, 2 is all
/// transparent RGBA, 3 is alternating extremes.
pub fn pixels(data: &[u8], len: usize, kind: u8) -> Vec<u8> {
    match kind % 4 {
        0 if !data.is_empty() => data.iter().cycle().take(len).copied().collect(),
        0 | 1 => vec![data.first().copied().unwrap_or(128); len],
        2 => (0..len).map(|i| if i % 4 == 3 { 0 } else { (i * 37) as u8 }).collect(),
        _ => (0..len).map(|i| if (i / 4) % 2 == 0 { 0 } else { 255 }).collect(),
    }
}

/// Checks the RIFF header and the chunk sizes walk to the end exactly.
pub fn check_riff(f: &[u8]) {
    assert_eq!(&f[..4], b"RIFF");
    assert_eq!(&f[8..12], b"WEBP");
    let size = u32::from_le_bytes(f[4..8].try_into().unwrap()) as usize;
    assert_eq!(size + 8, f.len());
    let mut pos = 12;
    while pos < f.len() {
        let len = u32::from_le_bytes(f[pos + 4..pos + 8].try_into().unwrap()) as usize;
        pos += 8 + len + (len & 1);
    }
    assert_eq!(pos, f.len());
}
