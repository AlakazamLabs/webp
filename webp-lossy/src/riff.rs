//! The WebP container (RFC 9649 Section 2): RIFF chunks, the simple and the
//! extended file layouts.

use crate::EncodeError;

/// The largest RIFF payload (the file size field is at most 2^32 - 10).
const MAX_RIFF: u64 = (1 << 32) - 10;

/// Appends a chunk: FourCC, little-endian size, payload, and a zero pad
/// byte if the size is odd.
pub(crate) fn chunk(out: &mut Vec<u8>, fourcc: &[u8; 4], payload: &[u8]) -> Result<(), EncodeError> {
    let len = u32::try_from(payload.len()).map_err(|_| EncodeError::TooLarge)?;
    out.extend_from_slice(fourcc);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    if len % 2 == 1 {
        out.push(0);
    }
    Ok(())
}

/// Wraps the chunks (already padded) in the RIFF/WEBP header.
pub(crate) fn file(chunks: &[u8]) -> Result<Vec<u8>, EncodeError> {
    let size = chunks.len() as u64 + 4;
    if size > MAX_RIFF {
        return Err(EncodeError::TooLarge);
    }
    let mut out = Vec::with_capacity(chunks.len() + 12);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(chunks);
    Ok(out)
}

/// VP8X feature flags (RFC 9649 Figure 7).
pub(crate) const FLAG_ICC: u8 = 0x20;
pub(crate) const FLAG_ALPHA: u8 = 0x10;
pub(crate) const FLAG_EXIF: u8 = 0x08;
pub(crate) const FLAG_XMP: u8 = 0x04;
pub(crate) const FLAG_ANIMATION: u8 = 0x02;

/// The VP8X chunk payload for a canvas and feature flags.
pub(crate) fn vp8x(flags: u8, width: u32, height: u32) -> [u8; 10] {
    let mut p = [0u8; 10];
    p[0] = flags;
    p[4..7].copy_from_slice(&(width - 1).to_le_bytes()[..3]);
    p[7..10].copy_from_slice(&(height - 1).to_le_bytes()[..3]);
    p
}

/// Metadata chunks carried through.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Meta {
    pub(crate) icc: Option<Vec<u8>>,
    pub(crate) exif: Option<Vec<u8>>,
    pub(crate) xmp: Option<Vec<u8>>,
}

impl Meta {
    pub(crate) fn flags(&self) -> u8 {
        let mut f = 0;
        if self.icc.is_some() {
            f |= FLAG_ICC;
        }
        if self.exif.is_some() {
            f |= FLAG_EXIF;
        }
        if self.xmp.is_some() {
            f |= FLAG_XMP;
        }
        f
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.flags() == 0
    }
}

/// A still image: the simple format when nothing else is needed, else the
/// extended one (VP8X, ICCP, ALPH, VP8, EXIF, XMP in that order).
pub(crate) fn still(
    width: u32,
    height: u32,
    vp8: &[u8],
    alph: Option<&[u8]>,
    meta: &Meta,
) -> Result<Vec<u8>, EncodeError> {
    let mut chunks = Vec::with_capacity(vp8.len() + alph.map_or(0, |a| a.len()) + 64);
    if alph.is_none() && meta.is_empty() {
        chunk(&mut chunks, b"VP8 ", vp8)?;
        return file(&chunks);
    }
    let flags = meta.flags() | if alph.is_some() { FLAG_ALPHA } else { 0 };
    chunk(&mut chunks, b"VP8X", &vp8x(flags, width, height))?;
    if let Some(icc) = &meta.icc {
        chunk(&mut chunks, b"ICCP", icc)?;
    }
    if let Some(a) = alph {
        chunk(&mut chunks, b"ALPH", a)?;
    }
    chunk(&mut chunks, b"VP8 ", vp8)?;
    trailing_meta(&mut chunks, meta)?;
    file(&chunks)
}

/// The EXIF and XMP chunks, which follow the image data.
pub(crate) fn trailing_meta(chunks: &mut Vec<u8>, meta: &Meta) -> Result<(), EncodeError> {
    if let Some(exif) = &meta.exif {
        chunk(chunks, b"EXIF", exif)?;
    }
    if let Some(xmp) = &meta.xmp {
        chunk(chunks, b"XMP ", xmp)?;
    }
    Ok(())
}

/// The payload of the first chunk named `fourcc` among a RIFF/WEBP file's
/// top-level chunks.
#[cfg_attr(not(any(test, feature = "vp8l")), allow(dead_code))]
pub(crate) fn find_chunk<'a>(file: &'a [u8], fourcc: &[u8; 4]) -> Option<&'a [u8]> {
    if file.len() < 12 || &file[..4] != b"RIFF" || &file[8..12] != b"WEBP" {
        return None;
    }
    let mut pos = 12;
    while pos + 8 <= file.len() {
        let len = u32::from_le_bytes(file[pos + 4..pos + 8].try_into().ok()?) as usize;
        let start = pos + 8;
        let end = start.checked_add(len)?;
        if end > file.len() {
            return None;
        }
        if &file[pos..pos + 4] == fourcc {
            return Some(&file[start..end]);
        }
        pos = end + (len & 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odd_chunks_are_padded_and_sizes_add_up() {
        let mut c = Vec::new();
        chunk(&mut c, b"ABCD", &[1, 2, 3]).expect("fits");
        assert_eq!(c, [b'A', b'B', b'C', b'D', 3, 0, 0, 0, 1, 2, 3, 0]);
        let f = file(&c).expect("fits");
        assert_eq!(u32::from_le_bytes(f[4..8].try_into().expect("4")), 4 + 12);
        assert_eq!(find_chunk(&f, b"ABCD"), Some(&[1u8, 2, 3][..]));
    }

    #[test]
    fn vp8x_layout() {
        let p = vp8x(FLAG_ALPHA | FLAG_ICC, 400, 300);
        assert_eq!(p[0], 0x30);
        assert_eq!(&p[1..4], &[0, 0, 0]);
        assert_eq!(&p[4..7], &[0x8f, 0x01, 0x00]);
        assert_eq!(&p[7..10], &[0x2b, 0x01, 0x00]);
    }

    #[test]
    fn extended_order() {
        let meta = Meta {
            icc: Some(vec![9; 3]),
            exif: Some(vec![8; 2]),
            xmp: Some(vec![7; 1]),
        };
        let f = still(2, 2, &[1, 2, 3, 4], Some(&[0, 5]), &meta).expect("fits");
        let mut order = Vec::new();
        let mut pos = 12;
        while pos < f.len() {
            order.push(String::from_utf8_lossy(&f[pos..pos + 4]).into_owned());
            let len = u32::from_le_bytes(f[pos + 4..pos + 8].try_into().expect("4")) as usize;
            pos += 8 + len + (len & 1);
        }
        assert_eq!(order, ["VP8X", "ICCP", "ALPH", "VP8 ", "EXIF", "XMP "]);
        assert_eq!(f[20], FLAG_ICC | FLAG_ALPHA | FLAG_EXIF | FLAG_XMP);
    }
}
