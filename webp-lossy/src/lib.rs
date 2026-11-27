//! A lossy and animated WebP encoder in pure, safe Rust.
//!
//! It writes standard WebP files (RFC 9649): a VP8 key frame (RFC 6386) for
//! the picture, an optional alpha channel, ICC, EXIF and XMP metadata, and
//! animations of lossy frames. It has no dependencies; the optional `vp8l`
//! feature adds compressed alpha and lossless animation frames through the
//! `image-webp` crate's lossless encoder.
//!
//! ```
//! use webp_lossy::{Config, Encoder};
//!
//! // A 64x64 RGBA gradient.
//! let (w, h) = (64u32, 64u32);
//! let mut rgba = Vec::new();
//! for y in 0..h {
//!     for x in 0..w {
//!         rgba.extend_from_slice(&[(x * 4) as u8, (y * 4) as u8, 128, 255]);
//!     }
//! }
//! let webp = Encoder::new(Config::new(80.0)).encode_rgba(w, h, &rgba)?;
//! assert_eq!(&webp[..4], b"RIFF");
//! # Ok::<(), webp_lossy::EncodeError>(())
//! ```
//!
//! See PROVENANCE.md for how this crate was written.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod alpha;
mod anim;
mod bool_encoder;
mod consts;
mod cost;
mod idct;
mod loop_filter;
#[cfg(feature = "vp8l")]
mod lossless;
mod predict;
mod riff;
mod tables;
mod tokens;
mod transform;
mod vp8;
mod yuv;

use std::fmt;

pub use anim::{AnimationConfig, AnimationEncoder, FrameKind};

/// The largest width or height a VP8 frame can have.
pub const MAX_DIMENSION: u32 = 16383;

/// Encoder settings. Start from [`Config::new`] or [`Config::default`]
/// and change the fields you need.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Config {
    /// Quality from 0 (smallest) to 100 (best). Default 75.
    pub quality: f32,
    /// Speed against compression, from 0 (fastest) to 6 (smallest).
    /// Default 4.
    pub method: u8,
    /// Aim for at most this many bytes of file (the encoder searches the
    /// quality; `quality` is then ignored). If even the lowest quality is
    /// larger, that smallest file is returned.
    pub target_size: Option<usize>,
    /// Aim for at least this PSNR, in dB, over Y, U and V together (the
    /// encoder searches the quality; ignored when `target_size` is set).
    pub target_psnr: Option<f32>,
    /// Alpha quality from 0 to 100. Below 100, fewer alpha levels are kept,
    /// which makes compressed alpha (the `vp8l` feature) smaller. Default
    /// 100.
    pub alpha_quality: u8,
    /// Segments, 1 to 4: groups of macroblocks with their own quantizer
    /// and loop-filter level. Default 4.
    pub segments: u8,
    /// Spatial noise shaping, 0 to 100: how much busier segments get a
    /// coarser quantizer, where the eye notices less. 0 spends the bits
    /// evenly (best PSNR). Default 50.
    pub sns_strength: u8,
    /// A fixed loop-filter level, 0 (off) to 63; `None` (the default)
    /// chooses one per segment.
    pub filter_level: Option<u8>,
    /// Loop-filter sharpness, 0 to 7. Default 0.
    pub filter_sharpness: u8,
    /// The simple loop filter (faster to decode, luma only). Default off.
    pub simple_filter: bool,
    /// Keep the colour of fully transparent pixels. Off by default: their
    /// colour is invisible, so it is replaced with values cheaper to code.
    pub exact: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            quality: 75.0,
            method: 4,
            target_size: None,
            target_psnr: None,
            alpha_quality: 100,
            segments: 4,
            sns_strength: 50,
            filter_level: None,
            filter_sharpness: 0,
            simple_filter: false,
            exact: false,
        }
    }
}

impl Config {
    /// The default settings at `quality` (0 to 100).
    pub fn new(quality: f32) -> Self {
        Config {
            quality,
            ..Config::default()
        }
    }

    /// The same settings with `method` (0 to 6).
    pub fn with_method(mut self, method: u8) -> Self {
        self.method = method;
        self
    }

    fn validate(&self) -> Result<(), EncodeError> {
        if !(0.0..=100.0).contains(&self.quality) {
            return Err(EncodeError::InvalidConfig("quality must be 0 to 100"));
        }
        if self.method > 6 {
            return Err(EncodeError::InvalidConfig("method must be 0 to 6"));
        }
        if self.alpha_quality > 100 {
            return Err(EncodeError::InvalidConfig("alpha_quality must be 0 to 100"));
        }
        if !(1..=4).contains(&self.segments) {
            return Err(EncodeError::InvalidConfig("segments must be 1 to 4"));
        }
        if self.sns_strength > 100 {
            return Err(EncodeError::InvalidConfig("sns_strength must be 0 to 100"));
        }
        if self.filter_level.is_some_and(|l| l > 63) {
            return Err(EncodeError::InvalidConfig("filter_level must be 0 to 63"));
        }
        if self.filter_sharpness > 7 {
            return Err(EncodeError::InvalidConfig("filter_sharpness must be 0 to 7"));
        }
        if self.target_psnr.is_some_and(|p| !(p.is_finite() && p > 0.0)) {
            return Err(EncodeError::InvalidConfig("target_psnr must be a positive number"));
        }
        if self.target_size == Some(0) {
            return Err(EncodeError::InvalidConfig("target_size must be above zero"));
        }
        Ok(())
    }
}

/// The quantizer index (0 to 127) for a quality (0 to 100): piecewise
/// linear through points chosen so that a quality gives about the file
/// size cwebp gives at the same number (measured on the Kodak set).
fn quality_to_q(quality: f32) -> u8 {
    const POINTS: [(f32, f32); 9] = [
        (0.0, 90.0),
        (20.0, 50.0),
        (35.0, 39.0),
        (50.0, 31.0),
        (65.0, 24.0),
        (75.0, 20.0),
        (85.0, 10.0),
        (92.0, 4.0),
        (100.0, 0.0),
    ];
    let qv = quality.clamp(0.0, 100.0);
    for w in POINTS.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if qv <= x1 {
            return (y0 + (y1 - y0) * (qv - x0) / (x1 - x0)).round() as u8;
        }
    }
    0
}

/// Why an image could not be encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EncodeError {
    /// A width or height is 0 or above [`MAX_DIMENSION`] (or, for an
    /// animation's canvas, above 2^24 or with more than 2^32 - 1 pixels).
    InvalidDimensions {
        /// The width given.
        width: u32,
        /// The height given.
        height: u32,
    },
    /// A pixel buffer has the wrong length for its size.
    BufferSize {
        /// The length the size needs.
        expected: usize,
        /// The length given.
        actual: usize,
    },
    /// A setting is out of range.
    InvalidConfig(&'static str),
    /// The image is too large for the format: the picture's mode data
    /// outgrew VP8's first partition, or the file outgrew RIFF's 4 GiB.
    TooLarge,
    /// An animation frame's timestamp is not after the previous one's.
    TimestampOrder {
        /// The previous frame's timestamp, in milliseconds.
        previous: u32,
        /// The new frame's timestamp, in milliseconds.
        got: u32,
    },
    /// A frame lasts longer than 2^24 - 1 milliseconds.
    DurationTooLong,
    /// An animation was finished without any frames.
    NoFrames,
    /// The lossless encoder (the `vp8l` feature) failed.
    Lossless(String),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::InvalidDimensions { width, height } => write!(f, "invalid image size {width}x{height}"),
            EncodeError::BufferSize { expected, actual } => {
                write!(f, "pixel buffer is {actual} bytes, the size needs {expected}")
            }
            EncodeError::InvalidConfig(why) => write!(f, "invalid setting: {why}"),
            EncodeError::TooLarge => f.write_str("image too large for the WebP format"),
            EncodeError::TimestampOrder { previous, got } => {
                write!(
                    f,
                    "frame timestamp {got} ms is not after the previous frame's {previous} ms"
                )
            }
            EncodeError::DurationTooLong => f.write_str("a frame lasts longer than 2^24 - 1 ms"),
            EncodeError::NoFrames => f.write_str("the animation has no frames"),
            EncodeError::Lossless(why) => write!(f, "lossless encoder failed: {why}"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// A picture to encode. Pixels are 8-bit and tightly packed, row by row.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub enum Picture<'a> {
    /// Non-premultiplied RGBA, 4 bytes per pixel.
    Rgba {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// `width * height * 4` bytes.
        data: &'a [u8],
    },
    /// RGB, 3 bytes per pixel.
    Rgb {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// `width * height * 3` bytes.
        data: &'a [u8],
    },
    /// Y'CbCr 4:2:0 (Rec. 601, studio range, as WebP decoders assume):
    /// a full-size luma plane and two chroma planes of
    /// `(width + 1) / 2` x `(height + 1) / 2`.
    Yuv420 {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// `width * height` bytes.
        y: &'a [u8],
        /// The Cb plane.
        u: &'a [u8],
        /// The Cr plane.
        v: &'a [u8],
    },
}

impl Picture<'_> {
    fn size(&self) -> (u32, u32) {
        match *self {
            Picture::Rgba { width, height, .. }
            | Picture::Rgb { width, height, .. }
            | Picture::Yuv420 { width, height, .. } => (width, height),
        }
    }
}

/// A 4:2:0 picture: what a decoder makes of an encoded image, before its
/// conversion to RGB.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Yuv420Buf {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Luma, `width * height` bytes.
    pub y: Vec<u8>,
    /// Cb, `(width + 1) / 2` x `(height + 1) / 2` bytes.
    pub u: Vec<u8>,
    /// Cr, the same size as `u`.
    pub v: Vec<u8>,
}

/// An encoded image with the details of how it was made.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Output {
    /// The WebP file.
    pub data: Vec<u8>,
    /// What a conforming decoder makes of the VP8 data: the picture after
    /// the loop filter, in 4:2:0.
    pub reconstruction: Yuv420Buf,
    /// The quantizer index used (0 to 127; lower is finer).
    pub quantizer: u8,
    /// PSNR of `reconstruction` against the encoder's 4:2:0 input, in dB,
    /// over Y, U and V together (`f64::INFINITY` when identical).
    pub psnr: f64,
}

/// Encodes still images.
#[derive(Clone, Debug, Default)]
pub struct Encoder {
    config: Config,
    meta: riff::Meta,
}

impl Encoder {
    /// An encoder with `config`. Settings are checked when encoding.
    pub fn new(config: Config) -> Self {
        Encoder {
            config,
            meta: riff::Meta::default(),
        }
    }

    /// The settings.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Embeds an ICC colour profile (the ICCP chunk).
    pub fn set_icc_profile(&mut self, icc: Vec<u8>) {
        self.meta.icc = Some(icc);
    }

    /// Embeds Exif metadata (the EXIF chunk), passed through unchanged.
    pub fn set_exif(&mut self, exif: Vec<u8>) {
        self.meta.exif = Some(exif);
    }

    /// Embeds XMP metadata (the `XMP ` chunk), passed through unchanged.
    pub fn set_xmp(&mut self, xmp: Vec<u8>) {
        self.meta.xmp = Some(xmp);
    }

    /// Encodes non-premultiplied RGBA (`width * height * 4` bytes). Alpha
    /// is stored only if some pixel is not fully opaque.
    pub fn encode_rgba(&self, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, EncodeError> {
        Ok(self
            .encode(Picture::Rgba {
                width,
                height,
                data: rgba,
            })?
            .data)
    }

    /// Encodes RGB (`width * height * 3` bytes).
    pub fn encode_rgb(&self, width: u32, height: u32, rgb: &[u8]) -> Result<Vec<u8>, EncodeError> {
        Ok(self
            .encode(Picture::Rgb {
                width,
                height,
                data: rgb,
            })?
            .data)
    }

    /// Encodes Y'CbCr 4:2:0 planes (see [`Picture::Yuv420`]).
    pub fn encode_yuv420(&self, width: u32, height: u32, y: &[u8], u: &[u8], v: &[u8]) -> Result<Vec<u8>, EncodeError> {
        Ok(self.encode(Picture::Yuv420 { width, height, y, u, v })?.data)
    }

    /// Encodes `picture` and also returns what a decoder will make of it.
    pub fn encode(&self, picture: Picture<'_>) -> Result<Output, EncodeError> {
        self.config.validate()?;
        let (width, height) = picture.size();
        check_size(width, height)?;
        let prepared = prepare(&picture, self.config.exact)?;
        let (vp8, quantizer) = encode_vp8_searching(&prepared.planes, width, height, &self.config)?;
        let alph = match &prepared.alpha {
            Some(a) => Some(alpha::alph_payload(
                width as usize,
                height as usize,
                a,
                self.config.alpha_quality,
                self.config.method,
            )?),
            None => None,
        };
        let data = riff::still(width, height, &vp8.data, alph.as_deref(), &self.meta)?;
        let reconstruction = crop(&vp8.recon, width, height);
        let psnr = psnr(&prepared.planes, &vp8.recon, width as usize, height as usize);
        Ok(Output {
            data,
            reconstruction,
            quantizer,
            psnr,
        })
    }
}

fn check_size(width: u32, height: u32) -> Result<(), EncodeError> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(EncodeError::InvalidDimensions { width, height });
    }
    Ok(())
}

fn check_len(actual: usize, expected: Option<usize>) -> Result<(), EncodeError> {
    let expected = expected.ok_or(EncodeError::TooLarge)?;
    if actual != expected {
        return Err(EncodeError::BufferSize { expected, actual });
    }
    Ok(())
}

/// A picture in the encoder's form: aligned 4:2:0 planes, and the alpha
/// plane if it has any transparency.
struct Prepared {
    planes: vp8::Planes,
    alpha: Option<Vec<u8>>,
}

fn prepare(picture: &Picture<'_>, exact: bool) -> Result<Prepared, EncodeError> {
    let (width, height) = picture.size();
    let (w, h) = (width as usize, height as usize);
    let pixels = w.checked_mul(h);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    match *picture {
        Picture::Rgba { data, .. } => {
            check_len(data.len(), pixels.and_then(|p| p.checked_mul(4)))?;
            let has_alpha = data.chunks_exact(4).any(|p| p[3] != 255);
            let yuv = yuv::rgb_to_yuv420(w, h, data, 4, has_alpha && !exact);
            let alpha = has_alpha.then(|| data.chunks_exact(4).map(|p| p[3]).collect());
            Ok(Prepared {
                planes: vp8::Planes::from_picture(w, h, &yuv.y, w, &yuv.u, &yuv.v, cw),
                alpha,
            })
        }
        Picture::Rgb { data, .. } => {
            check_len(data.len(), pixels.and_then(|p| p.checked_mul(3)))?;
            let yuv = yuv::rgb_to_yuv420(w, h, data, 3, false);
            Ok(Prepared {
                planes: vp8::Planes::from_picture(w, h, &yuv.y, w, &yuv.u, &yuv.v, cw),
                alpha: None,
            })
        }
        Picture::Yuv420 { y, u, v, .. } => {
            check_len(y.len(), pixels)?;
            check_len(u.len(), Some(cw * ch))?;
            check_len(v.len(), Some(cw * ch))?;
            Ok(Prepared {
                planes: vp8::Planes::from_picture(w, h, y, w, u, v, cw),
                alpha: None,
            })
        }
    }
}

fn params(config: &Config, q: u8, method: u8, segments: u8) -> vp8::Params {
    vp8::Params {
        q,
        method,
        segments,
        sns: config.sns_strength,
        filter_level: config.filter_level,
        simple_filter: config.simple_filter,
        sharpness: config.filter_sharpness,
    }
}

/// Encodes at quantizer `q`. A first partition too large for VP8 is retried
/// with fewer mode bits (no subblock modes, one segment).
fn encode_vp8_at(
    planes: &vp8::Planes,
    width: u32,
    height: u32,
    config: &Config,
    q: u8,
) -> Result<vp8::Encoded, EncodeError> {
    let attempts = [(config.method, config.segments), (0, config.segments), (0, 1)];
    for (method, segments) in attempts {
        match vp8::encode_frame(planes, width, height, &params(config, q, method, segments)) {
            Ok(e) => return Ok(e),
            Err(vp8::FrameError::FirstPartitionTooLarge) => continue,
        }
    }
    Err(EncodeError::TooLarge)
}

/// Encodes at the configured quality, or searches the quantizer for the
/// target size or PSNR.
fn encode_vp8_searching(
    planes: &vp8::Planes,
    width: u32,
    height: u32,
    config: &Config,
) -> Result<(vp8::Encoded, u8), EncodeError> {
    let (w, h) = (width as usize, height as usize);
    let encode = |q: u8| encode_vp8_at(planes, width, height, config, q);
    if let Some(size) = config.target_size {
        // The container header is counted; alpha and metadata are not.
        return finest_passing(encode, |e| e.data.len() + 20 <= size);
    }
    if let Some(t) = config.target_psnr {
        return coarsest_passing(encode, |e| psnr(planes, &e.recon, w, h) >= t as f64);
    }
    let q = quality_to_q(config.quality);
    Ok((encode(q)?, q))
}

/// The finest quantizer (lowest q) whose encoding passes `ok`, for a test
/// that passes from some q up (the file shrinks as q rises). If none
/// passes, the encoding at q 127.
fn finest_passing(
    encode: impl Fn(u8) -> Result<vp8::Encoded, EncodeError>,
    ok: impl Fn(&vp8::Encoded) -> bool,
) -> Result<(vp8::Encoded, u8), EncodeError> {
    let (mut lo, mut hi) = (0i32, 127i32);
    let mut best = None;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let e = encode(mid as u8)?;
        if ok(&e) {
            best = Some((e, mid as u8));
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
    }
    match best {
        Some(b) => Ok(b),
        None => Ok((encode(127)?, 127)),
    }
}

/// The coarsest quantizer (highest q) whose encoding passes `ok`, for a
/// test that passes up to some q (quality falls as q rises). If none
/// passes, the encoding at q 0.
fn coarsest_passing(
    encode: impl Fn(u8) -> Result<vp8::Encoded, EncodeError>,
    ok: impl Fn(&vp8::Encoded) -> bool,
) -> Result<(vp8::Encoded, u8), EncodeError> {
    let (mut lo, mut hi) = (0i32, 127i32);
    let mut best = None;
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let e = encode(mid as u8)?;
        if ok(&e) {
            best = Some((e, mid as u8));
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    match best {
        Some(b) => Ok(b),
        None => Ok((encode(0)?, 0)),
    }
}

fn crop(p: &vp8::Planes, width: u32, height: u32) -> Yuv420Buf {
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let take = |plane: &[u8], stride: usize, w: usize, h: usize| -> Vec<u8> {
        (0..h)
            .flat_map(|r| plane[r * stride..r * stride + w].iter().copied())
            .collect()
    };
    Yuv420Buf {
        width,
        height,
        y: take(&p.y, p.aw, w, h),
        u: take(&p.u, p.aw / 2, cw, ch),
        v: take(&p.v, p.aw / 2, cw, ch),
    }
}

/// PSNR over the visible Y, U and V samples.
fn psnr(a: &vp8::Planes, b: &vp8::Planes, w: usize, h: usize) -> f64 {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut err = 0u64;
    let mut n = 0u64;
    let mut add = |pa: &[u8], pb: &[u8], stride: usize, w: usize, h: usize| {
        for r in 0..h {
            for c in 0..w {
                let d = pa[r * stride + c] as i64 - pb[r * stride + c] as i64;
                err += (d * d) as u64;
            }
        }
        n += (w * h) as u64;
    };
    add(&a.y, &b.y, a.aw, w, h);
    add(&a.u, &b.u, a.aw / 2, cw, ch);
    add(&a.v, &b.v, a.aw / 2, cw, ch);
    if err == 0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 * n as f64 / err as f64).log10()
}

/// Encodes one frame's picture for an animation: its VP8 chunk data and
/// optional ALPH payload.
fn encode_frame_chunks(
    config: &Config,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(Vec<u8>, Option<Vec<u8>>), EncodeError> {
    let prepared = prepare(
        &Picture::Rgba {
            width,
            height,
            data: rgba,
        },
        config.exact,
    )?;
    let (vp8, _) = encode_vp8_searching(&prepared.planes, width, height, config)?;
    let alph = match &prepared.alpha {
        Some(a) => Some(alpha::alph_payload(
            width as usize,
            height as usize,
            a,
            config.alpha_quality,
            config.method,
        )?),
        None => None,
    };
    Ok((vp8.data, alph))
}

// The README's example is compiled as a doc test.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_maps_monotonically() {
        let mut last = 128u32;
        for q in 0..=100 {
            let v = quality_to_q(q as f32) as u32;
            assert!(v <= last);
            last = v;
        }
        assert_eq!(quality_to_q(100.0), 0);
        assert_eq!(quality_to_q(0.0), 90);
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        let e = Encoder::new(Config::default());
        assert_eq!(
            e.encode_rgba(0, 5, &[]),
            Err(EncodeError::InvalidDimensions { width: 0, height: 5 })
        );
        assert_eq!(
            e.encode_rgba(16384, 1, &[]),
            Err(EncodeError::InvalidDimensions {
                width: 16384,
                height: 1
            })
        );
        assert_eq!(
            e.encode_rgb(2, 2, &[0; 11]),
            Err(EncodeError::BufferSize {
                expected: 12,
                actual: 11
            })
        );
        let bad = Encoder::new(Config::new(101.0));
        assert!(matches!(
            bad.encode_rgb(1, 1, &[0; 3]),
            Err(EncodeError::InvalidConfig(_))
        ));
        let nan = Encoder::new(Config::new(f32::NAN));
        assert!(matches!(
            nan.encode_rgb(1, 1, &[0; 3]),
            Err(EncodeError::InvalidConfig(_))
        ));
    }

    #[test]
    fn simple_layout_without_alpha_extended_with_it() {
        let e = Encoder::new(Config::default());
        let rgb = vec![90u8; 8 * 8 * 3];
        let f = e.encode_rgb(8, 8, &rgb).expect("encodes");
        assert_eq!(&f[12..16], b"VP8 ");
        let mut rgba = vec![90u8; 8 * 8 * 4];
        rgba[3] = 0;
        let f = e.encode_rgba(8, 8, &rgba).expect("encodes");
        assert_eq!(&f[12..16], b"VP8X");
        assert!(riff::find_chunk(&f, b"ALPH").is_some());
        let opaque = vec![255u8; 8 * 8 * 4];
        let f = e.encode_rgba(8, 8, &opaque).expect("encodes");
        assert_eq!(&f[12..16], b"VP8 ");
    }

    #[test]
    fn target_size_is_met_when_possible() {
        let (w, h) = (96u32, 64u32);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| ((i * 7919) % 251) as u8).collect();
        let mut c = Config::default().with_method(2);
        c.target_size = Some(3000);
        let out = Encoder::new(c)
            .encode(Picture::Rgb {
                width: w,
                height: h,
                data: &rgb,
            })
            .expect("encodes");
        assert!(
            out.data.len() <= 3000 || out.quantizer == 127,
            "{} bytes at q {}",
            out.data.len(),
            out.quantizer
        );
        let mut c = Config::default().with_method(2);
        c.target_psnr = Some(30.0);
        let out = Encoder::new(c)
            .encode(Picture::Rgb {
                width: w,
                height: h,
                data: &rgb,
            })
            .expect("encodes");
        assert!(out.psnr >= 30.0 || out.quantizer == 0, "{} dB", out.psnr);
    }
}
