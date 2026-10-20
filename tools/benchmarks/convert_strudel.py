"""Convert the Strudel corpora (SAUS, CIUS, DeEx, Troy; `*.jl.gz`, one table per line) into real
.xlsx files plus a `labels.tsv` that `hedgehog eval labelled --dir <out>` reads.

    python convert_strudel.py <strudel_dir> <out_dir>

Strudel ships each table as a grid of strings with per-cell features from the original xls file.
Numbers (`cell_type=2`) are written as numbers, everything else as text; bold, indentation, font
size and top/bottom borders are kept as cell formats. Cell labels map to hedgehog roles:
metadata → title, header → header, group → group_header, data → data, derived → total,
notes → note; empty and unlabelled cells are not written. Tables with fewer feature rows than
value rows get plain cells there (numbers detected from the text).

labels.tsv columns: corpus, file (relative to out_dir), sheet, row, col, role (0-based row/col).
"""

from __future__ import annotations

import gzip
import json
import sys
from pathlib import Path

import xlsxwriter

ROLE = {"metadata": "title", "header": "header", "group": "group_header", "data": "data",
        "derived": "total", "notes": "note"}
SHEET = "table"


def number(s: str) -> float | None:
    try:
        return float(s.replace(",", ""))
    except ValueError:
        return None


def convert(src: Path, out: Path, labels) -> tuple[int, int]:
    corpus = src.name.split(".")[0]
    (out / corpus).mkdir(parents=True, exist_ok=True)
    tables = cells = 0
    with gzip.open(src, "rt", encoding="utf-8") as fh:
        for k, line in enumerate(fh):
            r = json.loads(line)
            names = r["feature_names"]
            idx = {n: i for i, n in enumerate(names)}
            rel = f"{corpus}/{k:04d}.xlsx"
            wb = xlsxwriter.Workbook(str(out / rel), {"strings_to_numbers": False})
            ws = wb.add_worksheet(SHEET)
            formats: dict = {}
            for i, row in enumerate(r["table_array"]):
                for j, text in enumerate(row):
                    if text is None or text == "":
                        continue
                    fa = r["feature_array"]
                    f = fa[i][j] if i < len(fa) and j < len(fa[i]) else None
                    if f is None:  # a few tables have fewer feature rows than value rows
                        num = number(text)
                        if num is not None:
                            ws.write_number(i, j, num)
                        else:
                            ws.write_string(i, j, text)
                    key = None if f is None else (
                        bool(f[idx["is_bold"]]),
                        int(f[idx["indentation"]]) if "indentation" in idx else 0,
                        int(f[idx["font_height"]]) if "font_height" in idx else 0,
                        "border_top_type=0" in idx and not f[idx["border_top_type=0"]],
                        "border_bottom_type=0" in idx and not f[idx["border_bottom_type=0"]],
                    )
                    if key is not None and key not in formats:
                        bold, indent, height, top, bottom = key
                        props = {"bold": bold, "indent": min(indent, 15)}
                        if height:
                            props["font_size"] = height / 20  # xls font height is in twips
                        if top:
                            props["top"] = 1
                        if bottom:
                            props["bottom"] = 1
                        formats[key] = wb.add_format(props)
                    if key is not None:
                        fmt = formats[key]
                        num = number(text) if f[idx["cell_type=2"]] else None
                        if num is not None:
                            ws.write_number(i, j, num, fmt)
                        else:
                            ws.write_string(i, j, text, fmt)
                    role = ROLE.get(r["annotations"][i][j] or "")
                    if role:
                        labels.write(f"{corpus}\t{rel}\t{SHEET}\t{i}\t{j}\t{role}\n")
                        cells += 1
            wb.close()
            tables += 1
    return tables, cells


def main() -> None:
    src, out = Path(sys.argv[1]), Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    with open(out / "labels.tsv", "w", encoding="utf-8") as labels:
        labels.write("corpus\tfile\tsheet\trow\tcol\trole\n")
        for f in sorted(src.glob("*.jl.gz")):
            t, c = convert(f, out, labels)
            print(f"{f.name}: {t} tables, {c} labelled cells")


if __name__ == "__main__":
    main()
