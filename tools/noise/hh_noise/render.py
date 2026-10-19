"""Stage D rendering: SheetDrafts → .xlsx (XlsxWriter) + ground-truth JSON.

Ground-truth schema `hh-noise-gt/1` (one JSON per workbook, next to it as `<name>.gt.json`):

{ "schema": "hh-noise-gt/1", "file": "<name>.xlsx", "profile", "seed", "generator",
  "datasets": ["vega_datasets/stocks.csv", ...],          # ds index → clean source path
  "op_sets": [["missing_tokens"], ...],                     # opset index → operator names
  "cell_fields": ["row","col","role","table","ds","src_row","src_col","orig","ops","true"],
  "sheets": [{ "name", "index", "kind": "tables"|"empty", "ops": [...sheet noise program...],
      "merges": [[top,left,bottom,right], ...], "hidden_rows": [...], "hidden_cols": [...],
      "tables": [{ "id", "rect": [top,left,bottom,right], "header_top", "header_rows", "header_cols",
                   "orientation": "col"|"row", "ds", "boundary": {"op", ...}, "scale",
                   "columns": [{"col"|"row": abs index, "header", "header_path", "semantic_type", "src_col", "derived"}],
                   "groups": [{"label": [r,c], "value", "rows": [r1, r2]}],
                   "implied": [{"row","col","src_row","src_col","orig","ops", ...}],
                   "ops": [...table noise program...] }],
      "cells": [[row, col, role, table|-1, ds|-1, src_row|null, src_col|null, orig, opset|-1, true|null], ...] }] }

All coordinates are 0-based sheet coordinates. Every non-empty written cell appears exactly once
in `cells` (merged areas at their top-left). `src_row = -1` marks a header cell of the source.
"""
from __future__ import annotations

import datetime as dt
import json
from pathlib import Path

import xlsxwriter
from xlsxwriter.utility import xl_rowcol_to_cell

from .model import Block, Cell, SheetDraft, jsonable
from .table import numeric_meaning

SCHEMA = "hh-noise-gt/1"
CELL_FIELDS = ["row", "col", "role", "table", "ds", "src_row", "src_col", "orig", "ops", "true"]
ERROR_FORMULA = {"#N/A": "=NA()", "#REF!": "=#REF!", "#DIV/0!": "=1/0", "#VALUE!": "=\"a\"+1"}


class _Formats:
    def __init__(self, wb):
        self.wb = wb
        self.cache = {}

    def get(self, style: dict):
        if not style:
            return None
        key = tuple(sorted(style.items()))
        if key not in self.cache:
            d = {}
            for k, v in style.items():
                if k == "fill":
                    d["bg_color"] = v
                    d["pattern"] = 1
                elif k == "border_top":
                    d["top"] = v
                elif k == "border_bottom":
                    d["bottom"] = v
                elif k == "wrap":
                    d["text_wrap"] = v
                else:
                    d[k] = v
            self.cache[key] = self.wb.add_format(d)
        return self.cache[key]


def _sum_formula(refs_abs: list[tuple[int, int]]) -> str:
    rows = {r for r, _ in refs_abs}
    cols = {c for _, c in refs_abs}
    rs = sorted(refs_abs)
    if len(cols) == 1 and max(rows) - min(rows) + 1 == len(rows):
        c = next(iter(cols))
        return f"=SUM({xl_rowcol_to_cell(min(rows), c)}:{xl_rowcol_to_cell(max(rows), c)})"
    if len(rows) == 1 and max(cols) - min(cols) + 1 == len(cols):
        r = next(iter(rows))
        return f"=SUM({xl_rowcol_to_cell(r, min(cols))}:{xl_rowcol_to_cell(r, max(cols))})"
    return "=SUM(" + ",".join(xl_rowcol_to_cell(r, c) for r, c in rs) + ")"


def _resolve_totals(b: Block):
    """Compute every total's true value (recursively) and decide formula vs typed value."""
    memo: dict[tuple[int, int], float | None] = {}

    def val(pos, depth=0):
        cell = b.cells.get(pos)
        if cell is None:
            return 0.0, True
        if cell.formula is not None and depth < 8:
            if pos not in memo:
                tot, clean = 0.0, True
                for rp in cell.formula["refs"]:
                    v, ok = val(rp, depth + 1)
                    tot += v
                    clean &= ok
                memo[pos] = (round(tot, 6), clean)
            return memo[pos]
        m = numeric_meaning(cell)
        typed = isinstance(cell.v, (int, float)) and not isinstance(cell.v, bool)
        return (m or 0.0), typed

    for pos, cell in b.cells.items():
        if cell.formula is None:
            continue
        total, clean = val(pos)
        f = cell.formula
        f["true_value"] = total
        if not clean and not f["as_value"]:
            f["as_value"] = True  # Excel's SUM would skip the text cells: a typed total instead
            cell.tag("total_as_value_text_refs")
        shown = total
        if f["wrong"] is not None:
            shown = round(total * f["wrong"], 2) if f["wrong"] != -1.0 else round(-total, 2)
            if shown == total:
                shown = total + 1
        cell.v = int(shown) if float(shown).is_integer() and abs(shown) < 1e15 else round(shown, 6)
        cell.true = {"num": total, **({"wrong": True, "shown": cell.v} if f["wrong"] is not None else {}),
                     **({"formula": False} if f["as_value"] else {"formula": True})}


def render(drafts: list[SheetDraft], xlsx: Path, meta: dict) -> dict:
    wb = xlsxwriter.Workbook(str(xlsx), {"strings_to_numbers": False, "strings_to_formulas": False,
                                          "strings_to_urls": False, "default_date_format": "yyyy-mm-dd"})
    fmts = _Formats(wb)
    datasets: list[str] = []
    opsets: list[tuple] = []
    opset_idx: dict[tuple, int] = {}

    def ds_of(name):
        if name not in datasets:
            datasets.append(name)
        return datasets.index(name)

    def ops_of(ops):
        if not ops:
            return -1
        t = tuple(ops)
        if t not in opset_idx:
            opset_idx[t] = len(opsets)
            opsets.append(t)
        return opset_idx[t]

    gt_sheets = []
    for si, d in enumerate(drafts):
        ws = wb.add_worksheet(d.name)
        grid: dict[tuple[int, int], tuple[Cell, int, int]] = {}  # abs → (cell, table, ds)
        merges: list[tuple[int, int, int, int]] = []
        tables = []
        for p in d.placed:
            b = p.block
            _resolve_totals(b)
            ds = ds_of(b.source)
            for (r, c), cell in b.cells.items():
                grid[(p.top + r, p.left + c)] = (cell, p.table_id, ds, (p.top, p.left))
            for r1, c1, r2, c2 in b.merges:
                merges.append((p.top + r1, p.left + c1, p.top + r2, p.left + c2))
            tables.append(_table_gt(p, ds))
        for lc in d.loose:
            if (lc.row, lc.col) in grid:
                continue  # never overwrite table content
            grid[(lc.row, lc.col)] = (lc.cell, -1, -1, (0, 0))
            if lc.merge_to is not None and lc.merge_to > lc.col:
                if not any((lc.row, cc) in grid for cc in range(lc.col + 1, lc.merge_to + 1)):
                    merges.append((lc.row, lc.col, lc.row, lc.merge_to))
        merged_tl = {(m[0], m[1]): m for m in merges}
        covered = {(r, c) for m in merges for r in range(m[0], m[2] + 1) for c in range(m[1], m[3] + 1) if (r, c) != (m[0], m[1])}
        cells_gt = []
        for (r, c) in sorted(grid):
            if (r, c) in covered:
                continue  # a merge swallowed it (should not happen; keep GT honest)
            cell, tid, ds, origin = grid[(r, c)]
            fmt = fmts.get(cell.style)
            v = cell.v
            if (r, c) in merged_tl:
                m = merged_tl[(r, c)]
                ws.merge_range(m[0], m[1], m[2], m[3], "", fmt)
            if cell.formula is not None and not cell.formula["as_value"]:
                refs = [(origin[0] + rr, origin[1] + cc) for rr, cc in cell.formula["refs"]]
                ws.write_formula(r, c, _sum_formula(refs), fmt, v)
            elif cell.error:
                ws.write_formula(r, c, ERROR_FORMULA.get(v, "=NA()"), fmt, v if v in ERROR_FORMULA else "#N/A")
            elif isinstance(v, bool):
                ws.write_boolean(r, c, v, fmt)
            elif isinstance(v, (int, float)):
                ws.write_number(r, c, v, fmt)
            elif isinstance(v, (dt.date, dt.datetime)):
                if fmt is None:
                    fmt = fmts.get({"num_format": "yyyy-mm-dd"})
                ws.write_datetime(r, c, v if isinstance(v, dt.datetime) else dt.datetime(v.year, v.month, v.day), fmt)
            else:
                v = str(v)
                if not v.strip():
                    continue
                ws.write_string(r, c, v, fmt)
            src_row, src_col = cell.src if cell.src else (None, None)
            cells_gt.append([r, c, cell.role, tid, ds if cell.src is not None or tid >= 0 else -1, src_row, src_col,
                             jsonable(cell.orig), ops_of(cell.ops), _true_json(cell.true, origin)])
        for c, w in d.col_widths.items():
            ws.set_column(c, c, w)
        for r in sorted(d.hidden_rows):
            ws.set_row(r, None, None, {"hidden": True})
        for c in sorted(d.hidden_cols):
            ws.set_column(c, c, None, None, {"hidden": True})
        if d.freeze:
            ws.freeze_panes(*d.freeze)
        if d.autofilter:
            ws.autofilter(*d.autofilter)
        gt_sheets.append({"name": d.name, "index": si, "kind": d.kind, "ops": d.ops, "merges": [list(m) for m in merges],
                          "hidden_rows": sorted(d.hidden_rows), "hidden_cols": sorted(d.hidden_cols), "tables": tables,
                          "cells": cells_gt})
    wb.close()
    gt = {"schema": SCHEMA, "file": xlsx.name, **meta, "datasets": datasets, "op_sets": [list(o) for o in opsets],
          "cell_fields": CELL_FIELDS, "sheets": gt_sheets}
    return gt


def _true_json(t, origin=(0, 0)):
    if not t:
        return None
    out = {k: jsonable(v) for k, v in t.items()}
    if "true_col" in out:  # value_drift: table-local → sheet column where the value belongs
        out["true_col"] = origin[1] + out["true_col"]
    return out


def _table_gt(p, ds: int) -> dict:
    b = p.block
    T, L = p.top, p.left
    cols = []
    for i, ci in enumerate(b.columns):
        e = {"col" if b.orientation == "col" else "row": (L + i) if b.orientation == "col" else (T + i),
             "header": ci.header, "header_path": [str(x) for x in ci.header_path],
             "semantic_type": ci.semantic or ci.stype, "gen_type": ci.stype,
             "src_col": ci.src_col}
        if ci.derived:
            e["derived"] = ci.derived
        cols.append(e)
    groups = []
    for r, k in enumerate(b.row_kind):
        if k != "group_header":
            continue
        lab = next(((r, c) for c in range(b.n_cols) if (r, c) in b.cells), None)
        rr = r + 1
        rows = []
        while rr < b.n_rows and b.row_kind[rr] not in ("group_header",):
            if b.row_kind[rr] == "data":
                rows.append(rr)
            if b.row_kind[rr] == "total":
                break
            rr += 1
        if lab and rows:
            groups.append({"label": [T + lab[0], L + lab[1]], "value": jsonable(b.cells[lab].orig),
                           "rows": [T + rows[0], T + rows[-1]]})
    implied = []
    for e in b.implied:
        x = {"row": None if e.get("r") is None else T + e["r"], "col": L + e["c"],
             "src_row": e["src"][0] if e.get("src") else None, "src_col": e["src"][1] if e.get("src") else None,
             "orig": jsonable(e.get("orig")), "ops": e.get("ops", [])}
        for k in ("same_as", "merged_into"):
            if k in e:
                x[k] = [T + e[k][0], L + e[k][1]]
        implied.append(x)
    return {
        "id": p.table_id,
        "rect": [T, L, T + b.n_rows - 1, L + b.n_cols - 1],
        "header_top": T if b.header_rows else None,
        "header_rows": b.header_rows,
        "header_cols": b.header_cols,
        "orientation": b.orientation,
        "ds": ds,
        "boundary": b.stats.get("boundary", {"op": "sheet_start"}),
        "scale": b.scale,
        "row_kinds": b.row_kind,
        "hidden_rows": sorted(T + r for r in b.hidden_rows),
        "hidden_cols": sorted(L + c for c in b.hidden_cols),
        "columns": cols,
        "groups": groups,
        "implied": implied,
        "ops": b.ops,
    }


def write_gt(gt: dict, path: Path):
    path.write_text(json.dumps(gt, ensure_ascii=False, separators=(",", ":")))
