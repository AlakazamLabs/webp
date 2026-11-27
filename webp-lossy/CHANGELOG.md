# Changelog

## 0.1.0

First release.

- Lossy WebP from RGBA, RGB or Y'CbCr 4:2:0 input, 1x1 to 16383x16383.
- Quality 0 to 100, method 0 to 6, target file size or target PSNR.
- Rate-distortion mode decision, trellis quantization, up to four
  segments with spatial noise shaping, loop-filter level search, fitted
  token probabilities.
- Alpha (raw; filtered and compressed with the `vp8l` feature), ICC, Exif
  and XMP metadata.
- Animation: changed-rectangle frames, merged repeats; lossy frames, and
  lossless or smaller-of-both frames with the `vp8l` feature.
- `Encoder::encode` returns the decoder's picture, its PSNR and the
  quantizer.
