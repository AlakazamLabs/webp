#!/usr/bin/env bash
# Single-threaded encode speed on the Kodak set: ours (in process) and
# cwebp (a process reading a PPM), quality 75, every method, best of REPS.
set -euo pipefail
. ci/libwebp-tools.sh
. ci/sets.sh
(grep -m1 "model name" /proc/cpuinfo 2>/dev/null || sysctl -n machdep.cpu.brand_string) | sed 's/^/cpu: /'
cargo build --release -p webp-oracle
"${CARGO_TARGET_DIR:-target}/release/webp-oracle" speed "$KODAK"
