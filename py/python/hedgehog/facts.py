"""Compact, deterministic "facts" about a workbook: sheets, sizes, previews and verified totals.

Facts are what an LLM sees instead of raw cells (summaries, ``describe_workbook`` tool); every value
in them carries its cell address so it can be cited and checked.
"""

from __future__ import annotations

import json
import os
from typing import Any, Dict, Iterable, List, Optional, Tuple, Union

from ._native import Workbook, open as _open

WorkbookLike = Union[Workbook, str, "os.PathLike[str]"]


def load(wb: WorkbookLike) -> Workbook:
    return wb if isinstance(wb, Workbook) else _open(os.fspath(wb))


# -- A1 helpers ---------------------------------------------------------------------------------
def col_index(letters: str) -> int:
    n = 0
    for ch in letters.upper():
        n = n * 26 + (ord(ch) - 64)
    return n - 1


def col_letters(idx: int) -> str:
    out = ""
    idx += 1
    while idx:
        idx, rem = divmod(idx - 1, 26)
        out = chr(65 + rem) + out
    return out


def split_a1(addr: str) -> Tuple[int, int]:
    """``"D43"`` → (row 42, col 3), 0-based."""
    addr = addr.replace("$", "").upper()
    i = 0
    while i < len(addr) and addr[i].isalpha():
        i += 1
    return int(addr[i:]) - 1, col_index(addr[:i])


def parse_range(rng: str) -> Tuple[int, int, int, int]:
    """``"A1:F20"`` / ``"D4"`` → (top, left, bottom, right), 0-based inclusive."""
    a, _, b = rng.partition(":")
    r1, c1 = split_a1(a)
    r2, c2 = split_a1(b) if b else (r1, c1)
    return min(r1, r2), min(c1, c2), max(r1, r2), max(c1, c2)


def make_range(top: int, left: int, bottom: int, right: int) -> str:
    a = f"{col_letters(left)}{top + 1}"
    b = f"{col_letters(right)}{bottom + 1}"
    return a if a == b else f"{a}:{b}"


def qualify(sheet: str, addr: str) -> str:
    """``Sheet1!D43``; sheet names with spaces or punctuation are quoted Excel-style."""
    plain = sheet.replace("_", "").replace(".", "").isalnum()
    name = sheet if plain else "'" + sheet.replace("'", "''") + "'"
    return f"{name}!{addr}"


# -- facts --------------------------------------------------------------------------------------
def _num(v: Any) -> Any:
    if isinstance(v, float) and v.is_integer() and abs(v) < 1e15:
        return int(v)
    if isinstance(v, float):
        return round(v, 6)
    return v


def _rows(rows: List[List[Any]]) -> List[List[Any]]:
    return [[_num(v) for v in row] for row in rows]


def total_status(t: Dict[str, Any]) -> str:
    """The normalize manifest's status words: ``ok`` (verified), ``mismatch`` (does not add up),
    ``unknown`` (not verified: e.g. the source range could not be confirmed, or the value is an
    average). Only ``mismatch`` means the total is wrong."""
    return {True: "ok", False: "mismatch"}.get(t.get("ok"), "unknown")


def _total_item(t: Dict[str, Any]) -> Dict[str, Any]:
    status = total_status(t)
    item: Dict[str, Any] = {
        "cell": qualify(t["sheet"], t["address"]),
        "label": t.get("label"),
        "value": _num(t.get("actual")),
        "status": status,
        # An unverified total's recomputed sum is a guess over a guessed range, not what it should be.
        "expected" if status != "unknown" else "recomputed": _num(t.get("expected")),
        "ok": t.get("ok"),
        "reason": t.get("reason"),
        "sources": t.get("sources") or [],
        "level": t.get("level"),
        "origin": t.get("origin"),
    }
    if t.get("formula"):
        item["formula"] = t["formula"]
    if t.get("label_address"):
        item["label_cell"] = qualify(t["sheet"], t["label_address"])
    if t.get("double_counted"):
        item["double_counted"] = t["double_counted"]
    if t.get("numbers_as_text"):
        item["numbers_as_text"] = t["numbers_as_text"]
    return item


def ordered_totals(totals: List[Dict[str, Any]]) -> List[Dict[str, Any]]:
    """Mismatches first, then verified totals, then unverified ones (sheet order within each)."""
    return [t for t in totals if t["ok"] is False] + [t for t in totals if t["ok"] is True] + [
        t for t in totals if t["ok"] is None
    ]


def all_totals(wb: Workbook, sheet: Optional[str] = None) -> List[Dict[str, Any]]:
    names = [sheet] if sheet else wb.sheet_names
    out: List[Dict[str, Any]] = []
    for name in names:
        out.extend(wb.totals(name))
    return out


def facts(
    wb: WorkbookLike,
    *,
    small_sheet_cells: int = 150,
    top_rows: int = 3,
    max_cols: int = 12,
    max_totals: int = 40,
) -> Dict[str, Any]:
    """Facts dict. Small sheets (≤ ``small_sheet_cells`` filled cells) include all their values; larger
    ones only the first ``top_rows`` rows (headers, usually). Totals list mismatches first."""
    wb = load(wb)
    sheets: List[Dict[str, Any]] = []
    for s in wb.sheets():
        entry = {k: s[k] for k in ("name", "used_range", "rows", "cols", "cells", "formulas", "merges")}
        if s["used_range"]:
            top, left, bottom, right = parse_range(s["used_range"])
            if s["cells"] <= small_sheet_cells and (bottom - top + 1) * (right - left + 1) <= 4 * small_sheet_cells:
                rng = s["used_range"]
                entry["values"] = {"range": rng, "rows": _rows(wb.get_range(s["name"], rng)["rows"])}
            else:
                rng = make_range(top, left, min(bottom, top + top_rows - 1), min(right, left + max_cols - 1))
                entry["top_rows"] = {"range": rng, "rows": _rows(wb.get_range(s["name"], rng)["rows"])}
        sheets.append(entry)

    totals = all_totals(wb)
    mismatched = [t for t in totals if t["ok"] is False]
    verified = [t for t in totals if t["ok"] is True]
    unchecked = [t for t in totals if t["ok"] is None]
    ordered = ordered_totals(totals)
    items = [_total_item(t) for t in ordered[:max_totals]]
    out_totals: Dict[str, Any] = {
        "found": len(totals),
        "verified": len(verified),
        "mismatched": len(mismatched),
        "unchecked": len(unchecked),
        "items": items,
    }
    if len(ordered) > max_totals:
        out_totals["omitted"] = len(ordered) - max_totals
    return {"sheets": sheets, "totals": out_totals}


def facts_json(wb: WorkbookLike, **kwargs: Any) -> str:
    """Compact JSON of :func:`facts` (no spaces, UTF-8 kept)."""
    return json.dumps(facts(wb, **kwargs), ensure_ascii=False, separators=(",", ":"))


def known_cells(f: Dict[str, Any]) -> Tuple[Dict[Tuple[str, str], Any], List[Tuple[str, Tuple[int, int, int, int]]]]:
    """Cells (sheet, A1) → value that appear in the facts, plus ranges named in them (totals' sources)."""
    cells: Dict[Tuple[str, str], Any] = {}
    ranges: List[Tuple[str, Tuple[int, int, int, int]]] = []
    for s in f.get("sheets", []):
        for key in ("values", "top_rows"):
            block = s.get(key)
            if not block:
                continue
            top, left, _, _ = parse_range(block["range"])
            for i, row in enumerate(block["rows"]):
                for j, v in enumerate(row):
                    if v is not None:
                        cells[(s["name"], f"{col_letters(left + j)}{top + i + 1}")] = v
    for t in f.get("totals", {}).get("items", []):
        sheet, addr = unqualify(t["cell"])
        cells[(sheet, addr)] = t.get("value")
        if t.get("label_cell"):
            ls, la = unqualify(t["label_cell"])
            cells[(ls, la)] = t.get("label")
        for src in t.get("sources", []):
            ranges.append((sheet, parse_range(src)))
    return cells, ranges


def unqualify(ref: str) -> Tuple[str, str]:
    sheet, _, addr = ref.rpartition("!")
    if sheet.startswith("'") and sheet.endswith("'"):
        sheet = sheet[1:-1].replace("''", "'")
    return sheet, addr


def iter_names(f: Dict[str, Any]) -> Iterable[str]:
    return (s["name"] for s in f.get("sheets", []))
