#!/usr/bin/env bash
# Browser check: runs ci/oracles.sh's corpus (MEDIA_DIR, default
# out/corpus, with the dwebp and anim_dump outputs) through Playwright's
# Chromium (cached in $WEBP_CI_CACHE, default .ci-cache in the repo):
# every file must decode, with libwebp's pixels.
set -euo pipefail
cache="${WEBP_CI_CACHE:-$PWD/.ci-cache}"
export MEDIA_DIR="$(cd "${MEDIA_DIR:-out/corpus}" && pwd)"
cd ci/browser
export PLAYWRIGHT_BROWSERS_PATH="$cache/pw-browsers"
npm install --no-audit --no-fund --silent
npx playwright install chromium
ls "$MEDIA_DIR"/*.webp | wc -l | xargs echo "files:"
node check.mjs
