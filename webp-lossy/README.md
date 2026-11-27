# webp-lossy

A lossy and animated WebP encoder in pure, safe Rust (`#![forbid(unsafe_code)]`).
It writes standard WebP files (RFC 9649): VP8 key frames (RFC 6386) with
rate-distortion optimisation, an optional alpha channel, ICC, Exif and XMP
metadata, and animations. No dependencies by default.

```toml
[dependencies]
webp-lossy = "0.1"
# or, for compressed alpha and lossless animation frames:
# webp-lossy = { version = "0.1", features = ["vp8l"] }
```

The crate is imported as `webp_lossy`. Without the `vp8l` feature, alpha
is stored uncompressed (about one byte per pixel on top of the VP8 data),
so turn `vp8l` on for any image with transparency.

```rust
use webp_lossy::{Config, Encoder};

fn to_webp(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, webp_lossy::EncodeError> {
    let mut config = Config::new(80.0); // quality 0 to 100
    config.method = 4; // 0 (fastest) to 6 (smallest)
    Encoder::new(config).encode_rgba(width, height, rgba)
}

let rgba = vec![200u8; 64 * 48 * 4];
let webp = to_webp(64, 48, &rgba)?;
assert_eq!(&webp[8..12], b"WEBP");
# Ok::<(), webp_lossy::EncodeError>(())
```

Animations take one full RGBA canvas per frame with its timestamp:

```rust
use webp_lossy::{AnimationConfig, AnimationEncoder, Config};

let mut settings = AnimationConfig::default();
settings.loop_count = 2; // play twice, then stop; 0 (the default) loops forever

let mut anim = AnimationEncoder::new(32, 32, Config::new(75.0), settings)?;
for i in 0..3u32 {
    let frame = vec![(i * 80) as u8; 32 * 32 * 4];
    anim.add_frame(&frame, i * 100)?; // milliseconds
}
let webp = anim.finish_at(300)?; // the last frame is shown until 300 ms
assert_eq!(&webp[8..12], b"WEBP");
# Ok::<(), webp_lossy::EncodeError>(())
```

- **Timestamps** are in milliseconds and must increase. Only the
  differences matter: each frame is shown until the next frame's
  timestamp, and the first frame is shown from the start of the
  animation, whatever its timestamp (so nothing is shown "before" it).
  `finish` gives the last frame the same duration as the one before it
  (100 ms if there is only one); `finish_at` sets when it ends.
- **`loop_count`** is written as given into the ANIM chunk: the number of
  times the animation plays (2 plays it twice), and 0 loops forever.
- **Lossless frames** (`FrameKind::Lossless` or `FrameKind::Smaller`)
  need the `vp8l` feature. Without it, `AnimationEncoder::new` returns
  `EncodeError::InvalidConfig`.

[`examples/image_crate.rs`](https://github.com/AlakazamLabs/webp/blob/main/webp-lossy/examples/image_crate.rs)
encodes anything the `image` crate reads.

## What it does

- **Input:** 8-bit RGBA, RGB, or Y'CbCr 4:2:0 (Rec. 601, studio range),
  any size from 1x1 to 16383x16383.
- **Settings (`Config`):** quality 0 to 100 (calibrated to give about the
  file size cwebp gives at the same number), method 0 to 6, a target file
  size or a target PSNR (searched over the quantizer), up to 4 segments,
  spatial noise shaping, loop-filter level and sharpness, the simple
  filter, and alpha quality.
- **Encoder:** rate-distortion choice of the luma, subblock and chroma
  modes; trellis quantization (method 3 and up); segments with their own
  quantizer and filter level; a search for the filter levels; token
  probabilities fitted to the picture, in up to three passes.
- **Alpha:** the ALPH chunk, written only if some pixel is not fully
  opaque. Without the `vp8l` feature it is stored uncompressed, one byte
  per pixel; with `vp8l` it is filtered and compressed. Invisible (fully
  transparent) pixels get colours that are cheaper to code, unless `exact`
  is set.
- **Metadata:** ICC, Exif and XMP chunks, embedded exactly as given: they
  are not parsed or checked.
- **Animation:** ANIM and ANMF chunks; only the rectangle that changed is
  encoded, and repeated frames are merged. Lossy frames, or with the `vp8l`
  feature lossless frames or the smaller of the two.
- `Encoder::encode` also returns the picture a decoder will make of the
  file (4:2:0), the quantizer used, and `Output.psnr`: the PSNR of that
  picture against the encoder's own 4:2:0 conversion of the input (not
  against the source RGB).
- `target_size` counts the VP8 data and the container header; alpha and
  metadata come on top.

**Not included:** decoding (see `image-webp`), a lossless encoder of its
own, SIMD, threads.

Invalid input returns an error: a zero or oversized dimension, a pixel
buffer of the wrong length, an out-of-range setting or an out-of-order
animation frame is an `EncodeError` (a still's settings are checked when
it is encoded, an animation's by `AnimationEncoder::new`). The encoder is
designed not to panic, and fuzzing has found no panic (see below); a
panic is a bug, so please report it. An allocation failure for a very
large picture can still abort the process.

**Untrusted callers:** memory and time grow with the pixel count and the
method. A 16383x16383 RGBA picture is about 1 GiB of input and takes
minutes of CPU at method 6, and `target_size` or `target_psnr` runs
several encodes. Cap the dimensions and method you accept. ICC, Exif and
XMP are embedded as given, so strip what you don't want to publish (Exif
can hold a GPS position) before passing them on.

## Features

| Feature | What it turns on |
|---|---|
| `vp8l` (optional, off by default) | Compressed alpha, and lossless animation frames (`FrameKind`), through the `image-webp` crate's lossless encoder. |

## How it is checked

- **Quality, measured** (not promised; `ci/measure.sh` in the repository):
  rate-distortion against libwebp 1.5.0's `cwebp -m 4` (other options at
  their defaults), this crate at its default method 4, qualities 20, 35, 50,
  65, 75, 85 and 92 for both, both decoded by `dwebp`, metrics against the
  source RGB. Bjontegaard rate difference of this crate against cwebp
  (negative: this crate's files are smaller at equal quality):

  | Test set | PSNR | SSIM | SSIMULACRA2 |
  |---|---|---|---|
  | Kodak (24 images) | -1.8% | -3.5% | -1.3% |
  | CLIC 2020 professional validation (41 images) | -0.01% | -5.9% | -1.9% |

  Animation (8 clips of 24 frames, 320x240, qualities 35 to 85): -7.1%
  (PSNR) against `img2webp -lossy -m 4`. Compressed alpha (`vp8l`) keeps
  every alpha value exactly; made by `image-webp`'s lossless encoder, it
  is about twice the size of cwebp's.
- **Speed, measured:** one thread on an Apple M1, the Kodak set at quality
  75, in megapixels per second:

  | Method | This crate | cwebp |
  |---|---|---|
  | 0 | 22 | 38 |
  | 4 | 2.4 | 12.9 |
  | 6 | 0.6 | 6.5 |

  cwebp was timed as a process that also reads a PPM; it uses SIMD, which
  this crate leaves out.
- **Checked with three decoders:** a corpus of 739 files (stills from 1x1
  to 640x480 at every method 0 to 6, qualities 0 to 100, 1, 3 and 4
  segments, both loop filters, several sharpness and fixed filter levels,
  4:2:0 input, alpha and metadata, plus four animations) decodes in
  libwebp 1.5.0's `dwebp` (also incrementally) to exactly the encoder's
  own reconstruction, in `image-webp` 0.2 to exactly dwebp's pixels, and
  in Chromium 131 (WebCodecs) to exactly libwebp's pixels; libwebp's
  `webpinfo` finds no problem in any file.
- **Fuzzed:** with cargo-fuzz on three targets (pictures of 1 to 48
  pixels a side with every setting, the target searches, and animations
  with fuzzed timestamps); a finding becomes a test in
  `tests/edge_cases.rs`. About a billion executions in total found no
  crash, panic, timeout or out-of-memory. `#![forbid(unsafe_code)]`, and
  every size is checked before anything is allocated.

## MSRV and platforms

Rust 1.80.1 or later (checked with `cargo check` on default and all features; the tests and the example use newer dev-dependencies). Pure Rust with no `unsafe` and no platform code, so any
target Rust supports.

## Licence

`MIT OR Apache-2.0` ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
[PROVENANCE.md](https://github.com/AlakazamLabs/webp/blob/main/webp-lossy/PROVENANCE.md)
records how the crate was written.

This is an independent implementation of the WebP format; no patent licence
is granted beyond Apache-2.0's grant for our own contributions.

## Issues and security

Bugs and requests: [open an issue](https://github.com/AlakazamLabs/webp/issues/new?labels=webp-lossy&template=webp-lossy.yml).

Please report vulnerabilities privately, as [SECURITY.md](https://github.com/AlakazamLabs/webp/blob/main/SECURITY.md)
describes; never in a public issue.
