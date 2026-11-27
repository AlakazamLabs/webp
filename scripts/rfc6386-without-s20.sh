#!/usr/bin/env bash
# Downloads RFC 6386 and writes it with Section 20 (Attachment One, the
# reference decoder source, BSD-3-Clause) removed: everything from the
# heading "20.  Attachment One" up to the heading "21.  Security
# Considerations" is dropped before the text is saved, so the attachment
# is never displayed. Usage: scripts/rfc6386-without-s20.sh OUT.txt
set -euo pipefail
out="${1:?usage: $0 OUT.txt}"
curl -fsSL https://www.rfc-editor.org/rfc/rfc6386.txt | awk '
  /^20\.  Attachment One/ { skip = 1; print ""; print "[Section 20 (Attachment One, reference decoder source) removed on download: not used.]"; print ""; next }
  /^21\.  Security Considerations/ { skip = 0 }
  !skip { print }
' > "$out"
if grep -q "^20\.[0-9]*\.  [a-z_]*\.[ch] *$\|#include" "$out"; then
  echo "Section 20 still present in $out" >&2
  exit 1
fi
echo "wrote $out ($(wc -l < "$out") lines)"
