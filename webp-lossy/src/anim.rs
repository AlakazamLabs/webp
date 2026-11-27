//! Animated WebP (RFC 9649 Section 2.7.1.1): ANIM and ANMF chunks.
//!
//! Each frame is compared with the one before it, and only the rectangle
//! that changed is encoded (at an even offset, as ANMF requires). Frames
//! replace their rectangle (no blending) and are not disposed, so the
//! pixels outside the rectangle stay as the earlier frames left them. A
//! frame identical to the one before it is merged into it.

use crate::riff::{self, Meta};
use crate::{check_len, encode_frame_chunks, Config, EncodeError};

/// The largest animation canvas side (24-bit field).
const MAX_CANVAS: u32 = 1 << 24;
/// The longest frame, in milliseconds (24-bit field).
const MAX_DURATION: u32 = (1 << 24) - 1;

/// How animation frames are coded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameKind {
    /// Lossy (VP8) frames, with alpha where needed.
    #[default]
    Lossy,
    /// Lossless (VP8L) frames. Needs the `vp8l` feature.
    Lossless,
    /// Each frame both ways, keeping the smaller. Needs the `vp8l`
    /// feature.
    Smaller,
}

/// Animation settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AnimationConfig {
    /// How many times to play; 0 (the default) loops forever.
    pub loop_count: u16,
    /// The background colour hint, RGBA. Default transparent black.
    pub background: [u8; 4],
    /// How frames are coded.
    pub frames: FrameKind,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        AnimationConfig {
            loop_count: 0,
            background: [0, 0, 0, 0],
            frames: FrameKind::Lossy,
        }
    }
}

struct Frame {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    timestamp: u32,
    /// The padded chunks inside the ANMF payload.
    chunks: Vec<u8>,
    alpha: bool,
}

/// Encodes an animation, one RGBA frame at a time.
///
/// ```
/// use webp_lossy::{AnimationConfig, AnimationEncoder, Config};
///
/// let (w, h) = (32u32, 32u32);
/// let mut anim = AnimationEncoder::new(w, h, Config::new(70.0), AnimationConfig::default())?;
/// for i in 0..4u32 {
///     let frame: Vec<u8> = (0..w * h).flat_map(|p| [((p + i * 8) % 256) as u8, 0, 255, 255]).collect();
///     anim.add_frame(&frame, i * 100)?;
/// }
/// let webp = anim.finish()?;
/// assert_eq!(&webp[8..12], b"WEBP");
/// # Ok::<(), webp_lossy::EncodeError>(())
/// ```
pub struct AnimationEncoder {
    width: u32,
    height: u32,
    config: Config,
    anim: AnimationConfig,
    meta: Meta,
    frames: Vec<Frame>,
    previous: Option<Vec<u8>>,
}

impl AnimationEncoder {
    /// An encoder for a `width` x `height` canvas. Each frame is encoded with
    /// `config` (`target_size` and `target_psnr` apply per frame).
    pub fn new(width: u32, height: u32, config: Config, anim: AnimationConfig) -> Result<Self, EncodeError> {
        config.validate()?;
        if width == 0
            || height == 0
            || width > MAX_CANVAS
            || height > MAX_CANVAS
            || (width as u64) * (height as u64) > u32::MAX as u64
        {
            return Err(EncodeError::InvalidDimensions { width, height });
        }
        if anim.frames != FrameKind::Lossy && !cfg!(feature = "vp8l") {
            return Err(EncodeError::InvalidConfig("lossless frames need the vp8l feature"));
        }
        Ok(AnimationEncoder {
            width,
            height,
            config,
            anim,
            meta: Meta::default(),
            frames: Vec::new(),
            previous: None,
        })
    }

    /// Embeds an ICC colour profile (the ICCP chunk).
    pub fn set_icc_profile(&mut self, icc: Vec<u8>) {
        self.meta.icc = Some(icc);
    }

    /// Embeds Exif metadata (the EXIF chunk).
    pub fn set_exif(&mut self, exif: Vec<u8>) {
        self.meta.exif = Some(exif);
    }

    /// Embeds XMP metadata (the `XMP ` chunk).
    pub fn set_xmp(&mut self, xmp: Vec<u8>) {
        self.meta.xmp = Some(xmp);
    }

    /// Adds a frame: a full canvas of non-premultiplied RGBA, shown from
    /// `timestamp_ms` until the next frame's timestamp. Timestamps must
    /// increase. A frame may be up to 16383 pixels on a side once reduced
    /// to the rectangle that changed.
    pub fn add_frame(&mut self, rgba: &[u8], timestamp_ms: u32) -> Result<(), EncodeError> {
        let (w, h) = (self.width as usize, self.height as usize);
        check_len(rgba.len(), w.checked_mul(h).and_then(|p| p.checked_mul(4)))?;
        if let Some(last) = self.frames.last() {
            if timestamp_ms <= last.timestamp {
                return Err(EncodeError::TimestampOrder {
                    previous: last.timestamp,
                    got: timestamp_ms,
                });
            }
            if timestamp_ms - last.timestamp > MAX_DURATION {
                return Err(EncodeError::DurationTooLong);
            }
        }
        let rect = match &self.previous {
            None => Some((0, 0, self.width, self.height)),
            Some(prev) => changed_rect(prev, rgba, self.width, self.height),
        };
        let Some((x, y, fw, fh)) = rect else {
            // Nothing changed: the previous frame simply lasts longer.
            return Ok(());
        };
        if fw > crate::MAX_DIMENSION || fh > crate::MAX_DIMENSION {
            return Err(EncodeError::InvalidDimensions { width: fw, height: fh });
        }
        let mut sub = Vec::with_capacity(fw as usize * fh as usize * 4);
        for r in y..y + fh {
            let o = (r as usize * w + x as usize) * 4;
            sub.extend_from_slice(&rgba[o..o + fw as usize * 4]);
        }
        let alpha = sub.chunks_exact(4).any(|p| p[3] != 255);
        let chunks = self.frame_chunks(fw, fh, &sub)?;
        self.frames.push(Frame {
            x,
            y,
            w: fw,
            h: fh,
            timestamp: timestamp_ms,
            chunks,
            alpha,
        });
        match &mut self.previous {
            Some(p) => p.copy_from_slice(rgba),
            None => self.previous = Some(rgba.to_vec()),
        }
        Ok(())
    }

    fn frame_chunks(&self, w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, EncodeError> {
        let lossy = || -> Result<Vec<u8>, EncodeError> {
            let (vp8, alph) = encode_frame_chunks(&self.config, w, h, rgba)?;
            let mut c = Vec::with_capacity(vp8.len() + alph.as_ref().map_or(0, |a| a.len()) + 16);
            if let Some(a) = &alph {
                riff::chunk(&mut c, b"ALPH", a)?;
            }
            riff::chunk(&mut c, b"VP8 ", &vp8)?;
            Ok(c)
        };
        #[cfg(feature = "vp8l")]
        {
            let lossless = || -> Result<Vec<u8>, EncodeError> {
                let mut c = Vec::new();
                riff::chunk(&mut c, b"VP8L", &crate::lossless::rgba_frame(w, h, rgba)?)?;
                Ok(c)
            };
            match self.anim.frames {
                FrameKind::Lossy => lossy(),
                FrameKind::Lossless => lossless(),
                FrameKind::Smaller => {
                    let a = lossy()?;
                    let b = lossless()?;
                    Ok(if b.len() < a.len() { b } else { a })
                }
            }
        }
        #[cfg(not(feature = "vp8l"))]
        {
            lossy()
        }
    }

    /// Ends the animation. The last frame lasts as long as the one before it
    /// (100 ms if there is only one).
    pub fn finish(self) -> Result<Vec<u8>, EncodeError> {
        let end = match self.frames.as_slice() {
            [] => return Err(EncodeError::NoFrames),
            [only] => only.timestamp.saturating_add(100),
            [.., a, b] => b.timestamp.saturating_add(b.timestamp - a.timestamp),
        };
        self.finish_at(end)
    }

    /// Ends the animation with the last frame shown until `end_ms`, which
    /// must be after its timestamp.
    pub fn finish_at(self, end_ms: u32) -> Result<Vec<u8>, EncodeError> {
        let Some(last) = self.frames.last() else {
            return Err(EncodeError::NoFrames);
        };
        if end_ms <= last.timestamp {
            return Err(EncodeError::TimestampOrder {
                previous: last.timestamp,
                got: end_ms,
            });
        }
        let any_alpha = self.frames.iter().any(|f| f.alpha);
        let flags = riff::FLAG_ANIMATION | self.meta.flags() | if any_alpha { riff::FLAG_ALPHA } else { 0 };
        let total: usize = self.frames.iter().map(|f| f.chunks.len() + 24).sum();
        let mut chunks = Vec::with_capacity(total + 64);
        riff::chunk(&mut chunks, b"VP8X", &riff::vp8x(flags, self.width, self.height))?;
        if let Some(icc) = &self.meta.icc {
            riff::chunk(&mut chunks, b"ICCP", icc)?;
        }
        let [r, g, b, a] = self.anim.background;
        let mut anim = [0u8; 6];
        anim[..4].copy_from_slice(&[b, g, r, a]);
        anim[4..].copy_from_slice(&self.anim.loop_count.to_le_bytes());
        riff::chunk(&mut chunks, b"ANIM", &anim)?;
        for (i, f) in self.frames.iter().enumerate() {
            let next = self.frames.get(i + 1).map_or(end_ms, |n| n.timestamp);
            let duration = next - f.timestamp;
            if duration > MAX_DURATION {
                return Err(EncodeError::DurationTooLong);
            }
            let mut p = Vec::with_capacity(16 + f.chunks.len());
            p.extend_from_slice(&(f.x / 2).to_le_bytes()[..3]);
            p.extend_from_slice(&(f.y / 2).to_le_bytes()[..3]);
            p.extend_from_slice(&(f.w - 1).to_le_bytes()[..3]);
            p.extend_from_slice(&(f.h - 1).to_le_bytes()[..3]);
            p.extend_from_slice(&duration.to_le_bytes()[..3]);
            // Do not blend (bit 1), do not dispose (bit 0 clear).
            p.push(0b10);
            p.extend_from_slice(&f.chunks);
            riff::chunk(&mut chunks, b"ANMF", &p)?;
        }
        riff::trailing_meta(&mut chunks, &self.meta)?;
        riff::file(&chunks)
    }
}

/// The smallest rectangle, at even x and y, holding every pixel that
/// differs between two canvases (pixels transparent in both count as the
/// same). `None` if nothing differs.
fn changed_rect(prev: &[u8], cur: &[u8], width: u32, height: u32) -> Option<(u32, u32, u32, u32)> {
    let w = width as usize;
    let same = |i: usize| {
        let (a, b) = (&prev[i * 4..i * 4 + 4], &cur[i * 4..i * 4 + 4]);
        a == b || (a[3] == 0 && b[3] == 0)
    };
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..height as usize {
        for x in 0..w {
            if !same(y * w + x) {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    if x0 == usize::MAX {
        return None;
    }
    let (x0, y0) = (x0 & !1, y0 & !1);
    Some((x0 as u32, y0 as u32, (x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_rect_is_even_aligned() {
        let (w, h) = (10u32, 8u32);
        let a = vec![0u8; 10 * 8 * 4];
        let mut b = a.clone();
        assert_eq!(changed_rect(&a, &b, w, h), None);
        let i = (5 * 10 + 3) * 4;
        b[i] = 9;
        b[i + 3] = 255;
        assert_eq!(changed_rect(&a, &b, w, h), Some((2, 4, 2, 2)));
        // Transparent in both: unchanged, whatever the colour.
        let mut c = a.clone();
        c[0] = 77;
        assert_eq!(changed_rect(&a, &c, w, h), None);
    }

    #[test]
    fn frames_and_durations() {
        let (w, h) = (24u32, 16u32);
        let mut enc =
            AnimationEncoder::new(w, h, Config::new(60.0).with_method(1), AnimationConfig::default()).expect("valid");
        let frame = |shift: u32| -> Vec<u8> {
            (0..w * h)
                .flat_map(|p| [((p % w) * 10 + shift) as u8, 40, 200, 255])
                .collect()
        };
        enc.add_frame(&frame(0), 0).expect("frame");
        enc.add_frame(&frame(0), 40).expect("same frame merges");
        enc.add_frame(&frame(5), 100).expect("frame");
        assert!(matches!(
            enc.add_frame(&frame(9), 100),
            Err(EncodeError::TimestampOrder { .. })
        ));
        let file = enc.finish_at(250).expect("finishes");
        // Walk the chunks: VP8X, ANIM, two ANMF with 100 and 150 ms.
        let mut pos = 12;
        let mut durations = Vec::new();
        let mut names = Vec::new();
        while pos < file.len() {
            let name = &file[pos..pos + 4];
            let len = u32::from_le_bytes(file[pos + 4..pos + 8].try_into().expect("4")) as usize;
            if name == b"ANMF" {
                let p = &file[pos + 8..];
                durations.push(u32::from_le_bytes([p[12], p[13], p[14], 0]));
            }
            names.push(String::from_utf8_lossy(name).into_owned());
            pos += 8 + len + (len & 1);
        }
        assert_eq!(names, ["VP8X", "ANIM", "ANMF", "ANMF"]);
        assert_eq!(durations, [100, 150]);
        assert_eq!(file[20] & riff::FLAG_ANIMATION, riff::FLAG_ANIMATION);
    }

    #[test]
    fn no_frames_is_an_error() {
        let enc = AnimationEncoder::new(4, 4, Config::default(), AnimationConfig::default()).expect("valid");
        assert!(matches!(enc.finish(), Err(EncodeError::NoFrames)));
        assert!(AnimationEncoder::new(0, 4, Config::default(), AnimationConfig::default()).is_err());
    }
}
