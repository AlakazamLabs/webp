# Provenance

How this crate was written, recorded so its licensing can be checked. The
crate is `MIT OR Apache-2.0` only: it contains no BSD-3-Clause code.

## Sources used

- **RFC 9649** (WebP), as text: the container (RIFF, VP8, VP8X, ALPH,
  ANIM, ANMF, ICCP, EXIF and XMP chunks), the alpha filters and the canvas
  assembly rules.
- **RFC 6386** (VP8), Sections 1 to 19 and 21 only: the prose, its tables and
  the inline descriptions of the decoding process. **Section 20 (Attachment
  One, the reference decoder source, BSD-3-Clause) was removed from the
  text before it was used and was never read.** The text was downloaded
  from rfc-editor.org and cut at the heading of Section 20 by a script, so
  the attachment was never displayed. The cut-out script is
  `scripts/rfc6386-without-s20.sh` (sha256
  `5764c878f4c7f2aa3efcfb68fb034d3491b1156fc2c8266cd5254175eb56398b`); the
  text it writes has sha256
  `f160b6875f4dc2d4db53865535dec034258becdd8853ae073797c56322706aa6`.
- General literature: the Rec. 601 colour matrix, rate-distortion
  optimisation (Lagrangian mode decision, trellis quantization), k-means
  clustering, the Bjontegaard rate difference.

## Reused from our own vp8-encoder

vp8-encoder (crates/vp8-encoder in the K2 repository) records in its
PROVENANCE.md that its encoder design is its own and that the parts derived
from RFC 6386 Section 20 live in vp8-decoder, which it depends on. Only
vp8-encoder's own files were reused; nothing was taken from vp8-decoder
(its source was not opened for this crate).

| File here | From vp8-encoder | Status there | Change |
|---|---|---|---|
| `src/transform.rs` | `src/transform.rs` | own work (forward DCT and WHT as transposes of the RFC inverses, quantization) | imports point at this crate's tables and inverse transforms |
| `src/tokens.rs` | `src/tokens.rs` | own work (the token writer and counter) | imports point at this crate's constants |
| `src/cost.rs` `bit_cost`, `src/vp8.rs` `fit_probabilities`, `copy_padded`, the frame-tag writing | `src/encoder.rs` | own work | adapted to key frames |

None of these files holds a table of its own. In vp8-encoder they took
their tables from vp8-decoder (whose tables were extracted from Section
20); here they use only this crate's tables, listed below and checked
against the RFC's prose.

## Rewritten from the RFC's prose (parts that vp8-encoder took from vp8-decoder)

| File | Written from |
|---|---|
| `src/bool_encoder.rs` | Section 7 (7.1 to 7.3): the coder's description and its inline encoder and decoder. The test-only decoder follows Section 7.3 too. |
| `src/predict.rs` | Section 12: the edge values (127 above, 129 left), the DC, V, H and TM modes, the ten subblock modes and the above-right rule. |
| `src/idct.rs` | Sections 14.3 and 14.4: the inverse WHT and DCT with 16-bit intermediates. |
| `src/loop_filter.rs` | Section 15: the simple, subblock and macroblock filters, the edge order and the control parameters. |

## Tables

No table comes from vp8-decoder's `tables.rs` or from any file whose
PROVENANCE says Section 20. Every table is transcribed from the RFC's prose
sections, and `scripts/check-tables.py` checks each one, value for value,
against the text with Section 20 cut out (it refuses text that still holds
it):

| Table | Source |
|---|---|
| `DC_QUANT`, `AC_QUANT` | Section 14.1, `dc_qlookup`, `ac_qlookup` |
| `DEFAULT_COEFF_PROBS` | Section 13.5, `default_coeff_probs` |
| `COEFF_UPDATE_PROBS` | Section 13.4, `coeff_update_probs` |
| `KF_BMODE_PROBS` | Section 11.5, `kf_bmode_prob` |
| `KF_YMODE_TREE`, `KF_YMODE_PROBS`, `BMODE_TREE`, the mode numbering | Section 11.2 |
| `UV_MODE_TREE`, `KF_UV_MODE_PROBS` | Section 11.4 |
| `SEGMENT_TREE` | Section 10, `mb_segment_tree` |
| `COEFF_TREE`, `PCAT1` to `PCAT6` | Section 13.2 (`coeff_tree`, `Pcat1` to `Pcat6`) |
| `COEFF_BANDS` | Section 13.3, `coeff_bands` |

The large tables (`src/tables.rs`) are generated from the same text by
`scripts/extract-tables.py`; the others (`src/consts.rs`) were typed from
it. Tests check that the token writer and the cost tables follow
`COEFF_TREE` exactly.

Format facts not tabulated in the prose, confirmed black-box:
- **The zig-zag scan order** (`ZIGZAG`): Section 13 names "a so-called
  zig-zag order" without the table. It is built from the zig-zag definition
  (alternate anti-diagonals, starting right), which the checker rebuilds.
- **Dequantization scaling:** the Y2 and chroma dequantization scaling
  isn't stated in RFC 6386's prose. It comes with `src/transform.rs`,
  reused from vp8-encoder's `src/transform.rs`, which marks it as its own
  work (see the table above); nothing was taken from vp8-decoder. The
  values were confirmed against dwebp's output.

dwebp decodes every file of the oracle corpus (qualities 0 to 100, every
method and segment setting) to exactly the encoder's own reconstruction,
which confirms both.

## Our own design

Everything that is an encoder decision: the rate-distortion mode decision
for the luma, subblock and chroma modes; the trellis quantizer; the
segmentation by local activity and the per-segment quantizers (spatial
noise shaping); the loop-filter level search; the token-probability passes;
the quality-to-quantizer mapping; the target-size and target-PSNR searches;
the RGB to 4:2:0 conversion and the treatment of invisible pixels; the
alpha level reduction and filter choice; the animation's changed-rectangle
search; the container writer.

## Dependencies

- None by default.
- The optional `vp8l` feature uses the `image-webp` crate (MIT OR
  Apache-2.0) through its public API, to make VP8L streams for compressed
  alpha and lossless animation frames. Its source was not read; the API was
  learnt from the compiler.

## Outside tools used in testing (never as code)

These run only as separate processes or as black-box libraries in the
unpublished `oracle/` tool. Their output is compared; their source was not
opened, and none of it is in this repository.

- libwebp 1.5.0's `cwebp`, `dwebp`, `webpinfo`, `anim_dump` and
  `img2webp`, the prebuilt binaries from Google's release archive (only
  `bin/` unpacked): decoding (plain and incremental), validation, and the
  still, alpha and animation comparisons.
- `image-webp` 0.2 as a decoder, in `oracle/`.
- Chromium 131 (Playwright 1.49.1), through WebCodecs' `ImageDecoder`.
- The `ssimulacra2` and `png` crates in `oracle/`, for metrics and test
  images.
- Test images: the Kodak set and the CLIC 2020 professional validation
  set, downloaded at test time by `ci/rd.sh`; not in the repository or the
  package.
- **The padding byte:** one zero byte at the end of the first partition
  (`src/vp8.rs`, commented there). Found by testing against libwebp's
  incremental decoder (`dwebp -incremental`) and Chromium, which refuse a
  first partition that ends right after the coder's flushed bytes; the
  RFC's prose and libwebp's non-incremental decoder accept either.

## Not used

No WebP or VP8 encoder or decoder source was opened: not libwebp, not
libvpx, not image-webp, not the `webp` crate, not zenwebp, not webp-rust,
not FFmpeg, and not our own vp8-decoder (whose code ports RFC 6386
Section 20).

## Development history

This repository starts from a single commit of the finished crate. The
development history (every commit, with the test and measurement records
behind the numbers in the README) is kept privately and is available on
request.

## Statement

The author of this crate followed these rules throughout: the licence is
MIT OR Apache-2.0 only; only vp8-encoder files marked as our own work were
reused; every part that vp8-encoder took from Section 20 was rewritten
from the RFC's prose and tables, as listed above; the container was
written from RFC 9649 alone; and outside tools were used only as oracles.

Nothing here grants any patent licence beyond what Apache-2.0 grants for
our own contributions.
