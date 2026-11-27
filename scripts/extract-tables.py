#!/usr/bin/env python3
"""Extracts VP8's constant tables from the prose of RFC 6386 (Sections 11.5,
13.4, 13.5 and 14.1) and prints them as Rust.

Input: the RFC's text with Section 20 (Attachment One, the reference
decoder source) removed, as scripts/rfc6386-without-s20.sh writes it;
Section 20 is never read. scripts/check-tables.py checks the result and
every other table against the same text.

    scripts/rfc6386-without-s20.sh rfc.txt
    python3 scripts/extract-tables.py rfc.txt > webp-lossy/src/tables.rs
"""
import re
import sys

text = open(sys.argv[1], encoding="utf-8").read()
if "20.4.  dixie.c" in text or "#include" in text:
    sys.exit("refusing: the input still holds Section 20")

lines = [l for l in text.splitlines()
         if not l.startswith("Bankoski") and not l.startswith("RFC 6386 ")]


def table(name, count):
    """The numbers after `name ... =` up to the end of its code block."""
    for i, l in enumerate(lines):
        if name in l and any("=" in x for x in lines[i:i + 2]) and ";" not in l:
            j = i
            while "=" not in lines[j]:
                j += 1
            nums = []
            # numbers after the '=' on the same line, then following lines
            rest = lines[j].split("=", 1)[1]
            k = j
            while True:
                nums += [int(x) for x in re.findall(r"\b\d+\b", rest)]
                if len(nums) >= count or "End code block" in lines[k + 1]:
                    break
                k += 1
                rest = re.sub(r"/\*.*?\*/", "", lines[k])
            if len(nums) != count:
                sys.exit(f"{name}: {len(nums)} numbers, wanted {count}")
            return nums
    sys.exit(f"{name} not found")


def nested(nums, dims, indent=0):
    if len(dims) == 1:
        return "[" + ", ".join(str(n) for n in nums) + "]"
    step = len(nums) // dims[0]
    pad = " " * (indent + 4)
    inner = [nested(nums[i * step:(i + 1) * step], dims[1:], indent + 4) for i in range(dims[0])]
    return "[\n" + "".join(f"{pad}{x},\n" for x in inner) + " " * indent + "]"


out = []
out.append("//! VP8's constant tables, extracted by `scripts/extract-tables.py` from the")
out.append("//! prose sections of RFC 6386 (11.5, 13.4, 13.5 and 14.1). Generated; do")
out.append("//! not edit by hand. Section 20 of the RFC was not used.")
out.append("")
out.append("/// Key-frame subblock mode probabilities, `[above][left][node]` (RFC 6386 Section 11.5).")
out.append("pub(crate) const KF_BMODE_PROBS: [[[u8; 9]; 10]; 10] = "
           + nested(table("kf_bmode_prob [num_intra_bmodes] [num_intra_bmodes]", 900), [10, 10, 9]) + ";")
out.append("")
out.append("/// Probabilities that a coefficient probability is updated (RFC 6386 Section 13.4).")
out.append("pub(crate) const COEFF_UPDATE_PROBS: [[[[u8; 11]; 3]; 8]; 4] = "
           + nested(table("coeff_update_probs [4] [8] [3]", 1056), [4, 8, 3, 11]) + ";")
out.append("")
out.append("/// The default coefficient probabilities (RFC 6386 Section 13.5).")
out.append("pub(crate) const DEFAULT_COEFF_PROBS: [[[[u8; 11]; 3]; 8]; 4] = "
           + nested(table("default_coeff_probs [4] [8] [3]", 1056), [4, 8, 3, 11]) + ";")
out.append("")
out.append("/// DC quantizer step sizes by index (RFC 6386 Section 14.1).")
out.append("pub(crate) const DC_QUANT: [i32; 128] = " + nested(table("dc_qlookup[QINDEX_RANGE]", 128), [128]) + ";")
out.append("")
out.append("/// AC quantizer step sizes by index (RFC 6386 Section 14.1).")
out.append("pub(crate) const AC_QUANT: [i32; 128] = " + nested(table("ac_qlookup[QINDEX_RANGE]", 128), [128]) + ";")
print("\n".join(out))
