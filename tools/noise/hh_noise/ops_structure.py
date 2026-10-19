"""Stage B — table-structure operators (inverses of Auto-Tables reshaping operators plus the
report-style structures found in Enron/DECO). They run on clean values before stage A, except
the `LATE_OPS` (spacers, repeated header, transpose) which run after it."""
from __future__ import annotations

import datetime as dt
import random
import re

from .model import Block, Cell, ColInfo
from .table import is_measure
from .vocab import GROUP_FMT, MONTHS_ABBR, PARENT_HEADERS, ROW_TOTAL_HDR, SUBTOTAL, TOTAL

GROUP_KEY_TYPES = ("category", "country", "state", "city", "name", "year", "code", "id")


def _measures(b: Block, exclude=()):
    return [c for c, ci in enumerate(b.columns) if is_measure(ci.stype) and c not in exclude
            and any(isinstance(x.v, (int, float)) and not isinstance(x.v, bool) for _, x in b.col_cells(c))]


def _label_col(b: Block, prefer: int | None = None) -> int | None:
    """Column where a total label can go: the key column, else the first non-measure column."""
    if prefer is not None and prefer < b.n_cols:
        return prefer
    for c, ci in enumerate(b.columns):
        if not is_measure(ci.stype) and ci.stype not in ("percent", "spacer"):
            return c
    return None


def _reorder_data(b: Block, order: list[int]):
    """Rearrange data rows (all of them are contiguous right after the header) in `order`."""
    rows = b.data_rows()
    mapping = {old: new for old, new in zip(order, rows)}
    b.cells = {((mapping.get(r, r)), c): v for (r, c), v in b.cells.items()}


def _choose_group_key(b: Block, rng) -> tuple[int, list] | None:
    rows = b.data_rows()
    cands = []
    for c, ci in enumerate(b.columns):
        if ci.stype not in GROUP_KEY_TYPES:
            continue
        vals = [b.cells[(r, c)].v for r in rows if (r, c) in b.cells]
        if len(vals) < len(rows):
            continue
        d = len(set(map(str, vals)))
        if 2 <= d <= min(12, max(2, len(rows) // 2)):
            cands.append((c, d))
    if not cands:
        return None
    c, _ = rng.choice(cands)
    return c, rows


def _month_key(b: Block, rng):
    """A derived month key from a date column (for period group labels like "July '00")."""
    rows = b.data_rows()
    for c, ci in enumerate(b.columns):
        if ci.stype not in ("date", "datetime"):
            continue
        vals = [b.cells[(r, c)].v for r in rows if (r, c) in b.cells]
        if len(vals) != len(rows) or not all(isinstance(v, (dt.date, dt.datetime)) for v in vals):
            continue
        months = {(v.year, v.month) for v in vals}
        if 2 <= len(months) <= 12:
            return c
    return None


def _period_label(y: int, m: int, lang: str, rng) -> str:
    ab = MONTHS_ABBR[lang][m - 1]
    return rng.choice([f"{ab} '{y % 100:02d}", f"{ab} {y}", f"{ab}-{y % 100:02d}", ab, f"{ab.upper()} {y}"])


def grouping(b: Block, rng: random.Random, ctx) -> list[str]:
    """Group-related structure: group header rows / far-left group label column, subtotals,
    blank repeats or vertical merges of the key. Each part is decided by its own probability."""
    applied: list[str] = []
    want_label_col = ctx.p("group_label_column")
    want_rows = ctx.p("group_header_rows")
    want_sub = ctx.p("subtotals")
    want_blank = ctx.p("blank_repeats")
    want_vmerge = ctx.p("vertical_merge_keys")
    if not (want_label_col or want_rows or want_sub or want_blank or want_vmerge):
        return applied

    month_col = _month_key(b, rng) if want_label_col else None
    if month_col is not None:
        rows = b.data_rows()
        keyf = lambda r: (b.cells[(r, month_col)].v.year, b.cells[(r, month_col)].v.month)  # noqa: E731
        kc = month_col
    else:
        got = _choose_group_key(b, rng)
        if got is None:
            return applied
        kc, rows = got
        keyf = lambda r: str(b.cells[(r, kc)].v)  # noqa: E731
    keys = {r: keyf(r) for r in rows}
    order_vals = sorted(set(keys.values()), key=str) if rng.random() < 0.7 or month_col is not None else list(dict.fromkeys(keys.values()))
    if month_col is not None:
        order_vals = sorted(set(keys.values()))
    rank = {v: i for i, v in enumerate(order_vals)}
    order = sorted(rows, key=lambda r: (rank[keys[r]], r))
    _reorder_data(b, order)
    keys = {new: keys[old] for old, new in zip(order, rows)}
    groups: list[tuple[object, list[int]]] = []
    for r in rows:
        if groups and groups[-1][0] == keys[r]:
            groups[-1][1].append(r)
        else:
            groups.append((keys[r], [r]))
    header = b.columns[kc].header
    b.log("sort_by_key", key=header)

    period = month_col is not None
    label_col_mode = want_label_col
    rows_mode = (not label_col_mode) and want_rows
    first_cell = {g: b.cells.get((rs[0], kc)) for g, rs in groups}
    fmt = rng.choice(GROUP_FMT)
    hname = header.replace("_", " ").strip().title()
    if len(hname) > 20 and "{h}" in fmt:
        fmt = "{g}"
    labels = {}
    for g, _ in groups:
        if period:
            labels[g] = _period_label(g[0], g[1], b.lang, rng)
        elif label_col_mode:
            labels[g] = str(g).upper() if rng.random() < 0.3 else str(g)
        else:
            labels[g] = fmt.format(g=g, G=str(g).upper(), h=hname)

    key_removed = False
    if rows_mode or (label_col_mode and not period):
        # the key column disappears; its value lives in the group label of the rows below
        opn = "group_header_rows" if rows_mode else "group_label_column"
        for (r, c), cell in list(b.cells.items()):
            if c == kc and cell.role == "data":
                b.implied.append({"r": r, "c": c, "src": cell.src, "orig": cell.orig, "ops": cell.ops + [opn],
                                  "removed": True, "moved_to_group": True})
        b.delete_col(kc)
        key_removed = True
    if label_col_mode:
        b.insert_col(0, ColInfo(header="", stype="group_label", src_col=None, header_path=[""], derived="group_label"))
        kc += 1

    meas = _measures(b, exclude=() if key_removed else (kc,))
    do_sub = bool(want_sub and meas)
    if label_col_mode:
        tot_lc = 0
    elif rows_mode:
        tot_lc = 0 if 0 not in meas else None
    else:
        tot_lc = kc
    insert_ops = []
    if do_sub:
        for g, rs in groups:
            insert_ops.append((rs[-1] + 1, 1, "total", g, rs))
    if rows_mode or label_col_mode:
        for g, rs in groups:
            insert_ops.append((rs[0], 0, "group", g, rs))
    # bottom-up so earlier row numbers stay valid; at equal positions insert the group row first
    # (it then ends up below the previous group's subtotal)
    insert_ops.sort(key=lambda t: (-t[0], t[1]))
    bold_groups = rng.random() < 0.5
    no_label = rng.random() < 0.12
    lab_fmt = rng.choice(SUBTOTAL[b.lang])
    for at, _, kind, g, rs in insert_ops:
        b.insert_row(at, "total" if kind == "total" else "group_header")
        if kind == "total":
            if tot_lc is not None and not no_label:
                b.cells[(at, tot_lc)] = Cell(v=lab_fmt.format(g=labels[g] if period else g), role="total", ops=["subtotals"])
            for c in meas:
                b.cells[(at, c)] = Cell(v=0, role="total", ops=["subtotals"], formula={"refs": [], "as_value": False, "wrong": None})
        else:
            src_cell = first_cell.get(g)
            op = "group_label_column" if label_col_mode else "group_header_rows"
            gcell = Cell(v=labels[g], role="group_header",
                         src=None if period or src_cell is None else src_cell.src,
                         orig=(f"{g[0]:04d}-{g[1]:02d}" if period else (src_cell.orig if src_cell else g)), ops=[op])
            if bold_groups:
                gcell.style["bold"] = True
            b.cells[(at, 0)] = gcell
    if do_sub:
        _fix_subtotal_refs(b)
        for (r, c), cell in list(b.cells.items()):
            if cell.formula is not None and not cell.formula["refs"]:
                del b.cells[(r, c)]
        applied.append("subtotals")
        b.log("subtotals", key=header, groups=len(groups), labelled=not no_label)
    if label_col_mode:
        applied.append("group_label_column")
        b.log("group_label_column", key=header, groups=len(groups))
    elif rows_mode:
        applied.append("group_header_rows")
        b.log("group_header_rows", key=header, groups=len(groups), fmt=fmt)
    if not key_removed and not label_col_mode and (want_blank or want_vmerge):
        kcol = kc
        runs = []
        cur = None
        for r in range(b.n_rows):
            if b.row_kind[r] != "data":
                cur = None
                continue
            cell = b.cells.get((r, kcol))
            v = str(cell.v) if cell else None
            if cur and cur[0] == v and v is not None:
                cur[2] = r
            else:
                cur = [v, r, r]
                runs.append(cur)
        runs = [x for x in runs if x[2] > x[1]]
        if runs:
            if want_vmerge and (not want_blank or rng.random() < 0.5):
                for _, r1, r2 in runs:
                    for r in range(r1 + 1, r2 + 1):
                        cell = b.cells.pop((r, kcol), None)
                        if cell:
                            b.implied.append({"r": r, "c": kcol, "src": cell.src, "orig": cell.orig, "ops": cell.ops + ["vertical_merge_keys"], "removed": True, "merged_into": [r1, kcol]})
                    b.merges.append((r1, kcol, r2, kcol))
                    b.cells[(r1, kcol)].tag("vertical_merge_keys")
                applied.append("vertical_merge_keys")
                b.log("vertical_merge_keys", col=kcol, runs=len(runs))
            else:
                for _, r1, r2 in runs:
                    for r in range(r1 + 1, r2 + 1):
                        cell = b.cells.pop((r, kcol), None)
                        if cell:
                            b.implied.append({"r": r, "c": kcol, "src": cell.src, "orig": cell.orig, "ops": cell.ops + ["blank_repeats"], "removed": True, "same_as": [r1, kcol]})
                applied.append("blank_repeats")
                b.log("blank_repeats", col=kcol, runs=len(runs))
    if do_sub:
        if ctx.p("grand_total_after_subtotals"):
            _grand_total(b, rng, over_subtotals=True)
            applied.append("grand_total")
    return applied


def _fix_subtotal_refs(b: Block):
    """Point every subtotal at the data rows of its group (rows since the previous total/group row)."""
    start = None
    for r in range(b.n_rows):
        k = b.row_kind[r]
        if k == "data" and start is None:
            start = r
        if k in ("group_header",):
            start = None
        if k == "total":
            for c in range(b.n_cols):
                cell = b.cells.get((r, c))
                if cell and cell.formula is not None and start is not None:
                    cell.formula["refs"] = [(rr, c) for rr in range(start, r) if b.row_kind[rr] == "data" and (rr, c) in b.cells]
            start = None


def _grand_total(b: Block, rng, over_subtotals=False):
    meas = _measures(b)
    if not meas:
        return False
    at = b.n_rows
    b.insert_row(at, "total")
    lc = _label_col(b)
    if lc is not None and lc not in meas and rng.random() < 0.92:
        b.cells[(at, lc)] = Cell(v=rng.choice(TOTAL[b.lang]), role="total", ops=["grand_total"])
    for c in meas:
        if over_subtotals:
            refs = [(r, c) for r in range(at) if b.row_kind[r] == "total" and (r, c) in b.cells]
        else:
            refs = [(r, c) for r in range(at) if b.row_kind[r] == "data" and (r, c) in b.cells]
        if refs:
            b.cells[(at, c)] = Cell(v=0, role="total", ops=["grand_total"], formula={"refs": refs, "as_value": False, "wrong": None})
    return True


def grand_total(b: Block, rng, ctx) -> bool:
    if "total" in b.row_kind:
        return False
    ok = _grand_total(b, rng)
    if ok:
        b.log("grand_total")
    return ok


def total_column(b: Block, rng, ctx) -> bool:
    meas = _measures(b)
    if len(meas) < 2:
        return False
    fam = {b.columns[c].stype for c in meas}
    crosstab = any(ci.derived and ci.derived.startswith("crosstab") for ci in b.columns)
    if not crosstab and (len(fam) > 1 or not fam <= {"money", "count"}):
        return False
    at = b.n_cols
    b.insert_col(at, ColInfo(header=ROW_TOTAL_HDR[b.lang], stype=b.columns[meas[0]].stype, src_col=None,
                             header_path=[ROW_TOTAL_HDR[b.lang]], derived="row_total"))
    hr = b.header_rows - 1
    b.cells[(hr, at)] = Cell(v=ROW_TOTAL_HDR[b.lang], role="header", ops=["total_column"])
    for r in range(b.n_rows):
        if b.row_kind[r] in ("data", "total"):
            refs = [(r, c) for c in meas if (r, c) in b.cells]
            if refs:
                b.cells[(r, at)] = Cell(v=0, role="total", ops=["total_column"], formula={"refs": refs, "as_value": False, "wrong": None})
    b.log("total_column", cols=meas)
    return True


def wrong_total(b: Block, rng, ctx) -> bool:
    cands = [c for c in b.cells.values() if c.formula is not None]
    if not cands:
        return False
    cell = rng.choice(cands)
    cell.formula["wrong"] = rng.choice([0.9, 1.1, 1.01, 0.5, 2.0, -1.0, 10.0])
    cell.formula["as_value"] = True
    cell.tag("wrong_total")
    b.log("wrong_total", factor=cell.formula["wrong"])
    return True


def totals_as_values(b: Block, rng, ctx) -> bool:
    cells = [c for c in b.cells.values() if c.formula is not None]
    if not cells:
        return False
    for c in cells:
        c.formula["as_value"] = True
        c.tag("totals_as_values")
    b.log("totals_as_values")
    return True


# ---------------------------------------------------------------- headers


def _common_affix(names: list[str]) -> str | None:
    toks = [re.split(r"[ _.\-/]+", n) for n in names]
    if len(toks) < 2 or any(len(t) < 2 for t in toks):
        return None
    if all(t[0].lower() == toks[0][0].lower() for t in toks):
        return toks[0][0]
    if all(t[-1].lower() == toks[0][-1].lower() for t in toks):
        return toks[0][-1]
    return None


def multirow_header(b: Block, rng, ctx) -> bool:
    """Insert 1–2 parent header rows; parents merged across their child columns."""
    if b.header_rows != 1 or b.n_cols < 3:
        return False
    cols = list(range(b.n_cols))
    lead = 0
    while lead < b.n_cols - 1 and not (is_measure(b.columns[lead].stype) or b.columns[lead].stype == "percent"):
        lead += 1
    body = cols[lead:]
    if len(body) < 2:
        lead, body = 1, cols[1:]
    # chunk the body into groups of 2..4
    groups = []
    i = 0
    while i < len(body):
        k = min(len(body) - i, rng.choice([2, 2, 3, 3, 4]))
        if len(body) - i - k == 1:
            k += 1
        groups.append(body[i:i + k])
        i += k
    levels = 2 if rng.random() < 0.15 and len(groups) >= 2 else 1
    b.insert_row(0, "header")
    for _ in range(levels - 1):
        b.insert_row(0, "header")
    b.header_rows += levels
    child_row = levels
    parents = PARENT_HEADERS[b.lang][:]
    rng.shuffle(parents)
    years = rng.random() < 0.25
    y0 = rng.randint(2000, 2024)
    merge = rng.random() < 0.8
    for gi, g in enumerate(groups):
        names = [str(b.cells[(child_row, c)].v) if (child_row, c) in b.cells else "" for c in g]
        aff = _common_affix(names)
        if years:
            lab = str(y0 + gi)
        elif aff and rng.random() < 0.8:
            lab = aff
            if rng.random() < 0.5:  # strip the shared part from the children (real report style)
                for c in g:
                    cell = b.cells.get((child_row, c))
                    if cell and isinstance(cell.v, str):
                        nv = re.sub(r"^" + re.escape(aff) + r"[ _.\-/]+|[ _.\-/]+" + re.escape(aff) + r"$", "", cell.v, flags=re.I)
                        if nv and nv != cell.v:
                            cell.v = nv
                            cell.tag("multirow_header")
        else:
            lab = parents[gi % len(parents)] if gi < len(parents) else f"{parents[0]} {gi + 1}"
        r = levels - 1
        b.cells[(r, g[0])] = Cell(v=lab, role="header", ops=["multirow_header"], style={"align": "center"})
        if merge and len(g) > 1:
            b.merges.append((r, g[0], r, g[-1]))
        for c in g:
            if c < len(b.columns):
                b.columns[c].header_path = [lab] + b.columns[c].header_path
    if levels == 2:
        lab = rng.choice(PARENT_HEADERS[b.lang])
        b.cells[(0, body[0])] = Cell(v=lab, role="header", ops=["multirow_header"], style={"align": "center"})
        if merge:
            b.merges.append((0, body[0], 0, body[-1]))
        for c in body:
            b.columns[c].header_path = [lab] + b.columns[c].header_path
    # leading key columns: header merged vertically over all header rows, or left blank above
    for c in range(lead):
        if rng.random() < 0.6 and (child_row, c) in b.cells:
            cell = b.cells.pop((child_row, c))
            b.cells[(0, c)] = cell
            b.merges.append((0, c, child_row, c))
    b.log("multirow_header", levels=levels + 1, groups=len(groups), merged=merge)
    return True


def wrapped_header(b: Block, rng, ctx) -> bool:
    """Header text split over two rows without merges ("Datum" / "isplate")."""
    if b.header_rows != 1:
        return False
    multi = [c for c in range(b.n_cols) if isinstance(getattr(b.cells.get((0, c)), "v", None), str)
             and len(re.split(r"[ _]+", b.cells[(0, c)].v.strip())) >= 2]
    if len(multi) < 2:
        return False
    b.insert_row(0, "header")
    b.header_rows = 2
    for c in multi:
        cell = b.cells[(1, c)]
        parts = re.split(r"[ _]+", cell.v.strip())
        k = max(1, len(parts) // 2)
        b.cells[(0, c)] = Cell(v=" ".join(parts[:k]), role="header", src=cell.src, orig=cell.orig, ops=["wrapped_header"])
        cell.v = " ".join(parts[k:])
        cell.tag("wrapped_header")
        b.columns[c].header_path = [b.cells[(0, c)].v, cell.v]
    b.log("wrapped_header", cols=multi)
    return True


def row_label_column(b: Block, rng, ctx) -> bool:
    """Text label column left of the numbers ("Gas Annuity ="), optionally separated from them by
    blank columns, header often empty. Ground truth: part of the table, role data (a key column)."""
    rows = b.data_rows()
    tcols = []
    for c, ci in enumerate(b.columns):
        if ci.stype not in ("name", "category", "text", "city", "country", "state", "code"):
            continue
        vals = [b.cells[(r, c)].v for r in rows if (r, c) in b.cells]
        if len(vals) >= 0.9 * len(rows) and len(set(map(str, vals))) >= 0.8 * len(vals):
            tcols.append(c)
    meas = _measures(b)
    if not tcols or len(meas) < 1:
        return False
    c = tcols[0]
    if c != 0:  # move the label column to the far left
        moved = {r: b.cells.pop((r, c)) for r in range(b.n_rows) if (r, c) in b.cells}
        info = b.columns[c]
        b.delete_col(c)
        b.insert_col(0, info)
        for r, cell in moved.items():
            b.cells[(r, 0)] = cell
    suffix = rng.choice([" =", " =", ":", " :", "=", ""])
    for r, cell in b.col_cells(0):
        if isinstance(cell.v, str) and suffix:
            cell.true = {**(cell.true or {}), "canon": cell.orig}
            cell.v = cell.v.rstrip() + suffix
            cell.tag("row_label_column")
    b.columns[0].derived = "row_label"
    hdr_blank = rng.random() < 0.5
    if hdr_blank:
        for r in range(b.header_rows):
            cell = b.cells.pop((r, 0), None)
            if cell:
                b.implied.append({"r": r, "c": 0, "src": cell.src, "orig": cell.orig, "ops": cell.ops + ["row_label_column"], "removed": True})
    gap = rng.choice([0, 1, 1, 2]) if rng.random() < 0.6 else 0
    for _ in range(gap):
        b.insert_col(1, ColInfo(header="", stype="spacer", src_col=None, header_path=[], derived="spacer"))
    b.log("row_label_column", suffix=suffix, header_blank=hdr_blank, gap_cols=gap)
    return True


# ---------------------------------------------------------------- late ops (after stage A)


def spacer_rows(b: Block, rng, ctx) -> bool:
    rows = b.data_rows()
    if len(rows) < 6:
        return False
    every = rng.randint(3, 10)
    ats = [r for i, r in enumerate(rows) if i and i % every == 0 and b.row_kind[r - 1] == "data"]
    if not ats:
        return False
    for at in sorted(ats, reverse=True):
        b.insert_row(at, "spacer")
    b.log("spacer_rows", every=every, n=len(ats))
    return True


def spacer_cols(b: Block, rng, ctx) -> bool:
    if b.n_cols < 3 or b.orientation != "col":
        return False
    at = rng.randint(1, b.n_cols - 1)
    if any(m[1] < at <= m[3] for m in b.merges):
        return False
    b.insert_col(at, ColInfo(header="", stype="spacer", src_col=None, header_path=[], derived="spacer"))
    b.log("spacer_cols", at=at)
    return True


def repeat_header(b: Block, rng, ctx) -> bool:
    rows = b.data_rows()
    if len(rows) < 30 or b.header_rows != 1:
        return False
    every = rng.randint(15, 40)
    ats = [r for i, r in enumerate(rows) if i and i % every == 0]
    hdr = {c: b.cells[(0, c)] for c in range(b.n_cols) if (0, c) in b.cells}
    for at in sorted(ats, reverse=True):
        b.insert_row(at, "header")
        for c, h in hdr.items():
            b.cells[(at, c)] = Cell(v=h.v, role="header", src=h.src, orig=h.orig, ops=h.ops + ["repeat_header"])
    b.log("repeat_header", every=every, n=len(ats))
    return bool(ats)


def transpose(b: Block, rng, ctx) -> bool:
    if b.n_rows > 16 or b.header_rows != 1 or any(k not in ("header", "data") for k in b.row_kind) or b.merges \
            or any("value_drift" in c.ops for c in b.cells.values()):
        return False
    b.cells = {(c, r): v for (r, c), v in b.cells.items()}
    for v in b.cells.values():
        if v.formula:
            v.formula["refs"] = [(c, r) for r, c in v.formula["refs"]]
    b.n_rows, b.n_cols = b.n_cols, b.n_rows
    b.header_cols, b.header_rows = 1, 0
    b.orientation = "row"
    b.row_kind = ["data"] * b.n_rows
    b.log("transpose")
    return True


STRUCT_OPS = {
    "group_header_rows": (None, "section rows ('Region: North') replace the key column; value belongs to the rows below"),
    "group_label_column": (None, "period/category label ('July \\'00') alone on its row in a far-left column, data rows below"),
    "subtotals": (None, "subtotal row after each group as =SUM() with cached values (label or unlabelled)"),
    "grand_total": (grand_total, "grand total row at the bottom (=SUM over data or over subtotals)"),
    "blank_repeats": (None, "repeated key values blanked (outline style, inverse of ffill)"),
    "vertical_merge_keys": (None, "runs of equal key values vertically merged"),
    "total_column": (total_column, "row-total column at the right (=SUM across measures)"),
    "wrong_total": (wrong_total, "one total deliberately wrong (typed value), labelled with the true sum"),
    "totals_as_values": (totals_as_values, "totals typed as values instead of formulas"),
    "multirow_header": (multirow_header, "1–2 parent header rows merged over child columns (common prefixes, years or generic parents)"),
    "wrapped_header": (wrapped_header, "header text wrapped over two rows without merges"),
    "row_label_column": (row_label_column, "label column left of numbers with '='/':' suffixes, maybe blank columns between; role data"),
    "crosstab": (None, "wide layout: key × period (years/months) aggregated measure"),
    "transpose": (transpose, "attributes in rows, records in columns (small tables)"),
    "spacer_rows": (spacer_rows, "blank row every k data rows inside the table"),
    "spacer_cols": (spacer_cols, "blank column inside the table"),
    "repeat_header": (repeat_header, "header repeated every N rows (print layout)"),
}
