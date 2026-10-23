"""M: nothing misleading. Invariants checked on every normalized table of the dev files (no gold needed).

    python3 -I eval/accuracy/invariants.py [--datasets ...] [--show N] [--holdout]

Each violation names the file, table, source cell and the evidence. Conservative by design (a violation
must be certain, not suspected):

  M1 total row in data      a data row whose own label reads like a total (eval/public score.TOTAL_RX)
                            AND whose number equals the sum of the other data rows of its block (above
                            it since the last non-data row, or below it) in some column (2+ summands,
                            0.1 % or 1 unit) - an aggregate an agent would double count;
  M2 percent mixed          a numeric column whose source cells mix "12 %" and plain numbers;
     percent scale          a column whose source cells are "N %" but some values are stored as N and
                            others as N / 100;
  M3 measures mixed         a numeric column without a unit where 2+ rows are labelled as percent /
                            rate (values within 0-100) and 2+ other rows hold values > 100;
  M4 date out of range      a date outside 1900-2100;
     dates collapsed        2+ source dates with different month/day stored as one value;
     date day changed       a source text "March 25" stored as a date with another day;
     Excel serial as number a column named like a date whose values are all whole numbers 20000-70000;
  M5 header missing         a column named col_<letter> (no header found);
     duplicate headers      columns told apart only by a " (2)" suffix (the header lost its parent);
     number as header       a column header that is a plain number (not a year): a data row taken as
                            the header.
"""

from __future__ import annotations

import argparse
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import acc  # noqa: E402
from acc import PS, norm_text, parse_number  # noqa: E402

MONTHS = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
PCT_LABEL = re.compile(r"%|\bpercent|\bper cent|\brate\b|\bshare\b|百分|比例|占比|比率")
DATE_HDR = re.compile(r"\bdate\b|\bday\b|\btime\b|日期|时间", re.I)


def _num(v):
    return float(v) if isinstance(v, (int, float)) and not isinstance(v, bool) else None


def _raw_of(t, raw: dict, i: int, j: int):
    letter = t.letters[j] if j < len(t.letters) else None
    if not letter or not t.src_rows or t.src_rows[i] < 1:
        return None, None
    c = 0
    for ch in letter:
        c = c * 26 + ord(ch) - 64
    key = (t.sheet, t.src_rows[i] - 1, c - 1)
    return raw.get(key), f"{t.sheet}!{letter}{t.src_rows[i]}"


def _md(s: str):
    """(month, day) of a source date text, or None."""
    s = str(s).strip()
    m = re.match(r"^(\d{4})[-/.](\d{1,2})[-/.](\d{1,2})", s)
    if m:
        return int(m.group(2)), int(m.group(3))
    m = re.search(r"(?i)\b(" + "|".join(MONTHS) + r")[a-z]*\.?\s+(\d{1,2})(?!\d)", s)
    if m:
        return MONTHS.index(m.group(1).lower()[:3]) + 1, int(m.group(2))
    m = re.search(r"(?i)(?<!\d)(\d{1,2})(?:st|nd|rd|th)?\s+(?:of\s+)?(" + "|".join(MONTHS) + r")", s)
    if m:
        return MONTHS.index(m.group(2).lower()[:3]) + 1, int(m.group(1))
    m = re.match(r"^(\d{1,2})月(\d{1,2})日", s)
    if m:
        return int(m.group(1)), int(m.group(2))
    return None


def _a1(a1: str) -> tuple[int, int]:
    return PS.parse_a1(a1) if hasattr(PS, "parse_a1") else __import__("common").parse_a1(a1)


def _lost_parent(t, raw: dict, j: int) -> str | None:
    """Columns told apart only by " (2)": is there a header text right above them (the group they belong
    to) that no column name of the table carries? Then the column lost its parent."""
    cm = getattr(t, "colmeta", [])
    if j >= len(cm) or not cm[j].get("original_header"):
        return None
    oh = cm[j]["original_header"]
    try:
        r0, c0 = min(_a1(x["cell"]) for x in oh if x.get("cell"))
    except ValueError:
        return None
    used = {norm_text(x.get("text")) for x in oh}  # this column's own header texts
    used.add(norm_text(t.title))
    for c in range(c0, max(-1, c0 - 12), -1):
        x = raw.get((t.sheet, r0 - 1, c))
        if x not in (None, ""):
            n = norm_text(x)
            if parse_number(x) is not None or not re.search(r"[^\W\d_]", str(x)):
                return None  # a number above: data of something else, not a header
            return f"{x!r}" if n and n not in used and n not in norm_text(t.headers[j]) else None
    return None


def check_table(t, raw: dict) -> list[dict]:
    out = []
    n = len(t.rows)
    dtypes = getattr(t, "dtypes", ["?"] * len(t.headers))
    rt = t.row_types or ["data"] * n
    own = sorted(t.label_cols - t.parent_cols)

    def v(kind, j, i, ev):
        cell = _raw_of(t, raw, i, j)[1] if i is not None and j is not None else None
        out.append({"inv": kind, "table": t.id, "cell": cell, "evidence": ev})

    numcols = [j for j, d in enumerate(dtypes) if d in ("integer", "number", "percent", "currency", "float")]
    # M1
    for i in range(n):
        if rt[i] != "data":
            continue
        lab = " ".join(norm_text(t.rows[i][k]) for k in own if k < len(t.rows[i]) and t.rows[i][k] not in (None, ""))
        if not lab or not PS.TOTAL_RX.search(lab):
            continue
        lo = i
        while lo > 0 and rt[lo - 1] == "data":
            lo -= 1
        hi = i
        while hi + 1 < n and rt[hi + 1] == "data":
            hi += 1
        for j in numcols:
            x = _num(t.rows[i][j]) if j < len(t.rows[i]) else None
            if x is None or x == 0:
                continue
            for blk in (range(lo, i), range(i + 1, hi + 1)):
                vals = [_num(t.rows[k][j]) for k in blk if j < len(t.rows[k])]
                vals = [y for y in vals if y is not None]
                if len([y for y in vals if y]) >= 2 and abs(sum(vals) - x) <= 1e-3 * abs(x) and \
                        not all(abs(y) < 1e-12 for y in vals):
                    v("M1 total row in data", j, i, f"row '{lab[:60]}' = {x:g} = sum of {len(vals)} data rows in '{t.headers[j][:50]}'")
                    break
            else:
                continue
            break
    # M2 / M3 / M4 per column
    for j in range(len(t.headers)):
        d = dtypes[j] if j < len(dtypes) else "?"
        if d in ("integer", "number", "percent", "currency", "float"):
            pct, plain, scale = [], [], set()
            for i in range(n):
                x = _num(t.rows[i][j]) if j < len(t.rows[i]) else None
                r, a1 = _raw_of(t, raw, i, j)
                if x is None or r is None:
                    continue
                p = parse_number(r)
                if p is None:
                    continue
                if p[2]:
                    pct.append(a1)
                    if p[0] != 0:
                        scale.add("fraction" if abs(x - p[0] / 100) <= 1e-9 * max(1, abs(x)) else
                                  "as shown" if abs(x - p[0]) <= 1e-9 * max(1, abs(x)) else "other")
                else:
                    plain.append(a1)
            if pct and plain and len(pct) + len(plain) >= 3:
                v("M2 percent mixed", j, None, f"'{t.headers[j][:50]}': {len(pct)} percent cells (e.g. {pct[0]}) and {len(plain)} plain (e.g. {plain[0]})")
            if {"fraction", "as shown"} <= scale:
                v("M2 percent scale", j, None, f"'{t.headers[j][:50]}': some percents stored as fractions, others as shown")
            # M3
            unit = (t.colmeta[j].get("unit") if j < len(getattr(t, "colmeta", [])) else None)
            if not unit:
                pr, other = [], []
                for i in range(n):
                    x = _num(t.rows[i][j]) if j < len(t.rows[i]) else None
                    if x is None or rt[i] != "data":
                        continue
                    lab = " ".join(str(t.rows[i][k]) for k in sorted(t.label_cols) if k < len(t.rows[i]) and t.rows[i][k] not in (None, ""))
                    (pr if PCT_LABEL.search(lab.lower()) else other).append(x)
                if len(pr) >= 2 and len(other) >= 2 and all(0 <= x <= 100 for x in pr) and sum(x > 100 for x in other) >= 2 \
                        and not PCT_LABEL.search(t.headers[j].lower()):
                    v("M3 measures mixed", j, None, f"'{t.headers[j][:50]}': {len(pr)} percent/rate rows and {len(other)} other rows (values > 100)")
            # Excel serial as number
            if DATE_HDR.search(t.headers[j]) and d == "integer":
                xs = [_num(r[j]) for r in t.rows if j < len(r) and _num(r[j]) is not None]
                if len(xs) >= 1 and all(20000 <= x <= 70000 and x == int(x) for x in xs):
                    v("M4 Excel serial as number", j, 0, f"'{t.headers[j][:50]}': {len(xs)} values like {int(xs[0])}")
        if d in ("date", "datetime"):
            by_val = defaultdict(set)
            for i in range(n):
                x = t.rows[i][j] if j < len(t.rows[i]) else None
                if not isinstance(x, str):
                    continue
                m = re.match(r"^(\d{4})-(\d{2})-(\d{2})", x)
                if not m:
                    continue
                y = int(m.group(1))
                r, a1 = _raw_of(t, raw, i, j)
                if not 1900 <= y <= 2100 and str(y) not in str(r):  # a year the source itself writes is kept
                    v("M4 date out of range", j, i, f"'{t.headers[j][:50]}' = {x} (source {r!r})")
                md = _md(r) if r is not None else None
                if md and (md[0], md[1]) != (int(m.group(2)), int(m.group(3))) and not re.match(r"^\d+(\.\d+)?$", str(r)):
                    v("M4 date day changed", j, i, f"source {r!r} stored {x}")
                if md:
                    by_val[x[:10]].add(md)
            for x, mds in by_val.items():
                if len(mds) >= 2:
                    v("M4 dates collapsed", j, None, f"'{t.headers[j][:50]}': {len(mds)} source dates stored as {x}")
    # M6 "(4)" (an outline / item number next to plain numbers 1, 2, 3 in the same column) stored as -4
    for j in range(len(t.headers)):
        paren, plain, hit = 0, 0, None
        for i in range(n):
            x = t.rows[i][j] if j < len(t.rows[i]) else None
            r, a1 = _raw_of(t, raw, i, j)
            rs = str(r or "").strip()
            m = re.fullmatch(r"\((\d{1,3})\)", rs)
            if m:
                paren += 1
                if hit is None and str(x).strip() in (f"-{int(m.group(1))}", f"-{int(m.group(1))}.0"):
                    hit = (i, rs, x)
            elif re.fullmatch(r"\d{1,3}", rs):
                plain += 1
        if hit and plain:
            v("M6 '(n)' read as negative", j, hit[0], f"source {hit[1]!r} stored {hit[2]!r} in '{t.headers[j][:40]}' ({paren} such cells next to {plain} plain item numbers)")
    # M5 headers
    base = [re.sub(r" \(\d+\)$", "", h) for h in t.headers]
    j = 0
    while j < len(base):  # 3+ adjacent columns with one name: a row of equal values taken as the header
        k = j
        while k + 1 < len(base) and base[k + 1] == base[j]:
            k += 1
        if k - j + 1 >= 3:
            v("M5 repeated header names", j, None, f"{k - j + 1} adjacent columns named '{base[j][:50]}' (a data row taken as the header?)")
        j = k + 1
    names = Counter(base)
    for j, h in enumerate(t.headers):
        leaf = h.split(" / ")[-1]
        if re.match(r"^col_[A-Z]+$", leaf):
            v("M5 header missing", j, None, f"column '{h}'")
        elif names[re.sub(r" \(\d+\)$", "", h)] >= 2 and re.search(r" \(\d+\)$", h):
            lost = _lost_parent(t, raw, j)
            if lost:
                v("M5 duplicate headers", j, None, f"column '{h}': the header above ({lost}) is not in any column name")
        else:
            p = parse_number(leaf)
            if p and not (p[0] == int(p[0]) and 1900 <= p[0] <= 2100) and (abs(p[0]) >= 100 or p[1] > 0) and \
                    not re.match(r"^\d{4}[-/]\d", leaf):
                v("M5 number as header", j, None, f"column '{h}'")
    return out


def run(datasets, side: str) -> list[dict]:
    rows = []
    for ds in datasets:
        files = sorted(acc.side_files(ds, side))
        for f in files:
            meta, raw, _, tables = acc.load(ds, f)
            for t in tables:
                for x in check_table(t, raw):
                    rows.append({"dataset": ds, "file": f, **x})
            acc.load.cache_clear() if len(files) > 400 and files.index(f) % 300 == 299 else None
    return rows


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--datasets", default="hitab,mimotable,realhitbench")
    ap.add_argument("--holdout", action="store_true")
    ap.add_argument("--show", type=int, default=0)
    a = ap.parse_args()
    rows = run(a.datasets.split(","), "holdout" if a.holdout else "dev")
    for ds in a.datasets.split(","):
        rr = [r for r in rows if r["dataset"] == ds]
        print(ds, len(rr), dict(Counter(r["inv"] for r in rr)))
        if a.show:
            seen = Counter()
            for r in rr:
                if seen[r["inv"]] < a.show:
                    seen[r["inv"]] += 1
                    print(f"   {r['inv']}: {r['file']} {r['table']} {r['cell']} | {r['evidence'][:150]}")


if __name__ == "__main__":
    main()
