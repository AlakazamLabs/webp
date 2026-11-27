#!/usr/bin/env bash
# Sourced: fetches the Kodak test images (24 PNGs, http://r0k.us/graphics/kodak/)
# into $WEBP_CI_CACHE/sets/kodak and sets KODAK to that folder.
KODAK="${WEBP_CI_CACHE:-$PWD/.ci-cache}/sets/kodak"
if [ "$(ls "$KODAK"/kodim*.png 2>/dev/null | wc -l)" -lt 24 ]; then
  mkdir -p "$KODAK"
  for i in $(seq -w 1 24); do
    curl -fsSL --retry 3 -o "$KODAK/kodim$i.png" "http://r0k.us/graphics/kodak/kodak/kodim$i.png"
  done
fi
export KODAK
