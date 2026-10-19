"""Finds cell citations (``Sheet1!D43``, ``'My sheet'!B2:B9``, ``(D43)``) in model text and checks them
against the workbook: the sheet exists, the cell is filled, and a number written right before the
citation equals the cell's value (allowing for the rounding the text shows)."""

from __future__ import annotations

import math
import re
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Tuple

from ._native import Workbook
from .facts import known_cells, parse_range, qualify

_CELL = r"\$?([A-Z]{1,3})\$?([1-9]\d{0,6})"
_ANY_QUALIFIED = re.compile(r"(?:'((?:[^']|'')+)'|([^\s'!(\[{,;:]+))!" + _CELL + r"(?::" + _CELL + r")?")
_BARE = re.compile(r"(?:\(|\[|\bcells?\s+)" + _CELL + r"(?::" + _CELL + r")?(?=[\s\)\],;.])", re.IGNORECASE)
_NUMBER = re.compile(
    r"(?<![\w.])[-−]?\(?[$€£]?"
    r"(\d{1,3}(?:,\d{3})+(?:\.\d+)?|\d{1,3}(?:\.\d{3})+,\d+|\d+(?:\.\d+)?)"
    r"\s*(%|k\b|K\b|M\b|bn\b|million\b|thousand\b|billion\b)?"
)
_SCALE = {"k": 1e3, "K": 1e3, "thousand": 1e3, "M": 1e6, "million": 1e6, "bn": 1e9, "billion": 1e9}


@dataclass
class Citation:
    ref: str  # normalized "Sheet!A1" or "Sheet!A1:B2"
    sheet: Optional[str]
    rect: Tuple[int, int, int, int]
    start: int
    end: int
    status: str = "ok"  # ok | unknown_sheet | empty_cell | out_of_range | mismatch | not_in_facts
    written: Optional[float] = None
    actual: Any = None


@dataclass
class CitationReport:
    citations: List[Citation] = field(default_factory=list)
    uncited_numbers: List[str] = field(default_factory=list)

    @property
    def cited(self) -> List[str]:
        seen: Dict[str, None] = {}
        for c in self.citations:
            seen.setdefault(c.ref, None)
        return list(seen)

    @property
    def invalid(self) -> List[Citation]:
        return [c for c in self.citations if c.status not in ("ok",)]

    @property
    def ok(self) -> bool:
        return not self.invalid

    def problems(self) -> List[str]:
        out = []
        for c in self.invalid:
            if c.status == "mismatch":
                out.append(f"{c.ref}: text says {c.written:g} but the cell holds {c.actual!r}")
            elif c.status == "unknown_sheet":
                out.append(f"{c.ref}: no such sheet")
            elif c.status == "out_of_range":
                out.append(f"{c.ref}: outside the sheet's used range")
            elif c.status == "empty_cell":
                out.append(f"{c.ref}: empty cell")
            elif c.status == "not_in_facts":
                out.append(f"{c.ref}: exists, but was not in the facts given to the model")
        return out


def parse_number(text: str, unit: Optional[str] = None) -> Tuple[float, float]:
    """(value, rounding tolerance) for a number as written: ``1,234.5`` → (1234.5, 0.05)."""
    if re.fullmatch(r"\d{1,3}(?:\.\d{3})+,\d+", text):  # European 1.234,56
        text = text.replace(".", "").replace(",", ".")
    else:
        text = text.replace(",", "")
    value = float(text)
    decimals = len(text.split(".", 1)[1]) if "." in text else 0
    tol = 0.5 * 10 ** (-decimals)
    scale = _SCALE.get(unit or "", 1.0)
    if unit == "%":
        scale = 0.01
    return value * scale, tol * scale


def _matches(written: float, tol: float, actual: Any) -> bool:
    if not isinstance(actual, (int, float)) or isinstance(actual, bool):
        return False
    for cand in (written, -written):
        if math.isclose(cand, actual, rel_tol=1e-9, abs_tol=1e-9) or abs(cand - actual) <= tol + 1e-9:
            return True
        # percentages may be stored as 12 or 0.12
        if tol and abs(cand * 100 - actual) <= tol * 100 + 1e-9:
            return True
    return False


def find_citations(text: str, sheet_names: List[str]) -> List[Citation]:
    """Qualified refs (sheet names may be unquoted even with spaces) and bare ``(D43)`` refs."""
    out: List[Citation] = []
    taken: List[Tuple[int, int]] = []
    # Unquoted names with spaces: match known sheet names explicitly, longest first.
    if sheet_names:
        names = sorted(sheet_names, key=len, reverse=True)
        alt = "|".join(re.escape(n) for n in names)
        known = re.compile(r"(?:'(" + alt + r")'|(?<![\w'])(" + alt + r"))!" + _CELL + r"(?::" + _CELL + r")?")
        for m in known.finditer(text):
            name = m.group(1) or m.group(2)
            out.append(_citation(name, m.groups()[2:], m.start(), m.end()))
            taken.append((m.start(), m.end()))
    for m in _ANY_QUALIFIED.finditer(text):
        if any(s <= m.start() < e for s, e in taken):
            continue
        name = (m.group(1) or m.group(2) or "").replace("''", "'")
        out.append(_citation(name, m.groups()[2:], m.start(), m.end(), known=False))
        taken.append((m.start(), m.end()))
    for m in _BARE.finditer(text):
        if any(s <= m.start() < e for s, e in taken):
            continue
        sheet = sheet_names[0] if len(sheet_names) == 1 else None
        out.append(_citation(sheet, m.groups(), m.start(), m.end()))
    out.sort(key=lambda c: c.start)
    return out


def _citation(sheet: Optional[str], g: Tuple[Any, ...], start: int, end: int, known: bool = True) -> Citation:
    c1, r1, c2, r2 = g[0], g[1], g[2], g[3]
    rng = f"{c1.upper()}{r1}" + (f":{c2.upper()}{r2}" if c2 else "")
    ref = qualify(sheet, rng) if sheet else rng
    c = Citation(ref=ref, sheet=sheet, rect=parse_range(rng), start=start, end=end)
    if not known:
        c.status = "unknown_sheet"
    return c


def check_citations(
    text: str,
    wb: Workbook,
    facts: Optional[Dict[str, Any]] = None,
) -> CitationReport:
    """Validates every citation in ``text`` against ``wb``. With ``facts``, a cited cell that exists in
    the workbook but was not part of the facts (cells, totals, or their source ranges) is flagged
    ``not_in_facts`` — the model could not have seen it."""
    sheets = {s["name"]: s for s in wb.sheets()}
    report = CitationReport(citations=find_citations(text, list(sheets)))
    fcells, franges = known_cells(facts) if facts else ({}, [])
    for c in report.citations:
        if c.status != "ok":
            continue
        if c.sheet is None or c.sheet not in sheets:
            # bare ref in a multi-sheet workbook: accept if exactly one sheet has it filled
            hits = [n for n in sheets if wb.get_cell(n, _first(c))["type"] != "empty"] if c.sheet is None else []
            if len(hits) == 1:
                c.sheet = hits[0]
                c.ref = qualify(hits[0], c.ref)
            else:
                c.status = "unknown_sheet"
                continue
        used = sheets[c.sheet]["used_range"]
        top, left, bottom, right = c.rect
        if not used:
            c.status = "out_of_range"
            continue
        ut, ul, ub, ur = parse_range(used)
        if top < ut or left < ul or bottom > ub or right > ur:
            c.status = "out_of_range"
            continue
        single = top == bottom and left == right
        if single:
            cell = wb.get_cell(c.sheet, _first(c))
            c.actual = cell["value"]
            if cell["type"] == "empty" and not cell["formula"]:
                c.status = "empty_cell"
                continue
        if facts is not None and not _in_facts(c, fcells, franges):
            c.status = "not_in_facts"
            continue
        if single:
            written = _number_before(text, c, report.citations)
            if written is not None:
                value, tol = written
                c.written = value
                if not _matches(value, tol, c.actual):
                    c.status = "mismatch"
    report.uncited_numbers = _uncited_numbers(text, report.citations)
    return report


def _first(c: Citation) -> str:
    top, left, _, _ = c.rect
    from .facts import col_letters  # noqa: PLC0415

    return f"{col_letters(left)}{top + 1}"


def _in_facts(c: Citation, cells: Dict[Tuple[str, str], Any], ranges: List[Tuple[str, Tuple[int, int, int, int]]]) -> bool:
    top, left, bottom, right = c.rect
    from .facts import col_letters  # noqa: PLC0415

    for r in range(top, bottom + 1):
        for col in range(left, right + 1):
            if (c.sheet, f"{col_letters(col)}{r + 1}") in cells:
                continue
            if any(s == c.sheet and rt <= r <= rb and rl <= col <= rr for s, (rt, rl, rb, rr) in ranges):
                continue
            return False
    return True


def _number_before(text: str, c: Citation, all_cits: List[Citation]) -> Optional[Tuple[float, float]]:
    """The last number in the ~60 characters before the citation, not inside another citation."""
    lo = max(0, c.start - 60)
    prev_end = max([x.end for x in all_cits if x.end <= c.start] + [lo])
    window = text[prev_end:c.start]
    best = None
    for m in _NUMBER.finditer(window):
        best = m
    if best is None:
        return None
    try:
        return parse_number(best.group(1), best.group(2))
    except ValueError:
        return None


def _uncited_numbers(text: str, cits: List[Citation]) -> List[str]:
    """Numbers that look like data values (decimals, thousands separators or ≥ 100, not a year) and
    are not followed by a citation within 40 characters."""
    out = []
    spans = [(c.start, c.end) for c in cits]
    for m in _NUMBER.finditer(text):
        if any(s <= m.start() < e for s, e in spans):
            continue
        raw = m.group(1)
        try:
            value, _ = parse_number(raw)
        except ValueError:
            continue
        plain_int = re.fullmatch(r"\d+", raw) is not None
        if plain_int and (value < 100 or 1900 <= value <= 2100):
            continue
        following = [s for s, _ in spans if m.end() <= s <= m.end() + 40]
        if not following:
            out.append(m.group(0).strip())
    return out
