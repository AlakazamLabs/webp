#!/usr/bin/env bash
# Encodes the oracle corpus, decodes it with libwebp's dwebp (to 4:2:0 and
# to RGBA) and anim_dump, validates it with webpinfo, and checks that
# dwebp's 4:2:0 equals the encoder's reconstruction and that image-webp
# decodes every file to exactly dwebp's pixels. Output in out/corpus.
set -euo pipefail
. ci/libwebp-tools.sh
cargo build --release -p webp-oracle
bin="${CARGO_TARGET_DIR:-target}/release/webp-oracle"
rm -rf out/corpus && mkdir -p out/corpus
"$bin" corpus out/corpus
bad=0
while IFS=$'\t' read -r name kind w h frames; do
  f="out/corpus/$name.webp"
  if ! webpinfo -quiet "$f" > "out/corpus/$name.webpinfo" 2>&1; then
    echo "webpinfo FAIL $name:"
    tail -20 "out/corpus/$name.webpinfo" | sed 's/^/    /'
    bad=1
  fi
  if [ "$kind" = still ]; then
    dwebp -quiet "$f" -yuv -o "out/corpus/$name.dwebp.yuv" || { echo "dwebp -yuv FAIL $name"; bad=1; }
    dwebp -quiet "$f" -pam -o "out/corpus/$name.dwebp.pam" || { echo "dwebp -pam FAIL $name"; bad=1; }
    # Incremental decoding (as browsers do) must give the same picture.
    if ! dwebp -quiet -incremental "$f" -yuv -o "out/corpus/$name.inc.yuv" 2> "out/corpus/$name.inc.log"; then
      echo "dwebp -incremental FAIL $name: $(tr '\n' ' ' < "out/corpus/$name.inc.log")"
      webpinfo -bitstream_info "$f" | sed 's/^/    /' | tail -30
      bad=1
    elif ! cmp -s "out/corpus/$name.inc.yuv" "out/corpus/$name.dwebp.yuv"; then
      echo "dwebp -incremental DIFFERS $name"
      bad=1
    fi
  else
    mkdir -p "out/corpus/$name.anim"
    anim_dump -folder "out/corpus/$name.anim" -pam "$f" > /dev/null || { echo "anim_dump FAIL $name"; bad=1; }
  fi
done < out/corpus/manifest.tsv
status=0
"$bin" compare out/corpus || status=$?
echo "webpinfo/dwebp/anim_dump problems: $bad"
[ "$bad" = 0 ] || status=1
exit $status
