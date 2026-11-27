#!/usr/bin/env bash
# Alpha and animation comparisons on the Kodak set (fetched by ci/rd.sh):
# ours against cwebp (alpha) and img2webp -lossy (animation).
set -euo pipefail
. ci/libwebp-tools.sh
imgs="${WEBP_CI_CACHE:-$PWD/.ci-cache}/sets/kodak"
[ "$(ls "$imgs"/*.png 2>/dev/null | wc -l)" -ge 24 ] || { echo "run ci/rd.sh first for the Kodak set"; exit 1; }
cargo build --release -p webp-oracle
bin="${CARGO_TARGET_DIR:-target}/release/webp-oracle"
"$bin" alpha "$imgs" out/alpha
"$bin" anim "$imgs" out/anim
