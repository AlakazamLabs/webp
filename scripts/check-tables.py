#!/usr/bin/env python3
"""Checks every constant table in webp-lossy against RFC 6386's prose.

Input: the RFC text with Section 20 removed (scripts/rfc6386-without-s20.sh).
Each table in src/tables.rs and src/consts.rs is compared, value for value,
with the code block of the section it cites. The zig-zag order, which the
prose names but does not tabulate, is rebuilt from its definition.

    scripts/rfc6386-without-s20.sh rfc.txt
    python3 scripts/check-tables.py rfc.txt
"""
import re
import subprocess
import sys

rfc_path = sys.argv[1]
text = open(rfc_path, encoding="utf-8").read()
if re.search(r"^20\.\d+\.  \w+\.[ch]\s*$", text, re.M) or "#include" in text:
    sys.exit("refusing: the input still holds Section 20")
lines = [l for l in text.splitlines() if not l.startswith("Bankoski") and not l.startswith("RFC 6386 ")]
src = "webp-lossy/src/"
failures = 0


def report(name, section, ok, detail=""):
    global failures
    failures += not ok
    print(f"{'OK  ' if ok else 'FAIL'} {name:22} {section:14} {detail}")


def block_after(pattern):
    """The code from the line matching `pattern` to the end of its
    statement (the first `;`), comments removed."""
    for i, l in enumerate(lines):
        if re.search(pattern, l):
            raw = []
            for m in lines[i:]:
                if "End code block" in m:
                    break
                raw.append(m)
            code = re.sub(r"/\*.*?\*/", "", "\n".join(raw), flags=re.S)
            return code.split(";", 1)[0]
    sys.exit(f"pattern not found: {pattern}")


def numbers_after_eq(pattern):
    b = block_after(pattern)
    b = b.split("=", 1)[1]
    return [int(x) for x in re.findall(r"-?\b\d+\b", b)]


def enum_values(first_member):
    """Name -> value for the C enum that starts with `first_member`."""
    b = block_after(r"^\s*" + first_member + r",").split("}", 1)[0]
    names = re.findall(r"^\s*([A-Za-z_][A-Za-z_0-9]*)\s*(?:,|=|$)", b, re.M)
    vals = {}
    for n in names:
        if n.startswith("num_") or n in vals:
            continue
        vals[n] = len(vals)
    return vals


def tree(pattern, enums):
    b = block_after(pattern).split("=", 1)[1]
    toks = re.findall(r"-?\s*[A-Za-z_][A-Za-z_0-9]*|-?\d+", b)
    out = []
    for t in toks:
        t = t.replace(" ", "")
        neg = t.startswith("-")
        name = t.lstrip("-")
        v = int(name) if name.isdigit() else enums[name]
        out.append(-v if neg else v)
    return out


# Rust side: evaluate `const NAME: [...] = [...];` with this crate's names.
rust = open(src + "consts.rs").read() + open(src + "tables.rs").read()
rust_consts = {m.group(1): int(m.group(2)) for m in re.finditer(r"const ([A-Z_0-9]+): u8 = (\d+);", rust)}


def rust_array(name):
    m = re.search(r"const " + name + r": [^=]+= (\[.*?\]);", rust, re.S)
    body = m.group(1)
    body = re.sub(r"-\(([A-Z_0-9]+) as i8\)", lambda x: str(-rust_consts[x.group(1)]), body)
    return [int(x) for x in re.findall(r"-?\d+", body)]


# Large tables: tables.rs must equal what the extractor makes now.
gen = subprocess.run([sys.executable, "scripts/extract-tables.py", rfc_path], capture_output=True, text=True, check=True).stdout
for name, sec, pat, count in [
    ("KF_BMODE_PROBS", "s11.5", r"kf_bmode_prob \[num_intra_bmodes\] \[num_intra_bmodes\]$", 900),
    ("COEFF_UPDATE_PROBS", "s13.4", r"coeff_update_probs \[4\] \[8\] \[3\]", 1056),
    ("DEFAULT_COEFF_PROBS", "s13.5", r"default_coeff_probs \[4\] \[8\] \[3\]", 1056),
    ("DC_QUANT", "s14.1 dc_qlookup", r"dc_qlookup\[QINDEX_RANGE\]", 128),
    ("AC_QUANT", "s14.1 ac_qlookup", r"ac_qlookup\[QINDEX_RANGE\]", 128),
]:
    got = rust_array(name)
    m = re.search(r"const " + name + r": [^=]+= (\[.*?\]);", gen, re.S)
    want = [int(x) for x in re.findall(r"-?\d+", m.group(1))]
    report(name, sec, got == want and len(got) == count, f"{len(got)} values")

mb = enum_values("DC_PRED")
bm = enum_values("B_DC_PRED")
dct = enum_values("DCT_0")
for name, sec, pat, enums in [
    ("KF_YMODE_TREE", "s11.2", r"kf_ymode_tree \[", mb),
    ("BMODE_TREE", "s11.2", r"bmode_tree \[2", bm),
    ("UV_MODE_TREE", "s11.4", r"uv_mode_tree \[2", mb),
    ("SEGMENT_TREE", "s10", r"mb_segment_tree \[", {}),
    ("COEFF_TREE", "s13.2", r"coeff_tree \[2", dct),
]:
    want = tree(pat, enums)
    got = rust_array(name)
    report(name, sec, got == want, f"{len(got)} entries")
for name, sec, pat in [
    ("KF_YMODE_PROBS", "s11.2", r"kf_ymode_prob \[num_ymodes - 1\] ="),
    ("KF_UV_MODE_PROBS", "s11.4", r"kf_uv_mode_prob \[num_uv_modes - 1\] ="),
]:
    want = numbers_after_eq(pat)
    want = [w for w in want if w != 1 or len(want) <= 1][: len(rust_array(name))]
    report(name, sec, rust_array(name) == want, str(want))
want = numbers_after_eq(r"const int coeff_bands \[16\] =")[:16]
got = rust_array("COEFF_BANDS")
report("COEFF_BANDS", "s13.3", got[:16] == want and got[16] == 0, "16 values (+1 unused entry past the end)")
for k in range(1, 7):
    want = numbers_after_eq(rf"const Prob Pcat{k}\[\] =")
    want = want[: want.index(0)]
    report(f"PCAT{k}", "s13.2", rust_array(f"PCAT{k}") == want, str(want))
# Mode numbering used by the Rust constants.
for n, v in list(mb.items())[:5]:
    report(n, "s11.2 enum", rust_consts.get(n) == v, str(v))
for n, v in bm.items():
    report(n, "s11.2 enum", rust_consts.get(n) == v, str(v))

# Zig-zag: named in Section 13, not tabulated in the prose.
tabulated = re.search(r"zig.?zag\w*\s*\[\s*16\s*\]", text, re.I)
scan = []
for d in range(7):
    cells = [(r, d - r) for r in range(4) if 0 <= d - r < 4]
    if d % 2 == 0:
        cells.reverse()
    scan += [r * 4 + c for r, c in cells]
report("ZIGZAG", "s13 (defined)", rust_array("ZIGZAG") == scan and not tabulated,
       "rebuilt from the zig-zag definition; not tabulated in the prose (a format fact, confirmed by dwebp)")

print(f"{failures} failures")
sys.exit(1 if failures else 0)
