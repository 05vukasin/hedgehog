"""Deterministic choice of the sheets in the European open-data label sample.

Reads `hedgehog describe` for every workbook under <data>/irregular/eu_* and picks, per source
folder, the sheets most likely to carry real table structure, with a fixed tie-break:

- 40..20 000 non-empty cells (enough to be a table, few enough to label carefully);
- sheets with merged ranges first (merged titles / multi-row headers), then sheets of
  150..6 000 cells, then a hash of `file|sheet` (stable, not alphabetical);
- at most one sheet per workbook; two sheets from sources with 30+ candidates, one otherwise;
- sheets rejected after looking at them (see EXCLUDE) are skipped and the next one taken.

Usage: python -I tools/benchmarks/select_eu_sheets.py --data $HEDGEHOG_DATA --bin target/release/hedgehog
Prints one TSV line per chosen sheet: source, file (relative to irregular/), sheet, cells, merges.
"""

import argparse
import hashlib
import re
import subprocess
import sys
from pathlib import Path

# (file, sheet) → why it was skipped after inspection.
EXCLUDE = {
    (
        "eu_at_statistik_austria/002_Bev_2026_Alter_Geschlecht_Gebietseinheiten.ods",
        "'file:///J:/_TEAM/Hilgarth/Website%20NEU/Volkszählung/Dokumente_alte_Website_VZ/VZ_Lebensformen/Lebensformen2.ods'#Tab_1",
    ): "a leftover link sheet: numbers and section labels only, no headers or row labels to label against",
    ("eu_pt_opendata/012_balanco-social-2016.xls", "Balanço Social 2016"): "about forty small forms with "
    "M/F/T row triplets and `T: 12` text cells; one sheet would dominate the sample with one ambiguous convention",
}

LINE = re.compile(r"^(?P<sheet>.*): (?P<range>\S+) · (?P<cells>\d+) cells · \d+ formulas · (?P<merges>\d+) merged ranges$")


def sheets_of(bin_path, path):
    try:
        out = subprocess.run([bin_path, "describe", str(path)], capture_output=True, text=True, timeout=120).stdout
    except subprocess.TimeoutExpired:
        return []
    found = []
    for line in out.splitlines():
        m = LINE.match(line)
        if m:
            found.append((m["sheet"], int(m["cells"]), int(m["merges"])))
    return found


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True)
    ap.add_argument("--bin", required=True)
    args = ap.parse_args()
    root = Path(args.data) / "irregular"
    per_source = {}
    for src in sorted(p for p in root.iterdir() if p.is_dir() and p.name.startswith("eu_") and p.name != "eu_labelled"):
        cands = []
        for f in sorted(src.iterdir()):
            if not f.is_file():
                continue
            rel = f"{src.name}/{f.name}"
            for sheet, cells, merges in sheets_of(args.bin, f):
                if 40 <= cells <= 20_000 and (rel, sheet) not in EXCLUDE:
                    tie = hashlib.sha1(f"{rel}|{sheet}".encode()).hexdigest()
                    key = (merges == 0, not 150 <= cells <= 6_000, tie)
                    cands.append((key, rel, sheet, cells, merges))
        per_source[src.name] = sorted(cands)
    for src, cands in per_source.items():
        want = 2 if len(cands) >= 30 else 1
        used_files = set()
        for _, rel, sheet, cells, merges in cands:
            if len(used_files) == want:
                break
            if rel in used_files:
                continue
            used_files.add(rel)
            print(f"{src}\t{rel}\t{sheet}\t{cells}\t{merges}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
