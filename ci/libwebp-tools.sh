#!/usr/bin/env bash
# Sourced by the oracle scripts: puts libwebp's prebuilt command-line tools
# (cwebp, dwebp, webpinfo, anim_dump) on PATH, from Google's release
# archive, cached in $WEBP_CI_CACHE (default .ci-cache in the repo). Only
# the bin/ folder is unpacked; the tools run as separate processes.
cache="${WEBP_CI_CACHE:-$PWD/.ci-cache}/libwebp"
ver="${LIBWEBP_VERSION:-1.5.0}"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) plat=linux-x86-64 ;;
  Linux-aarch64) plat=linux-aarch64 ;;
  Darwin-arm64) plat=mac-arm64 ;;
  Darwin-x86_64) plat=mac-x86-64 ;;
  *) echo "no libwebp tools for $(uname -s)-$(uname -m)"; exit 1 ;;
esac
dir="$cache/libwebp-$ver-$plat"
if [ ! -x "$dir/bin/dwebp" ]; then
  mkdir -p "$cache"
  curl -fsSL -o "$cache/tools.tar.gz" \
    "https://storage.googleapis.com/downloads.webmproject.org/releases/webp/libwebp-$ver-$plat.tar.gz"
  tar -xzf "$cache/tools.tar.gz" -C "$cache" "libwebp-$ver-$plat/bin"
  rm -f "$cache/tools.tar.gz"
fi
export PATH="$dir/bin:$PATH"
export CWEBP="$dir/bin/cwebp" DWEBP="$dir/bin/dwebp"
"$CWEBP" -version | sed 's/^/cwebp /'
