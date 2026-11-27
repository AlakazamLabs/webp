#!/usr/bin/env bash
# Kodak rate-distortion against cwebp, then single-threaded speed, on one
# machine (for comparing encoder changes).
set -uo pipefail
mkdir -p out
. ci/libwebp-tools.sh
. ci/sets.sh
(grep -m1 "model name" /proc/cpuinfo 2>/dev/null || sysctl -n machdep.cpu.brand_string) | sed 's/^/cpu: /'
cargo build --release -p webp-oracle > /tmp/build.log 2>&1 || { tail -40 /tmp/build.log; exit 1; }
bin="${CARGO_TARGET_DIR:-target}/release/webp-oracle"
"$bin" rd "$KODAK" out/rd > out/rd.log 2>&1 || { tail -40 out/rd.log; exit 1; }
grep SUMMARY out/rd.log
"$bin" speed "$KODAK"
