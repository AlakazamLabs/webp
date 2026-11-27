#!/usr/bin/env bash
# Every comparison in one run: Kodak and CLIC rate-distortion against
# cwebp, then the alpha and animation comparisons. Prints the summary
# lines; full logs in out/measure-*.log.
set -uo pipefail
mkdir -p out
(grep -m1 "model name" /proc/cpuinfo 2>/dev/null || sysctl -n machdep.cpu.brand_string) | sed 's/^/cpu: /'
status=0
run() {
  local name=$1; shift
  if ! env "$@" > "out/measure-$name.log" 2>&1; then
    echo "!!! $name failed:"; tail -40 "out/measure-$name.log"; status=1
  fi
  grep "SUMMARY\|ANIM BD\|images:" "out/measure-$name.log" | sed "s/^/$name: /"
}
run kodak bash ci/rd.sh
cp out/rd/rd.tsv out/rd-kodak.tsv 2>/dev/null || true
run clic SET=clic CLIC_URL="${CLIC_URL:-https://data.vision.ee.ethz.ch/cvl/clic/professional_valid_2020.zip}" bash ci/rd.sh
cp out/rd/rd.tsv out/rd-clic.tsv 2>/dev/null || true
run extras bash ci/extras.sh
exit $status
