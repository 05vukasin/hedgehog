"""Expand the European open-data label spec (`eu_labels.json`) into a labelled corpus that
`hedgehog benchmark --corpora eu` reads:

    python -I build_eu_labels.py --data $HEDGEHOG_DATA --bin target/release/hedgehog [--spec eu_labels.json]

Cells come from `hedgehog cells <file> --sheet <name>`, i.e. exactly the non-empty cells and
0-based coordinates the benchmark's reader sees. Each sheet's ranges are applied in order (later
ones override); the build fails if a cell is left unlabelled, a range matches no cell, or a sheet
is missing, so the spec and the files cannot drift apart silently.

Writes <data>/irregular/eu_labelled/labels.tsv (corpus, file, sheet, row, col, role). `file` is
relative to eu_labelled/ and points at the original download (`../eu_ch_bfs/...`); `corpus` is
the source folder, which the benchmark reports scores by.
"""

import argparse
import json
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path

ROLES = {"title", "header", "data", "group_header", "total", "note", "other"}
REF = re.compile(r"^([A-Z]+)([0-9]+)$")


def cell_ref(ref):
    m = REF.match(ref)
    if not m:
        raise ValueError(f"bad cell reference `{ref}`")
    col = 0
    for ch in m.group(1):
        col = col * 26 + ord(ch) - 64
    return int(m.group(2)) - 1, col - 1


def rect(rng):
    """`A1:C5` or `B7` → (top, left, bottom, right), 0-based inclusive; `*` → None (everything)."""
    if rng == "*":
        return None
    a, _, b = rng.partition(":")
    (r0, c0), (r1, c1) = cell_ref(a), cell_ref(b or a)
    if r1 < r0 or c1 < c0:
        raise ValueError(f"inverted range `{rng}`")
    return r0, c0, r1, c1


def sheet_cells(bin_path, path, sheet):
    out = subprocess.run([bin_path, "cells", str(path), "--sheet", sheet], capture_output=True, text=True, check=True)
    return [(int(p[1]), int(p[2])) for p in (line.split("\t", 4) for line in out.stdout.splitlines()) if len(p) == 5]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True, help="HEDGEHOG_DATA root (contains irregular/)")
    ap.add_argument("--bin", required=True, help="hedgehog binary")
    ap.add_argument("--spec", default=str(Path(__file__).with_name("eu_labels.json")))
    args = ap.parse_args()

    irregular = Path(args.data) / "irregular"
    out_dir = irregular / "eu_labelled"
    spec = json.loads(Path(args.spec).read_text(encoding="utf-8"))
    errors, lines, roles = [], [], Counter()
    for s in spec["sheets"]:
        where = f"{s['file']} [{s['sheet']}]"
        cells = sheet_cells(args.bin, irregular / s["file"], s["sheet"])
        if not cells:
            errors.append(f"{where}: no cells (missing file or sheet?)")
            continue
        role_of = {}
        for lab in s["labels"]:
            if lab["role"] not in ROLES:
                errors.append(f"{where}: unknown role `{lab['role']}`")
                continue
            r = rect(lab["range"])
            hit = [c for c in cells if r is None or (r[0] <= c[0] <= r[2] and r[1] <= c[1] <= r[3])]
            if not hit:
                errors.append(f"{where}: range {lab['range']} matches no cell")
            for c in hit:
                role_of[c] = lab["role"]
        missing = [c for c in cells if c not in role_of]
        if missing:
            errors.append(f"{where}: {len(missing)} cells unlabelled, first at row {missing[0][0] + 1} col {missing[0][1] + 1}")
        source = s["file"].split("/", 1)[0]
        for (row, col), role in sorted(role_of.items()):
            lines.append(f"{source}\t../{s['file']}\t{s['sheet']}\t{row}\t{col}\t{role}\n")
            roles[role] += 1
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    out_dir.mkdir(parents=True, exist_ok=True)
    with open(out_dir / "labels.tsv", "w", encoding="utf-8") as f:
        f.write("corpus\tfile\tsheet\trow\tcol\trole\n")
        f.writelines(lines)
    sources = len({s["file"].split("/", 1)[0] for s in spec["sheets"]})
    print(f"{out_dir / 'labels.tsv'}: {len(spec['sheets'])} sheets from {sources} sources, {len(lines)} cells")
    print("  " + " · ".join(f"{r} {roles[r]}" for r in sorted(ROLES, key=lambda r: -roles[r])))
    return 0


if __name__ == "__main__":
    sys.exit(main())
