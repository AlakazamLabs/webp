#!/usr/bin/env bash
# Rate-distortion against cwebp on a published test set: the Kodak
# images (24 PNGs, http://r0k.us/graphics/kodak/), cached in
# $WEBP_CI_CACHE. QUALITIES, METHOD, CWEBP_ARGS, JOBS and SNS tune the
# run (see oracle/src/main.rs). SET=clic uses the CLIC 2020 professional
# validation set instead (download URL in CLIC_URL).
set -euo pipefail
. ci/libwebp-tools.sh
cache="${WEBP_CI_CACHE:-$PWD/.ci-cache}"
set_name="${SET:-kodak}"
imgs="$cache/sets/$set_name"
if [ "$set_name" = kodak ] && [ "$(ls "$imgs" 2>/dev/null | wc -l)" -lt 24 ]; then
  mkdir -p "$imgs"
  for i in $(seq -w 1 24); do
    curl -fsSL -o "$imgs/kodim$i.png" "http://r0k.us/graphics/kodak/kodak/kodim$i.png"
  done
fi
if [ "$set_name" = clic ] && [ ! -f "$imgs/.done" ]; then
  mkdir -p "$imgs"
  curl -fsSL -o "$cache/clic.zip" "${CLIC_URL:?set CLIC_URL to the CLIC zip}"
  (cd "$imgs" && python3 -c "import zipfile,sys; zipfile.ZipFile(sys.argv[1]).extractall('.')" "$cache/clic.zip")
  rm -rf "$imgs/__MACOSX"
  find "$imgs" -mindepth 2 -name '*.png' ! -name '._*' -exec mv {} "$imgs"/ \;
  touch "$imgs/.done"
fi
ls "$imgs"/*.png | wc -l | xargs echo "images:"
cargo build --release -p webp-oracle
"${CARGO_TARGET_DIR:-target}/release/webp-oracle" rd "$imgs" out/rd
