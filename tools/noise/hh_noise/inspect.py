"""Round-trip checks (generated file read back with calamine matches the ground truth) and a
plain-text viewer for debugging."""
from __future__ import annotations

import datetime as dt
import json
from pathlib import Path

ROLE_CH = {"title": "T", "header": "H", "data": "d", "group_header": "G", "total": "S", "note": "N", "other": "o"}


def read_back(xlsx: Path) -> dict[str, dict[tuple[int, int], object]]:
    from python_calamine import CalamineWorkbook

    wb = CalamineWorkbook.from_path(str(xlsx))
    out = {}
    for name in wb.sheet_names:
        sh = wb.get_sheet_by_name(name)
        cells = {}
        for r, row in enumerate(sh.to_python(skip_empty_area=False)):
            for c, v in enumerate(row):
                if v is None or v == "":
                    continue
                cells[(r, c)] = v
        out[name] = cells
    return out


def roundtrip_errors(xlsx: Path, gt: dict, limit: int = 20) -> list[str]:
    """Every GT cell is non-empty in the file and every non-empty file cell is in the GT; text
    values match exactly."""
    back = read_back(xlsx)
    errs = []
    for s in gt["sheets"]:
        got = back.get(s["name"])
        if got is None:
            errs.append(f"sheet {s['name']!r} missing")
            continue
        want = {(c[0], c[1]): c for c in s["cells"]}
        for pos in want.keys() - got.keys():
            t = want[pos][9] or {}
            if str(t.get("missing", "")).startswith("#"):
                continue  # Excel error values read back as empty by python-calamine
            errs.append(f"{s['name']}!{pos}: in GT ({want[pos][2]}) but empty in file")
        for pos in got.keys() - want.keys():
            if isinstance(got[pos], str) and not got[pos].strip():
                continue
            errs.append(f"{s['name']}!{pos}: {got[pos]!r} in file but not in GT")
        if len(errs) > limit:
            break
    return errs[:limit]


def show(xlsx: Path, gt_path: Path, sheet: int = 0, width: int = 14, max_rows: int = 60) -> str:
    gt = json.loads(Path(gt_path).read_text())
    back = read_back(xlsx)
    s = gt["sheets"][sheet]
    got = back[s["name"]]
    roles = {(c[0], c[1]): c[2] for c in s["cells"]}
    tid = {(c[0], c[1]): c[3] for c in s["cells"]}
    if not got:
        return f"[{s['name']}] empty"
    R = max(r for r, _ in got) + 1
    C = max(c for _, c in got) + 1
    lines = [f"[{s['name']}] tables: " + "; ".join(f"#{t['id']} rect={t['rect']} hdr={t['header_rows']} boundary={t['boundary']}" for t in s["tables"])]
    for r in range(min(R, max_rows)):
        parts = []
        for c in range(C):
            v = got.get((r, c))
            if v is None:
                parts.append(" " * width)
                continue
            if isinstance(v, float) and v.is_integer():
                v = int(v)
            if isinstance(v, (dt.date, dt.datetime)):
                v = v.isoformat()
            t = str(v).replace("\n", " ")[: width - 3]
            parts.append(f"{ROLE_CH.get(roles.get((r, c)), '?')}{tid.get((r, c), '')!s:>1}|{t:<{width - 3}}"[:width])
        lines.append(f"{r:>3} " + "".join(parts))
    return "\n".join(lines)
