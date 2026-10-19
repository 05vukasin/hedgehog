"""Sampling a clean sub-table from a cached source into a :class:`Block`."""
from __future__ import annotations

import datetime as dt
import math
import random

from .model import Block, Cell, ColInfo
from .sources import DATE, NUMERIC, Source

KEY_TYPES = {"category", "country", "state", "city", "name", "id", "code", "date", "year", "month"}
MEASURE_TYPES = {"int", "float", "money", "count"}


def is_measure(stype: str) -> bool:
    return stype in MEASURE_TYPES


def pick_rows(rng: random.Random, median: float, sigma: float, lo: int = 3, hi: int = 250) -> int:
    return max(lo, min(hi, int(round(math.exp(rng.gauss(math.log(median), sigma))))))


def sample_block(src: Source, rng: random.Random, n_rows: int, n_cols: int, lang: str) -> Block | None:
    a, b = rng.choice(src.chunks)
    if b - a < 3:
        return None
    n_rows = min(n_rows, b - a)
    start = rng.randint(a, b - n_rows)
    rows = list(range(start, start + n_rows))

    ncols = len(src.columns)
    n_cols = max(2, min(n_cols, ncols))
    keys = [i for i, c in enumerate(src.columns) if c.stype in KEY_TYPES]
    others = [i for i in range(ncols)]
    chosen: list[int] = []
    if keys and rng.random() < 0.85:
        chosen.append(rng.choice(keys[:3]) if rng.random() < 0.7 else rng.choice(keys))
    measures = [i for i, c in enumerate(src.columns) if c.stype in NUMERIC and i not in chosen]
    if measures and rng.random() < 0.9:
        k = min(len(measures), max(1, n_cols - len(chosen) - rng.randint(0, 2)))
        if rng.random() < 0.6:
            s = rng.randint(0, len(measures) - k)
            chosen += measures[s:s + k]
        else:
            chosen += rng.sample(measures, k)
    rest = [i for i in others if i not in chosen]
    rng.shuffle(rest)
    while len(chosen) < n_cols and rest:
        chosen.append(rest.pop())
    key_first = chosen[0] if chosen and src.columns[chosen[0]].stype in KEY_TYPES else None
    chosen = sorted(set(chosen))
    if key_first is not None and rng.random() < 0.6:
        chosen.remove(key_first)
        chosen.insert(0, key_first)
    # drop rows that are empty in every chosen column
    rows = [r for r in rows if any(src.rows[r][c] is not None for c in chosen)]
    if len(rows) < 2:
        return None

    cells: dict[tuple[int, int], Cell] = {}
    columns = []
    for j, c in enumerate(chosen):
        col = src.columns[c]
        columns.append(ColInfo(header=col.name, stype=col.stype, src_col=c, header_path=[col.name], semantic=col.semantic))
        cells[(0, j)] = Cell(v=col.name, role="header", src=(-1, c), orig=col.name)
    for i, r in enumerate(rows, start=1):
        for j, c in enumerate(chosen):
            v = src.rows[r][c]
            if v is None or v == "" or (isinstance(v, float) and not math.isfinite(v)):
                continue
            cells[(i, j)] = Cell(v=v, role="data", src=(src.row_idx[r], c), orig=v)
    return Block(source=src.name, title=src.title, cells=cells, n_rows=len(rows) + 1, n_cols=len(chosen),
                 header_rows=1, columns=columns, row_kind=["header"] + ["data"] * len(rows), lang=lang)


def numeric_meaning(cell: Cell | None) -> float | None:
    """The number a cell stands for (typed number, or text with a numeric truth)."""
    if cell is None or cell.error:
        return None
    if isinstance(cell.v, bool):
        return None
    if isinstance(cell.v, (int, float)):
        return float(cell.v)
    if cell.true and "num" in cell.true:
        return float(cell.true["num"])
    return None


def crosstab_block(src: Source, rng: random.Random, lang: str, months: list[str]) -> Block | None:
    """Wide layout: key × period → aggregated measure (inverse of `stack`)."""
    cols = src.columns
    keys = [i for i, c in enumerate(cols) if c.stype in ("category", "country", "state", "city", "name")]
    periods = [i for i, c in enumerate(cols) if c.stype in ("year", "date", "month", "datetime")]
    meas = [i for i, c in enumerate(cols) if c.stype in MEASURE_TYPES | {"percent"}]
    if not keys or not periods or not meas:
        return None
    k, p, m = rng.choice(keys), rng.choice(periods), rng.choice(meas)
    ptype = cols[p].stype
    by_year = ptype == "year" or rng.random() < 0.4
    agg: dict[tuple, list] = {}
    first_row: dict[tuple, int] = {}
    year_pick = None
    if not by_year:
        years = [src.rows[r][p].year for r in range(len(src.rows)) if src.rows[r][p] is not None]
        if not years:
            return None
        year_pick = rng.choice(years)
    for r, row in enumerate(src.rows):
        kv, pv, mv = row[k], row[p], row[m]
        if kv is None or pv is None or mv is None or (isinstance(mv, float) and not math.isfinite(mv)):
            continue
        if by_year:
            per = pv if ptype == "year" else pv.year
        else:
            if pv.year != year_pick:
                continue
            per = pv.month
        agg.setdefault((kv, per), []).append(mv)
        first_row.setdefault((kv, per), src.row_idx[r])
    if not agg:
        return None
    pcount: dict = {}
    for kv, per in agg:
        pcount[per] = pcount.get(per, 0) + 1
    if len(pcount) < 3:
        return None
    npers = min(len(pcount), rng.randint(3, 12))
    pers = sorted(sorted(pcount, key=lambda x: -pcount[x])[:npers])
    cover: dict = {}
    for kv, per in agg:
        if per in pers:
            cover[kv] = cover.get(kv, 0) + 1
    good = [kv for kv, n in cover.items() if n >= 0.6 * len(pers)]
    if len(good) < 2:
        return None
    rng.shuffle(good)
    kvals = good[: rng.randint(3, 15)]
    if rng.random() < 0.6:
        kvals = sorted(kvals, key=str)
    how = "sum" if cols[m].stype in ("money", "count", "int") else "mean"

    def label(per):
        if by_year:
            return per if rng.random() < 0.0 else per  # years stay numbers (headers that are numbers)
        return months[per - 1]

    plabels = [label(x) for x in pers]
    if by_year and rng.random() < 0.3:
        plabels = [str(x) for x in pers]
    cells: dict[tuple[int, int], Cell] = {(0, 0): Cell(v=cols[k].name, role="header", src=(-1, k), orig=cols[k].name)}
    columns = [ColInfo(header=cols[k].name, stype=cols[k].stype, src_col=k, header_path=[cols[k].name], semantic=cols[k].semantic)]
    for j, (per, lab) in enumerate(zip(pers, plabels), start=1):
        cells[(0, j)] = Cell(v=lab, role="header", src=None, orig=per, ops=["crosstab"])
        columns.append(ColInfo(header=cols[m].name, stype=cols[m].stype, src_col=m, header_path=[str(lab)], semantic=cols[m].semantic,
                               derived=f"crosstab:{cols[p].name}={per}:{how}"))
    for i, kv in enumerate(kvals, start=1):
        cells[(i, 0)] = Cell(v=kv, role="data", src=None, orig=kv, ops=["crosstab"])
        for j, per in enumerate(pers, start=1):
            vals = agg.get((kv, per))
            if not vals:
                continue
            v = sum(vals) if how == "sum" else sum(vals) / len(vals)
            if isinstance(v, float):
                v = round(v, 4)
            cells[(i, j)] = Cell(v=v, role="data", src=(first_row[(kv, per)], m), orig=v, ops=["crosstab"],
                                 true={"agg": how, "n": len(vals)})
    b = Block(source=src.name, title=src.title, cells=cells, n_rows=len(kvals) + 1, n_cols=len(pers) + 1,
              header_rows=1, columns=columns, row_kind=["header"] + ["data"] * len(kvals), lang=lang)
    b.log("crosstab", key=cols[k].name, period=cols[p].name, measure=cols[m].name, agg=how,
          by="year" if by_year else f"month:{year_pick}")
    b.stats["measure_name"] = cols[m].name
    return b


def is_date(v) -> bool:
    return isinstance(v, (dt.date, dt.datetime))


__all__ = ["sample_block", "crosstab_block", "numeric_meaning", "is_measure", "pick_rows", "DATE", "is_date"]
