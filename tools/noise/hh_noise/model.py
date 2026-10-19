"""In-memory representation of a synthetic sheet, with provenance on every cell.

A *block* is one table in table-local coordinates (row 0 = first header row). Operators edit
blocks (stage A/B), then the layout stage places blocks and loose cells (titles, notes, stray
cells) on a sheet (stage C), and the renderer writes XlsxWriter output plus ground truth (stage D).

Invariants every operator keeps (tests check them):
* a cell's ``src``/``orig`` never change once set (provenance is never deleted);
* operators append their name to ``cell.ops`` for every cell they touch;
* injected cells (titles, totals, notes, …) have ``src = None`` and a role set by the operator.
"""
from __future__ import annotations

import datetime as dt
from dataclasses import dataclass, field
from typing import Any

ROLES = ("title", "header", "data", "group_header", "total", "note", "other")


@dataclass
class Cell:
    v: Any  # value written to the file: str/int/float/bool/date/datetime (never None)
    role: str
    src: tuple[int, int] | None = None  # (original data row, original column) in the source file
    orig: Any = None  # clean value from the source (for data/header cells)
    ops: list[str] = field(default_factory=list)
    true: dict | None = None  # value-level truth when `v` is text standing for a number/date/missing
    style: dict = field(default_factory=dict)  # bold/italic/fill/border_top/border/indent/align/num_format/wrap/font_size
    # {"refs": [(r, c), ...], "as_value": bool, "wrong": float|None} in block-local coordinates;
    # resolved to `=SUM(...)` with a cached value at render time
    formula: dict | None = None
    error: bool = False  # `v` is an Excel error literal written as an error value

    def tag(self, op: str):
        if op not in self.ops:
            self.ops.append(op)


@dataclass
class ColInfo:
    header: str  # original header text in the source
    stype: str
    src_col: int | None  # original column index (None for injected columns, e.g. row totals)
    header_path: list[str] = field(default_factory=list)  # rendered header texts top → bottom
    derived: str | None = None  # "row_total" / "crosstab:<period>" for synthesized columns
    semantic: str | None = None  # MANIFEST.csv semantic type of the source column


@dataclass
class Block:
    """One table, table-local coordinates."""

    source: str  # dataset path relative to testdata/clean
    title: str
    cells: dict[tuple[int, int], Cell]
    n_rows: int
    n_cols: int
    header_rows: int
    columns: list[ColInfo]
    row_kind: list[str]  # per local row: header/data/total/group_header/spacer
    header_cols: int = 0  # >0 for transposed tables (header in the first column)
    orientation: str = "col"
    merges: list[tuple[int, int, int, int]] = field(default_factory=list)  # (r1, c1, r2, c2) local
    implied: list[dict] = field(default_factory=list)  # values removed by blank_repeats (still true data)
    ops: list[dict] = field(default_factory=list)  # noise program for this table
    hidden_rows: set[int] = field(default_factory=set)
    hidden_cols: set[int] = field(default_factory=set)
    lang: str = "en"
    scale: int = 1  # numbers shown divided by this (scale_thousands)
    stats: dict = field(default_factory=dict)

    def log(self, op: str, **params):
        self.ops.append({"op": op, **params})

    def data_rows(self) -> list[int]:
        return [r for r, k in enumerate(self.row_kind) if k == "data"]

    def col_cells(self, c: int, kind: str = "data"):
        for r, k in enumerate(self.row_kind):
            if k == kind and (r, c) in self.cells:
                yield r, self.cells[(r, c)]

    def insert_row(self, at: int, kind: str):
        """Insert an empty row before local row `at`; shifts cells, merges and hidden rows."""
        self.cells = {((r + 1 if r >= at else r), c): v for (r, c), v in self.cells.items()}
        self.merges = [((r1 + 1 if r1 >= at else r1), c1, (r2 + 1 if r2 >= at else r2), c2) for r1, c1, r2, c2 in self.merges]
        self.hidden_rows = {r + 1 if r >= at else r for r in self.hidden_rows}
        self._shift_formulas(lambda r, c: ((r + 1 if r >= at else r), c))
        self.row_kind.insert(at, kind)
        self.n_rows += 1

    def insert_col(self, at: int, info: ColInfo):
        self.cells = {(r, (c + 1 if c >= at else c)): v for (r, c), v in self.cells.items()}
        self.merges = [(r1, (c1 + 1 if c1 >= at else c1), r2, (c2 + 1 if c2 >= at else c2)) for r1, c1, r2, c2 in self.merges]
        self.hidden_cols = {c + 1 if c >= at else c for c in self.hidden_cols}
        self._shift_formulas(lambda r, c: (r, (c + 1 if c >= at else c)))
        self.columns.insert(at, info)
        self.n_cols += 1

    def delete_col(self, at: int):
        self.cells = {(r, (c - 1 if c > at else c)): v for (r, c), v in self.cells.items() if c != at}
        self.merges = [(r1, (c1 - 1 if c1 > at else c1), r2, (c2 - 1 if c2 > at else c2)) for r1, c1, r2, c2 in self.merges if not (c1 == c2 == at)]
        self._shift_formulas(lambda r, c: (r, (c - 1 if c > at else c)))
        del self.columns[at]
        self.n_cols -= 1

    def _shift_formulas(self, f):
        for cell in self.cells.values():
            if cell.formula:
                cell.formula["refs"] = [f(r, c) for r, c in cell.formula["refs"]]


@dataclass
class Loose:
    """A cell outside every table (title, note, stray cell), in sheet coordinates."""

    row: int
    col: int
    cell: Cell
    merge_to: int | None = None  # last column of a horizontal merge


@dataclass
class Placed:
    block: Block
    top: int
    left: int
    table_id: int


@dataclass
class SheetDraft:
    name: str
    placed: list[Placed] = field(default_factory=list)
    loose: list[Loose] = field(default_factory=list)
    freeze: tuple[int, int] | None = None
    autofilter: tuple[int, int, int, int] | None = None
    col_widths: dict[int, float] = field(default_factory=dict)
    hidden_rows: set[int] = field(default_factory=set)
    hidden_cols: set[int] = field(default_factory=set)
    ops: list[dict] = field(default_factory=list)
    kind: str = "tables"

    def log(self, op: str, **params):
        self.ops.append({"op": op, **params})

    def extent(self) -> tuple[int, int]:
        """(next free row, next free col) over everything placed so far."""
        rows, cols = [0], [0]
        for p in self.placed:
            rows.append(p.top + p.block.n_rows)
            cols.append(p.left + p.block.n_cols)
        for lc in self.loose:
            rows.append(lc.row + 1)
            cols.append((lc.merge_to if lc.merge_to is not None else lc.col) + 1)
        return max(rows), max(cols)


def jsonable(v):
    if isinstance(v, dt.datetime):
        return v.isoformat()
    if isinstance(v, dt.date):
        return v.isoformat()
    if isinstance(v, float) and v != v:
        return None
    return v


EXCEL_EPOCH = dt.datetime(1899, 12, 30)


def excel_serial(v) -> float:
    if isinstance(v, dt.datetime):
        d = v - EXCEL_EPOCH
        return d.days + d.seconds / 86400.0
    if isinstance(v, dt.date):
        return float((v - EXCEL_EPOCH.date()).days)
    raise TypeError(v)
