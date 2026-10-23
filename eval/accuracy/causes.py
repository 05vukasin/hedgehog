"""C miss causes: every C miss gets exactly one family and cause, with an evidence string.

Families
  library       hedgehog's tables lose or change what the computation needs: an operand cell in no table,
                a value changed, a number kept as text, a total row among the data rows, a header missing or
                misnamed (`col_X`, `Yuan (2)`: reachable through the source's own header cells), a label path
                lost (the question names a source row label hedgehog's row does not carry), a table split
                between operands the source keeps in one table.
  oracle-limit  the program shape is not covered or the question names an operand only loosely (the shape
                is listed: "share of a sum of rows", "forecast", "group-by average", ...).
  gold/question the gold is not reproduced from its own operand cells (HiTab), or the question does not
                name an operand at all (an implicit year, ...). Audit candidates: never audited automatically,
                listed for review with the evidence.

Order: library causes first (they need a fix in hedgehog), then gold/question (HiTab's gold operands prove
it), then oracle-limit. Source header grounding ("hedgehog header naming") reruns the oracle on hedgehog's
tables with generated column names replaced by the source header cells above the column (cells.tsv): a
tight witness there is recorded as the miss's witness, and the miss stays a miss.
"""

from __future__ import annotations

import copy
import re

import numpy as np

import acc
import oracle as O
from acc import PS, norm_text, parse_number
from common import col_letter

LIB, LIM, GOLD = "library", "oracle-limit", "gold/question"
GENERATED = re.compile(r"^col_[A-Z]+$|\(\d+\)$")


# ------------------------------------------------------------------------------------------------
# source header cells


def _a1_row(a1: str | None) -> int | None:
    m = re.match(r"^[A-Z]+(\d+)", a1 or "")
    return int(m.group(1)) if m else None


def _letter_index(letters: str) -> int:
    n = 0
    for ch in letters:
        n = n * 26 + ord(ch) - 64
    return n - 1


def generated(name: str) -> bool:
    """A column name hedgehog made up: col_X, a '(n)' duplicate, or a bare unit ('Yuan', 'kg', '%')."""
    if GENERATED.search(name.split(" / ")[-1]):
        return True
    return not (O.cwords(name) - O._UNIT_WORDS - O._GENERIC)


def source_header(raw: dict, sheet: str, t, k: int) -> str:
    """The source's header text above column k of table t: the cells in the column from the header's
    last row up to 8 rows above its first (a merged header cell: the nearest non-empty cell to the left in
    rows above the last header row), stopping at a number (the data of a table above)."""
    src = (getattr(t, "manifest", None) or {}).get("source", {})
    top = _a1_row((src.get("header_range") or src.get("data_range") or "").split(":")[0])
    if top is None or k >= len(t.letters) or not t.letters[k]:
        return ""
    bottom = top + max(1, int(src.get("header_rows") or 1)) - 1
    ci = _letter_index(t.letters[k])
    first = min(_letter_index(x) for x in t.letters if x)
    texts = []
    for r in range(bottom, max(0, top - 9), -1):
        rr = r - 1
        v = raw.get((sheet, rr, ci), "").strip()
        if not v and r < bottom:
            for cj in range(ci - 1, first - 1, -1):
                w = raw.get((sheet, rr, cj), "").strip()
                if w:
                    v = w if parse_number(w) is None else ""
                    break
        if not v:
            continue
        p = parse_number(v)
        if p is not None and not (p[0] == int(p[0]) and 1900 <= p[0] <= 2100):
            break
        if not texts or texts[-1] != v:
            texts.append(v)
    return " / ".join(reversed(texts))


def relabel(it: dict, tables: list, q) -> tuple[list, list[str]]:
    """Copies of the tables with generated numeric column names replaced by the source header text, where
    that names the column better for this question; and the evidence lines."""
    out, ev = [], []
    raws = {}
    for t in tables:
        rel = getattr(t, "file", None)
        if rel not in raws:
            raws[rel] = acc.load(it["dataset"], rel)[1] if rel else {}
        raw = raws[rel]
        new = None
        for k, h in enumerate(t.headers):
            if k in t.label_cols or not generated(h):
                continue
            sh = source_header(raw, t.sheet, t, k)
            if not sh or O._n(sh) == O._n(h):
                continue
            if q.col_grounded(sh) > q.col_grounded(h):
                new = new or list(t.headers)
                new[k] = sh
                ev.append(f"{t.id} '{h}' is '{sh}' in the source ({t.sheet}!{t.letters[k]})")
        if new:
            t2 = copy.copy(t)
            t2.headers = new
            out.append(t2)
        else:
            out.append(t)
    return out, ev


# ------------------------------------------------------------------------------------------------
# HiTab: the gold operand cells and their source header paths


def _hitab_replay_any(cells: list[dict], gold: dict, question: str) -> str | None:
    """HiTab's gold reproduced from its operand cells by any reading of its aggregations (a superset of
    the oracle's formulas: sums of all-but-one, ratios of sums, differences of sums, absolute values)."""
    vs = []
    for c in cells:
        p = parse_number(c.get("raw") or c.get("text"))
        if p is None:
            return None
        vs.append(p[0] / 100 if p[2] else p[0])
    if not vs:
        return None
    tg = O.gold_targets(gold, question)
    cand = {"value": vs[0], "sum": sum(vs), "avg": sum(vs) / len(vs), "max": max(vs), "min": min(vs),
            "range": max(vs) - min(vs), "count": float(len(vs))}
    for i, a in enumerate(vs):
        rest = vs[:i] + vs[i + 1:]
        cand[f"v{i}"] = a
        if rest:
            sr = sum(rest)
            cand[f"rest{i}-a"] = sr - a
            if a:
                cand[f"rest{i}/a"] = sr / a
                cand[f"a/rest{i}"] = a / sr if sr else np.nan
                cand[f"(rest-a)/a{i}"] = (sr - a) / a
                cand[f"rest{i}avg/a"] = sr / len(rest) / a
            for j, b in enumerate(vs):
                if i != j:
                    cand[f"{i}-{j}"] = a - b
                    if b:
                        cand[f"{i}/{j}"] = a / b
                        cand[f"({i}-{j})/{j}"] = (a - b) / b
    for k, v in cand.items():
        if v is not None and np.isfinite(v) and O.match_mask(np.array([float(v)]), tg)[0]:
            return k
    return None


_IMPLICIT_WHOLE = {"canada", "united states", "u s", "us", "both sexes", "total population", "all ages", "national"}


def _same(a: str, b: str) -> bool:
    x, y = O._n(a), O._n(b)
    return bool(x) and (x == y or (len(x) >= 4 and (x in y or y in x)))


def _totalish(path: list[str]) -> bool:
    return not path or bool(O.TOTAL_LABEL.search(O._n(path[-1])))


def hitab_cause(it: dict, atom: dict, tables: list, q) -> tuple[str, str, str] | None:
    cells = it.get("cells") or []
    if not cells:
        return None
    tbl = {t.id: t for t in tables}
    locs = []
    for c in cells:
        loc = PS.locate(tables, it["sheet"], c["row"], c["col"])
        if isinstance(loc, str):
            return LIB, "operand cell not in any table", f"{it['sheet']}!{c.get('a1')} = {c.get('raw')}: {loc}"
        locs.append((c, loc))
    # values hedgehog changed
    for c, (t, i, k) in locs:
        p = parse_number(c.get("raw") or c.get("text"))
        v = t.rows[i][k] if k < len(t.rows[i]) else None
        if p is None:
            continue
        x = O._f_relaxed(v)
        sv = p[0] / 100 if p[2] else p[0]
        if not (np.isfinite(x) and (abs(x - sv) <= 1e-6 * max(1, abs(sv)) or abs(x - p[0]) <= 1e-6 * max(1, abs(p[0])))):
            return LIB, "value changed", f"{it['sheet']}!{c.get('a1')}: source '{c.get('raw')}' stored {v!r} ({t.id} '{t.headers[k]}')"
    # a table split between operands the source keeps in one row or one column
    if len({t.id for _, (t, _, _) in locs}) > 1:
        rows = {c["row"] for c, _ in locs}
        cols = {c["col"] for c, _ in locs}
        if len(rows) == 1 or len(cols) == 1:
            where = ", ".join(f"{c.get('a1')}->{t.id}" for c, (t, _, _) in locs)
            return LIB, "operands split into two tables", f"source {'row' if len(rows) == 1 else 'column'} shared by {where}"
    # label path lost / header misnamed: a source path element the question names that hedgehog lacks
    for c, (t, i, k) in locs:
        labs = t.labels(i)
        roots = [t.headers[x] for x in t.label_cols]
        for e in c.get("left_path") or []:
            if O.TOTAL_LABEL.search(O._n(e)) or any(_same(e, x) for x in roots):
                continue  # HiTab roots its left tree at the label column's header or a total row
            if q.entity(e) and not any(_same(e, x) for x in labs) and not _same(e, t.headers[k]):
                return LIB, "label path lost", f"{c.get('a1')}: '{e}' (named by the question) is no label of hedgehog's row {labs[:4]} ({t.id})"
        segs = t.headers[k].split(" / ")
        for e in c.get("top_path") or []:
            if O.TOTAL_LABEL.search(O._n(e)):
                continue
            if q.col_grounded(e) == 2 and not any(_same(e, s) for s in segs) and not any(_same(e, x) for x in labs):
                return LIB, "header misnamed", f"{c.get('a1')}: '{e}' (named by the question) is not in hedgehog's column '{t.headers[k]}' ({t.id})"
    # the gold is not reproduced from its own operands
    if not _hitab_replay_any(cells, atom, it["question"]):
        vals = ", ".join(f"{c.get('a1')}={c.get('raw')}" for c in cells[:6])
        return GOLD, "gold not reproduced from its operand cells", f"gold {atom['text']}, aggregation {it.get('aggregation')}, operands {vals}"
    # does the question name every operand (through the source paths)?
    single = {t.id: sum(1 for x in (t.row_types or ["data"] * len(t.rows)) if x == "data") == 1 for t in tables}
    agg = set(it.get("aggregation") or [])
    picked = bool(agg & {"max", "min", "range", "argmax", "argmin", "counta", "pair-argmax", "pair-argmin"})  # the op picks the rows
    for c, (t, i, k) in locs:
        lp, tp = c.get("left_path") or [], c.get("top_path") or []
        row_named = picked or _totalish(lp) or single.get(t.id) or any(q.entity(e) for e in lp) or \
            any(re.search(rf"(?<!\d){y}(?!\d)", " ".join(lp)) for y in map(str, q.years))
        col_named = _totalish(tp) or q.col_grounded(" / ".join(tp)) >= 1 or not (O.cwords(" / ".join(tp)) - O._UNIT_WORDS - O._GENERIC)
        if not (row_named and col_named):
            miss = lp if not row_named else tp
            words = O.cwords(" ".join(miss)) & q.cwords
            what = f"{'row' if not row_named else 'column'} {miss}"
            if words:
                return LIM, "grounding: question paraphrases an operand", f"{c.get('a1')} {what}; shared words {sorted(words)[:4]}"
            if miss and O._n(miss[-1]) in _IMPLICIT_WHOLE:
                return LIM, "grounding: implicit whole (national total, both sexes)", f"{c.get('a1')} {what}"
            if re.search(r"(?<!\d)(19|20)\d\d(?!\d)", " ".join(miss)) and (not q.years or q.adj_years or
                                                                     re.search(r"previous|prior|last year|year earlier", q.norm)):
                return LIM, "grounding: implicit period (the question names no year)", f"{c.get('a1')} {what}"
            return GOLD, "question does not name an operand", f"{c.get('a1')} {what}: no word of it in the question"
    # named, but loosely: the oracle's tight guard needs the whole column and a row the question names
    for c, (t, i, k) in locs:
        lp, tp = c.get("left_path") or [], c.get("top_path") or []
        if not (q.col_grounded(" / ".join(tp)) == 2 or _totalish(tp) or not (O.cwords(" / ".join(tp)) - O._UNIT_WORDS - O._GENERIC)):
            return LIM, "grounding: column named only in part", f"{c.get('a1')} column {tp} vs hedgehog '{t.headers[k]}'"
        if not (picked or single.get(t.id) or any(q.entity(e) for e in lp) or
                any(re.search(rf"(?<!\d){y}(?!\d)", " ".join(lp)) for y in map(str, q.years))):
            return LIM, "grounding: implicit whole (total row the question does not name)", f"{c.get('a1')} row {lp}"
    agg = "+".join(it.get("aggregation") or ["?"])
    rows = {c["row"] for c in cells}
    cols = {c["col"] for c in cells}
    geom = "one cell" if len(cells) == 1 else "one column" if len(cols) == 1 else "one row" if len(rows) == 1 else "a block"
    return LIM, f"shape: {_HITAB_SHAPE.get(agg, agg)} ({geom})", f"{len(cells)} operand cells named by the question; aggregation {agg}"


_HITAB_SHAPE = {"div": "ratio / share", "sum": "sum of named cells", "diff": "difference", "sum+div": "share of a sum",
                "div+sum": "share of a sum", "average": "average of named cells", "none": "lookup", "opposite": "lookup (sign)",
                "max": "maximum", "min": "minimum", "range": "range", "diff+div": "relative change", "counta": "count",
                "max+min": "extremes"}


# ------------------------------------------------------------------------------------------------
# question shapes (MiMoTable, RealHiTBench, and HiTab without operands)


_SHAPES = [
    (r"predict|forecast|projected|projection|extrapolat|trend continues|if the .* (continues|trend)|next year|will be", "forecast / extrapolation"),
    (r"\bif\b.*\b(were|was|increase|decrease|change|reduce|raise|sold|added|removed|doubl)", "hypothetical (what-if)"),
    (r"\bmedian\b", "median"),
    (r"standard deviation|variance|\bstd\b|coefficient of variation", "standard deviation / variance"),
    (r"compound|cagr|average (annual|yearly|monthly) (growth|increase|change|rate)|annual(ized)? growth rate", "average growth over a period"),
    (r"weighted", "weighted average"),
    (r"\b(each|every|per)\b.*\b(average|mean|total|sum)\b|\b(average|mean|total|sum)\b.*\b(each|every|per|by)\b", "group-by aggregate"),
    (r"\bwhich\b|\bwho\b|\bwhose\b|\brank|\bhighest|\blowest|\bmost\b|\bleast\b|\btop\b", "argmax / ranking over groups"),
    (r"\bdays?\b|\bduration|how long|\bmonths? between|date", "date arithmetic"),
    (r"percent|percentage|proportion|share|\bratio\b|rate\b", "ratio / share of unnamed operands"),
    (r"differ|more than|less than|compare|exceed|gap|change|increase|decrease|growth|higher|lower", "difference / change of aggregates"),
    (r"how many|number of|\bcount", "conditional count"),
    (r"average|mean", "average over a conditional set"),
    (r"total|sum|combined|altogether|overall", "sum over a conditional set"),
]
_SHAPES = [(re.compile(a, re.I), b) for a, b in _SHAPES]


def question_shape(q: str) -> str:
    for rx, name in _SHAPES:
        if rx.search(q):
            return name
    return "multi-step expression"


# ------------------------------------------------------------------------------------------------


def _witness_cols(progs, i: int) -> list[str]:
    ti, k = progs.ref[i]
    tp = progs.tps[ti]
    j = tp.meta[k][2]
    if isinstance(j, (int, np.integer)):
        return [tp.t.headers[tp.numcols[j]]]
    if isinstance(j, tuple) and j and j[0] == "cols":
        return [tp.t.headers[tp.numcols[x]] for x in j[1]]
    return []


def _loose_reason(progs, res: dict, q) -> tuple[str, str, str] | None:
    """Why the best non-tight match is not tight, from its grounding."""
    cand = res.get("_unary") or []
    pairs = res.get("_binary") or []
    if cand:
        i = cand[0]
        cols = _witness_cols(progs, i)
        if any(GENERATED.search(c.split(" / ")[-1]) for c in cols):
            return LIB, "header missing (generated column name)", "column '" + next(c for c in cols if GENERATED.search(c.split(" / ")[-1]))[:60] + "'; "
        if not progs.tight[i] and not O.op_named(progs.tps[progs.ref[i][0]].meta[progs.ref[i][1]][0], q.text):
            return LIM, "grounding: operation not named by the question", ""
        if progs.colg[i] < 2 and (progs.rowg[i] >= 1 or progs.whole[i]):
            return LIM, "grounding: column named only in part", ""
        if progs.rowg[i] == 0:
            return LIM, "grounding: row set not named by the question", ""
        return LIM, "grounding: operand named only in part", ""
    if pairs:
        op, a, b = pairs[0]
        cols = _witness_cols(progs, a) + _witness_cols(progs, b)
        if any(GENERATED.search(c.split(" / ")[-1]) for c in cols):
            return LIB, "header missing (generated column name)", "column '" + next(c for c in cols if GENERATED.search(c.split(" / ")[-1]))[:60] + "'; "
        if not O.op_named(op, q.text):
            return LIM, "grounding: operation not named by the question", ""
        return LIM, "grounding: an operand named only in part", ""
    return None


def classify(it: dict, atom: dict, tables: list, q, res: dict, progs, mkinds: set | None = None) -> dict:
    """{family, cause, evidence[, witness]} for a missed atom."""
    def out(fam, cause, ev, **kw):
        d = {"family": fam, "cause": cause, "evidence": ev}
        d.update(kw)
        return d

    # source header cells for generated column names: the witness counts, the miss is hedgehog's
    t2, ev = relabel(it, tables, q)
    if ev:
        r2 = O.Programs(t2, q).search(atom, it["question"], 2)
        if r2["tight"]:
            w = (r2["witnesses"] or [""])[0]
            used = [e for e in ev if e.split("' is '", 1)[1].rsplit("' in the source", 1)[0] in w] or ev
            return out(LIB, "hedgehog header naming", "; ".join(used[:2]), witness=w)
    if it["dataset"] == "hitab" and it.get("cells"):
        h = hitab_cause(it, atom, tables, q)
        if h and h[0] == LIB:
            return out(*h)
    for kw, label, why in (({"relaxed_text": True}, "number kept as text", "reachable when numbers kept as text are parsed"),
                           ({"drop_total_labels": True}, "total row mixed into the data", "reachable when total-labelled data rows are dropped")):
        r3 = O.Programs(tables, q, **kw).search(atom, it["question"], 1)
        if r3["tight"]:
            return out(LIB, label, f"{why}: {(r3['witnesses'] or [''])[0][:160]}")
    if it["dataset"] == "hitab" and it.get("cells"):
        if h:
            return out(*h)
    # MiMoTable / RealHiTBench: by the best loose match, else by the question's shape
    lr = _loose_reason(progs, res, q) if res.get("strong") else None
    if lr:
        return out(lr[0], lr[1], (lr[2] + (res.get("witnesses") or [""])[0])[:240])
    if res.get("weak"):
        ex = res.get("weak_example", "")
        if re.search(r"\((col_[A-Z]+|[^()]*\(\d+\))\)", ex):
            return out(LIB, "header missing (generated column name)", ex[:200])
        return out(LIM, "grounding: no operand named by the question", ex[:200])
    shape = question_shape(it["question"])
    hint = f"; file has {', '.join(sorted(mkinds))}" if mkinds else ""
    allvals = np.array([O._f_relaxed(v) for t in tables for r in t.rows for v in r], dtype=float)
    tg = O.gold_targets(atom, it["question"])
    incell = bool(len(allvals) and len(tg) and O.match_mask(allvals, tg).any())
    if atom.get("moved") and it["dataset"] == "mimotable" and it.get("group") == "lookup":
        return out(GOLD, "lookup answer is no cell of the file", f"gold {atom['text']} of a lookup question is no source cell{hint}")
    return out(LIM, f"shape: {shape}", f"{'gold equals some cell, none named; ' if incell else ''}question: {it['question'][:120]}{hint}")
