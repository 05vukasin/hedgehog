"""Structural checks of the ground-truth JSON (`hh-noise-gt/1`)."""
from __future__ import annotations

import json
from pathlib import Path

from .model import ROLES
from .render import CELL_FIELDS, SCHEMA


def validate(gt: dict) -> list[str]:
    e = []
    if gt.get("schema") != SCHEMA:
        e.append(f"schema {gt.get('schema')!r}")
    if gt.get("cell_fields") != CELL_FIELDS:
        e.append("cell_fields mismatch")
    for k in ("file", "profile", "seed", "datasets", "op_sets", "sheets"):
        if k not in gt:
            e.append(f"missing key {k}")
    nds, nops = len(gt.get("datasets", [])), len(gt.get("op_sets", []))
    names = set()
    for s in gt.get("sheets", []):
        if s["name"] in names:
            e.append(f"duplicate sheet {s['name']}")
        names.add(s["name"])
        seen = set()
        tids = {t["id"] for t in s["tables"]}
        rects = {t["id"]: t["rect"] for t in s["tables"]}
        for t in s["tables"]:
            top, left, bottom, right = t["rect"]
            if not (0 <= top <= bottom and 0 <= left <= right):
                e.append(f"{s['name']}: bad rect {t['rect']}")
            if t["header_rows"] and t["header_top"] != top:
                e.append(f"{s['name']}: header_top {t['header_top']} != rect top {top}")
            if t["header_rows"] > bottom - top + 1:
                e.append(f"{s['name']}: header_rows exceeds table")
            if not (0 <= t["ds"] < nds):
                e.append(f"{s['name']}: table ds out of range")
            if "op" not in t.get("boundary", {}):
                e.append(f"{s['name']}: table {t['id']} has no boundary op")
            for g in t["groups"]:
                r, c = g["label"]
                if not (top <= r <= bottom and left <= c <= right):
                    e.append(f"{s['name']}: group label outside table")
        # tables never overlap
        ts = list(rects.values())
        for i in range(len(ts)):
            for j in range(i + 1, len(ts)):
                a, b = ts[i], ts[j]
                if a[0] <= b[2] and b[0] <= a[2] and a[1] <= b[3] and b[1] <= a[3]:
                    e.append(f"{s['name']}: tables overlap {a} {b}")
        for cell in s["cells"]:
            if len(cell) != len(CELL_FIELDS):
                e.append(f"{s['name']}: cell arity {cell}")
                continue
            r, c, role, tid, ds, sr, sc, orig, ops, true = cell
            if (r, c) in seen:
                e.append(f"{s['name']}: duplicate cell {(r, c)}")
            seen.add((r, c))
            if role not in ROLES:
                e.append(f"{s['name']}: bad role {role}")
            if tid != -1:
                if tid not in tids:
                    e.append(f"{s['name']}: unknown table {tid}")
                else:
                    t0, l0, b0, r0 = rects[tid]
                    if not (t0 <= r <= b0 and l0 <= c <= r0):
                        e.append(f"{s['name']}: cell {(r, c)} outside its table {rects[tid]}")
            elif role in ("data", "header", "total", "group_header"):
                e.append(f"{s['name']}: {role} cell {(r, c)} outside every table")
            if ds != -1 and not (0 <= ds < nds):
                e.append(f"{s['name']}: ds out of range")
            if ops != -1 and not (0 <= ops < nops):
                e.append(f"{s['name']}: opset out of range")
            if role == "data" and tid != -1 and sr is None and "crosstab" not in (gt["op_sets"][ops] if ops >= 0 else []) \
                    and "group_label_column" not in (gt["op_sets"][ops] if ops >= 0 else []):
                e.append(f"{s['name']}: data cell {(r, c)} without provenance")
            if ops == -1 and sr is None and role != "data":
                e.append(f"{s['name']}: injected {role} cell {(r, c)} without operator")
        if len(e) > 50:
            break
    return e


def validate_dir(root: Path, limit: int | None = None, roundtrip: bool = False) -> tuple[int, list[str]]:
    errs = []
    files = sorted(root.glob("*/*.gt.json"))[: limit or None]
    for g in files:
        gt = json.loads(g.read_text())
        es = validate(gt)
        if roundtrip:
            from .inspect import roundtrip_errors
            es += roundtrip_errors(g.with_name(gt["file"]), gt)
        errs += [f"{g.relative_to(root)}: {x}" for x in es]
    return len(files), errs
