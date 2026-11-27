#!/usr/bin/env bash
# Checks every table against RFC 6386's prose (Section 20 cut out on
# download).
set -euo pipefail
mkdir -p out
scripts/rfc6386-without-s20.sh out/rfc6386-without-s20.txt
shasum -a 256 out/rfc6386-without-s20.txt 2>/dev/null || sha256sum out/rfc6386-without-s20.txt
python3 scripts/check-tables.py out/rfc6386-without-s20.txt
