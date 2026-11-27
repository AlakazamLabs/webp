//! WebP lossless (VP8L) streams from the `image-webp` crate's encoder, for
//! compressed alpha and lossless animation frames (the `vp8l` feature).

use crate::EncodeError;

/// Encodes with `image-webp` and returns the VP8L chunk's payload.
fn vp8l_payload(width: u32, height: u32, data: &[u8], color: image_webp::ColorType) -> Result<Vec<u8>, EncodeError> {
    let mut file = Vec::new();
    image_webp::WebPEncoder::new(&mut file)
        .encode(data, width, height, color)
        .map_err(|e| EncodeError::Lossless(e.to_string()))?;
    crate::riff::find_chunk(&file, b"VP8L")
        .map(<[u8]>::to_vec)
        .ok_or_else(|| EncodeError::Lossless("no VP8L chunk in the lossless output".into()))
}

/// The lossless image stream of an alpha plane, without the VP8L header
/// (signature and size): what the ALPH chunk holds, with alpha in green.
pub(crate) fn alpha_stream(width: u32, height: u32, alpha: &[u8]) -> Result<Vec<u8>, EncodeError> {
    let payload = vp8l_payload(width, height, alpha, image_webp::ColorType::L8)?;
    // The VP8L header: the 0x2f signature, then 14 + 14 + 1 + 3 bits of
    // size, alpha flag and version: 5 bytes, so the stream is byte-aligned.
    if payload.len() < 5 || payload[0] != 0x2f {
        return Err(EncodeError::Lossless("unexpected VP8L header".into()));
    }
    Ok(payload[5..].to_vec())
}

/// A VP8L chunk payload for an RGBA frame.
pub(crate) fn rgba_frame(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, EncodeError> {
    if rgba.chunks_exact(4).all(|p| p[3] == 255) {
        // Opaque: as RGB, so the stream does not claim alpha.
        let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        return vp8l_payload(width, height, &rgb, image_webp::ColorType::Rgb8);
    }
    vp8l_payload(width, height, rgba, image_webp::ColorType::Rgba8)
}
