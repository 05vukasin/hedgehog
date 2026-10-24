"""L: lookup reachability. Is a question's answer a cell of hedgehog's tables, under the right labels?

    python3 -I eval/accuracy/lookup.py [--datasets ...] [--show N] [--holdout]

- HiTab lookups (one gold answer cell with its header paths): eval/public/score.py `link_one`. Pass =
  the source cell is a cell of a normalized table, its stored number equals the source, the column name
  holds the gold top-header path as whole " / " segments in order (column exact), and every element of
  the gold left-header path is one of the row's labels (row exact = the strict label path).
- MiMoTable / RealHiTBench L atoms (acc.py), measured only when the atom is a cell of the source file
  (raw `hedgehog cells`; a text atom that is no source cell is a paraphrase: not attempted; a number atom
  that is no source cell moves to C). Pass = the atom is a cell of a normalized table, a column header or
  a table title (score.reach_tables "cell"); a number atom must also be grounded: its row labels or its
  column header share a content word with the question (score.reach_tables "loose"; a bare number in a
  wide table is otherwise found by chance). A date atom ("April 24") matches a normalized ISO date with
  the same month and day (and year when both give one).

Miss signatures (first that applies; computed from where the atom's source cell went):
  cell not in any table (row / column) / value changed: <how> / column header path differs /
  label path missing parent / row label differs / number not grounded by labels or header /
  generic column header (col_X).
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import acc  # noqa: E402
from acc import PS, norm_text, parse_number  # noqa: E402
from common import words  # noqa: E402

MONTHS = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october",
          "november", "december"]


def value_change(src: str, stored) -> str:
    """How a source cell's text changed in the normalized table (a short, generic signature)."""
    s = str(src).strip()
    st = "" if stored is None else str(stored)
    iso = re.match(r"^(\d{4})-(\d{2})-(\d{2})", st)
    md = re.search(r"(?i)\b(" + "|".join(m[:3] for m in MONTHS) + r")[a-z]*\.?\s+(\d{1,2})(?!\d)", s) or \
        re.search(r"(?i)(?<!\d)(\d{1,2})(?:st|nd|rd|th)?\s+(?:of\s+)?(" + "|".join(m[:3] for m in MONTHS) + r")", s)
    if iso and md:
        day = int(md.group(2)) if md.group(1).isalpha() else int(md.group(1))
        if int(iso.group(3)) != day:
            return "date text parsed with another day" + (" (day 1)" if iso.group(3) == "01" else "")
        return "date text changed"
    if re.fullmatch(r"\(\d{1,3}\)", s) and isinstance(stored, (int, float)) and stored < 0:
        return "item number '(n)' read as negative"
    p = parse_number(s)
    if isinstance(stored, (int, float)) and p is None and re.match(r"^\d{4}-\d{2}-\d{2}", s):
        return "date became a number (serial)"
    if isinstance(stored, (int, float)) and 20000 <= float(stored) <= 70000 and float(stored) == int(stored) and \
            (not p or s == str(int(stored))):
        return "date left as Excel serial"
    if p is not None and isinstance(stored, str):
        return "number kept as text"
    if p is not None and isinstance(stored, (int, float)):
        return "number changed"
    if stored in (None, ""):
        return "value emptied"
    return "text changed"


def source_hits(raw: dict, atom: "PS.Atom", date: dict | None, tables=None) -> list[tuple]:
    out = []
    for (sh, r, c), v in raw.items():
        if date is not None:
            if _date_text_match(v, date):
                out.append((sh, r, c, v))
            elif tables is not None and re.fullmatch(r"[2-6]\d{4}", str(v).strip()) and _date_text_match(f"date:{v}", date):
                # a whole number that is this date as an Excel serial, in a column named like a date
                loc = PS.locate(tables, sh, r, c)
                if not isinstance(loc, str) and re.search(r"(?i)\bdate\b|\bday\b|日期", loc[0].headers[loc[2]]):
                    out.append((sh, r, c, v))
        elif atom.matches_value(v):
            out.append((sh, r, c, v))
    return out


def _serial_iso(s: str) -> str | None:
    """`hedgehog cells` writes a date cell as date:<Excel serial>."""
    import datetime as dt
    m = re.fullmatch(r"date:(\d+(?:\.\d+)?)", s.strip())
    if not m:
        return None
    try:
        return (dt.date(1899, 12, 30) + dt.timedelta(days=int(float(m.group(1))))).isoformat()
    except (OverflowError, ValueError):
        return None


def _date_text_match(v: str, d: dict) -> bool:
    s = str(v)
    s = _serial_iso(s) or s
    m = re.match(r"^(\d{4})-(\d{2})-(\d{2})", s)
    if m:
        return int(m.group(2)) == d["m"] and int(m.group(3)) == d["d"] and (not d.get("y") or int(m.group(1)) == d["y"])
    mm = re.search(r"(?i)\b(" + "|".join(x[:3] for x in MONTHS) + r")[a-z]*\.?\s+(\d{1,2})(?!\d)", s)
    if mm:
        mon = [x[:3] for x in MONTHS].index(mm.group(1).lower()[:3]) + 1
        return mon == d["m"] and int(mm.group(2)) == d["d"]
    return False


def table_date_match(v, d: dict) -> bool:
    if not isinstance(v, str) or not d.get("m"):
        return False
    m = re.match(r"^(\d{4})-(\d{2})-(\d{2})", v)
    return bool(m) and int(m.group(2)) == d["m"] and (d.get("d") is None or int(m.group(3)) == d["d"]) and \
        (not d.get("y") or int(m.group(1)) == d["y"])


def check_atom(it: dict, a: dict, tables, raw: dict) -> dict:
    """One MiMoTable / RealHiTBench L atom."""
    date = {k: a[k] for k in ("m", "d", "y") if k in a} if a["kind"] == "date" else None
    at = PS.Atom(a["text"], "number" if a["kind"] == "number" else "text")
    hits = source_hits(raw, at, date, tables)
    if not hits:
        return {"text": a["text"], "attempted": False}
    rec = {"text": a["text"], "attempted": True}
    qw = words(it["question"])
    if date is not None:
        ok = any(table_date_match(v, date) or (isinstance(v, str) and _date_text_match(v, date))
                 for t in tables for row in t.rows for v in row)
        rec["pass"] = ok
    else:
        r = PS.reach_tables(tables, [at], qw)
        rec["pass"] = r["cell"] and (r["loose"] or at.num is None or year_key(tables, at, qw))
        rec["level"] = "loose" if r["loose"] else "cell" if r["cell"] else "none"
    if rec["pass"] and date is None and at.num is None:
        # a text answer found only in columns without a name (col_X): the agent cannot tell what it is
        # (unless the column is the row-label stub the source leaves unnamed too: unnamed_stub)
        cols = [t.headers[j] for t in tables for row in t.rows for j, v in enumerate(row)
                if at.matches_value(v) and not unnamed_stub(t, j, raw)]
        if cols and all(re.match(r"^col_[A-Z]+$", h.split(" / ")[-1]) for h in cols):
            rec["pass"] = False
            rec["cause"], rec["evidence"] = "answer column has no header (col_X)", f"{a['text']!r} only in {sorted(set(cols))[:3]}"
            return rec
    if not rec["pass"]:
        rec["cause"], rec["evidence"] = miss_signature(it, tables, hits, at, date)
    return rec


def year_key(tables, at, qwords: set[str]) -> bool:
    """A year asked for by a 'which year' question, found as the row key of a year stub: a label column whose
    values are all years (`Period | Total bankruptcies` with 2010 .. 2019). The year is the row's own label, so
    no other label grounds it; the question's 'year' and a column of nothing but years do."""
    x = at.num[0] if at.num is not None else None
    if x is None or at.num[2] or x != int(x) or not 1900 <= x <= 2100 or not ({"year", "years"} & qwords):
        return False
    for t, _i, c in PS.find_cells(tables, at):
        if c not in t.label_cols:
            continue
        vals = [row[c] for row in t.rows if c < len(row) and row[c] not in (None, "")]
        yrs = [v for v in vals if isinstance(v, (int, float)) and v == int(v) and 1900 <= v <= 2100]
        if len(vals) >= 3 and len(yrs) == len(vals):
            return True
    return False


def _a1_rows(rng: str) -> tuple[int, int] | None:
    m = re.fullmatch(r"[A-Z]+(\d+):[A-Z]+(\d+)", rng or "")
    return (int(m.group(1)), int(m.group(2))) if m else None


def unnamed_stub(t, j: int, raw: dict) -> bool:
    """Column j is the table's row-label stub and the source names it nowhere: every column to its left is a
    group, parent, numeric or unnamed column, it carries no original header, the table has header rows, and the source cells
    of its header rows are empty. Its row labels are then reachable as they are in the source: a `col_X` name loses nothing there
    (the col_X rule is for a header the source has and hedgehog lost)."""
    if j not in t.label_cols or j in getattr(t, "parent_cols", set()) or not re.fullmatch(r"col_[A-Z]+", t.headers[j]):
        return False
    meta = getattr(t, "colmeta", [])
    if j < len(meta) and (meta[j].get("role") == "group" or meta[j].get("original_header")):
        return False
    # the stub: only unnamed columns, the group column, parent columns, numeric columns (`Int# | LiST#` item
    # numbers before the names) and empty columns to its left (a named text column there, e.g. a title taken as the header, means
    # the header is misplaced, not absent)
    if any(not re.fullmatch(r"col_[A-Z]+(_key)?", t.headers[k]) and k not in getattr(t, "parent_cols", set())
           and not (k < len(meta) and meta[k].get("role") == "group") and k not in t.numeric_cols
           and any(k < len(r) and r[k] not in (None, "") for r in t.rows) for k in range(j)):
        return False
    src = (getattr(t, "manifest", {}) or {}).get("source", {})
    letter = t.letters[j] if j < len(t.letters) else None
    if not letter:
        return False
    rng, data = _a1_rows(src.get("range")), _a1_rows(src.get("data_range"))
    if not rng or not data or data[0] <= rng[0]:
        return False  # no header rows detected: a header taken as data would look the same
    col = 0
    for ch in letter:
        col = col * 26 + ord(ch) - 64
    return all(str(raw.get((t.sheet, r - 1, col - 1), "")).strip() == "" for r in range(rng[0], data[0]))


def question_keys(it: dict, raw: dict) -> list[tuple]:
    """Text cells of the source that the question quotes (codes, names, item numbers such as "(4)"):
    an agent looks them up, so they must reach the tables unchanged. Only cells of 3+ characters with a
    letter or digit (or a bracketed item number), at word boundaries in the question."""
    q = " " + norm_text(it["question"]) + " "
    qraw = it["question"]
    out, seen = [], set()
    for (sh, r, c), v in raw.items():
        sv = str(v).strip()
        if not sv or sv in seen or parse_number(sv) is not None and not re.fullmatch(r"\(\d{1,3}\)", sv):
            continue
        if re.fullmatch(r"\(\d{1,3}\)", sv):
            hit = re.search(r"(?<![\w(])" + re.escape(sv) + r"(?![\w)])", qraw) is not None
        else:
            n = norm_text(sv)
            hit = len(n) >= 3 and len(n) <= 60 and bool(re.search(r"\d", n) or len(n.split()) >= 2) and f" {n} " in q
        if hit:
            seen.add(sv)
            out.append((sh, r, c, sv))
    return out[:6]


def _key_date(sv: str):
    from invariants import _md
    m = re.match(r"^(\d{4})[-/.](\d{1,2})[-/.](\d{1,2})$", sv.strip())
    if m:
        return {"y": int(m.group(1)), "m": int(m.group(2)), "d": int(m.group(3))}
    my = re.fullmatch(r"(?i)(" + "|".join(MONTHS) + r")\.?,?\s+((?:19|20)\d\d)", sv.strip())
    if my:
        return {"m": MONTHS.index(my.group(1).lower()) + 1, "d": None, "y": int(my.group(2))}
    md = _md(sv)
    if md:
        y = re.search(r"(?<!\d)((?:19|20)\d\d)(?!\d)", sv)
        return {"m": md[0], "d": md[1], "y": int(y.group(1)) if y else None}
    return None


def check_key(it, key, tables, catalog: str) -> dict:
    """A question key passes when a table cell, a column header, a table title or the catalog holds it
    unchanged; a date key also when a cell holds the same date (ISO)."""
    sh, r, c, sv = key
    at = PS.Atom(sv, "text")
    d = _key_date(sv)
    exact = re.fullmatch(r"\(\d{1,3}\)", sv) is not None  # "(4)" must stay "(4)": norm_text would equal -4
    in_cells = any((not exact and at.matches_value(v)) or str(v).strip() == sv for t in tables for row in t.rows for v in row) or \
        (d is not None and any(table_date_match(v, d) for t in tables for row in t.rows for v in row))
    found = in_cells or (not exact and (PS.in_header_or_title(tables, at) or (bool(at.norm) and f" {at.norm} " in f" {norm_text(catalog)} ")))
    rec = {"text": sv, "attempted": True, "key": True, "pass": found}
    if found and not in_cells:
        # found only as column names "X", "X (2)", "X (3)": a row of equal values was taken as the header
        for t in tables:
            reps = [h for h in t.headers if re.fullmatch(re.escape(sv) + r"( \(\d+\))?", h)]
            if len(reps) >= 3:
                rec["pass"] = False
                rec["cause"] = "question key: only columns told apart by (2), (3) carry it"
                rec["evidence"] = f"{sv!r} names {len(reps)} columns of {t.id}: {reps[:3]} (a row of values taken as the header, or lost parents)"
                return rec
    if not found:
        rec["cause"], rec["evidence"] = miss_signature(it, tables, [(sh, r, c, sv)], at, None)
        rec["cause"] = "question key: " + rec["cause"]
    return rec


def miss_signature(it, tables, hits, at, date) -> tuple[str, str]:
    """Follow the atom's source cells into the normalized tables."""
    sigs = []
    for sh, r, c, v in hits[:20]:
        loc = PS.locate(tables, sh, r, c)
        a1 = f"{sh}!{PS.col_letter(c)}{r + 1}"
        if isinstance(loc, str):
            hdr = any(any(oh.get("cell") == f"{PS.col_letter(c)}{r + 1}" for col in getattr(t, "colmeta", []) for oh in col.get("original_header") or [])
                      for t in tables if t.sheet == sh)
            sigs.append(("source cell became a column header" if hdr else f"cell not in any table ({loc})", f"{a1}={v!r}"))
            continue
        t, i, j = loc
        stored = t.rows[i][j] if j < len(t.rows[i]) else None
        if re.fullmatch(r"\(\d{1,3}\)", str(v).strip()):
            ok = str(stored).strip() == str(v).strip()
        else:
            ok = ((table_date_match(stored, date) or (isinstance(stored, str) and _date_text_match(stored, date)))
                  if date is not None else at.matches_value(stored))
        if not ok:
            sigs.append((f"value changed: {value_change(v, stored)}", f"{a1}={v!r} -> {stored!r} ({t.id} '{t.headers[j]}')"))
            continue
        h = t.headers[j]
        if re.match(r"^col_[A-Z]+$", h) or re.match(r"^col_[A-Z]+$", h.split(" / ")[-1]):
            sigs.append(("generic column header (col_X)", f"{a1} in {t.id} column '{h}'"))
        else:
            sigs.append(("number not grounded by labels or header", f"{a1} in {t.id} column '{h}', labels {t.labels(i, j)[:3]}"))
    order = ["value changed", "source cell became", "cell not in any table", "generic column header", "number not grounded"]
    sigs.sort(key=lambda s: next((k for k, p in enumerate(order) if s[0].startswith(p)), 9))
    return sigs[0] if sigs else ("atom not located", "")


def _forms(x: str) -> set[str]:
    """A label as written, without a bracketed qualifier or unit, without a glued footnote letter."""
    n = norm_text(x)
    out = {n, norm_text(re.sub(r"\([^)]*\)|\[[^]]*\]", " ", str(x)))}
    m = re.fullmatch(r"(.*[a-z]{3})[a-z]", n)  # "totalb" (a footnote letter glued on) -> "total"
    if m and len(n) > 4:
        out.add(m.group(1))
    m = re.fullmatch(r"((?:19|20)\d\d)[a-z]", n)  # "2015r" (a revision mark glued on a year) -> "2015"
    if m:
        out.add(m.group(1))
    return {f for f in out if f}


def _el_eq(gold: str, label: str) -> bool:
    return bool(_forms(gold) & _forms(label)) or norm_text(gold) in _forms(label)


def stub_key_cols(t) -> list[int]:
    """Non-text columns of the row stub that key the rows: the first column or a column left of a text label
    column, not a label column itself, with a distinct value on every row where it is filled (most rows)."""
    first_label = min((c for c in t.label_cols if c not in getattr(t, "parent_cols", set())), default=None)
    out = []
    for k in range(len(t.headers)):
        if k in t.label_cols or not (k == 0 or (first_label is not None and k < first_label)):
            continue
        vals = [r[k] for r in t.rows if k < len(r) and r[k] not in (None, "")]
        # distinct where filled (a total row without a rank is no key), filled on most rows
        if len(vals) >= 2 and 2 * len(vals) >= len(t.rows) and len({str(v) for v in vals}) == len(vals):
            out.append(k)
    return out


def _key_eq(gold: str, v) -> bool:
    """A gold label equals a stub key: as text, or as a number ('0.20' = 0.2, '23' = 23)."""
    if _el_eq(gold, str(v)):
        return True
    g = parse_number(gold)
    try:
        return g is not None and isinstance(v, (int, float)) and not isinstance(v, bool) and abs(float(g[0]) - float(v)) < 1e-9
    except (TypeError, ValueError):
        return False


def hitab_paths(t, i: int, j: int, cell: dict) -> tuple[bool, bool, bool]:
    """(row path exact, leaf exact, column path exact) with HiTab's own normalisations allowed: case,
    punctuation, a bracketed unit or qualifier the annotators dropped, a glued footnote letter; a year
    row label kept as a number counts as a label; gold elements without letters or digits ('-') are
    placeholders and not checked."""
    gold_left = [x for x in cell["left_path"] if re.search(r"\w", x)]
    gold_top = [x for x in cell["top_path"] if re.search(r"\w", x)]
    labels = t.labels(i, j) + [str(int(v)) for k, v in enumerate(t.rows[i]) if k != j and isinstance(v, (int, float))
                               and not isinstance(v, bool) and v == int(v) and 1900 <= v <= 2100]
    # the row's key in a stub column that is not text (HiTab roots rows at 'Rank' 23, 'Number of children' 0,
    # 'Growth rate' 0.20): the first column, or one left of a label column, with a distinct value on every row
    keys = [t.rows[i][k] for k in stub_key_cols(t) if k != j and k < len(t.rows[i]) and t.rows[i][k] not in (None, "")]
    # HiTab roots its left tree at the label column's header or at the table's total row ("all
    # industries" above "manufacturing industries"), also a total-worded row kept as data; a root found
    # there is not a missing parent.
    roots = [t.headers[c] for c in t.label_cols]
    if t.row_types:
        roots += [L for k, rt in enumerate(t.row_types) if rt != "data" for L in t.labels(k)]
    # ... also at a total-worded row hedgehog keeps as data ('All businesses' over 'Businesses majority-owned by
    # visible minorities': percentages of a population, no sum): only the root, every other element is checked
    roots += [L for k in range(len(t.rows)) for L in t.labels(k) if PS.TOTAL_RX.search(norm_text(L))]
    row = all(any(_el_eq(g, L) for L in labels) or any(_key_eq(g, v) for v in keys)
              or (n == 0 and len(gold_left) > 1 and any(_el_eq(g, R) for R in roots))
              for n, g in enumerate(gold_left))
    leaf = not gold_left or any(_el_eq(gold_left[-1], L) for L in labels) or any(_key_eq(gold_left[-1], v) for v in keys)
    segs = t.headers[j].split(" / ")
    k = 0
    col = True
    for g in gold_top:
        # a gold element that itself holds " / " ('unemployed / job vacancies') is a run of segments
        n = g.count(" / ") + 1
        while k + n <= len(segs) and not _el_eq(g, " / ".join(segs[k:k + n])):
            k += 1
        if k + n > len(segs):
            col = False
            break
        k += n
    return row, leaf, col


def hitab_lookup(it: dict, tables, raw: dict) -> dict:
    r = PS.link_one(tables, {"sheet": it["sheet"], "cells": it["cells"]}, raw)
    rec = {"text": str(it["atoms"][0]["text"]) if it["atoms"] else "", "attempted": True}
    cell = it["cells"][0]
    a1 = f"{it['sheet']}!{cell['a1']}"
    if not r["found"]:
        rec.update(**{"pass": False, "cause": f"cell not in any table ({r['reason']})", "evidence": f"{a1}={cell['raw']!r}"})
        return rec
    val_ok = r["value"] in ("equal", "equal (percent as fraction)", "equal (accounting negative)", "source not a number")
    t, i, j = PS.locate(tables, it["sheet"], cell["row"], cell["col"])
    row_ok, leaf_ok, col_ok = hitab_paths(t, i, j, cell)
    rec["pass"] = val_ok and col_ok and row_ok
    if rec["pass"]:
        return rec
    if not val_ok:
        rec["cause"] = f"value changed: {r['value']}"
        rec["evidence"] = f"{a1}: source {r['source']!r} stored {r['stored']!r}"
    elif not row_ok:
        gold = [x for x in cell["left_path"] if x.strip()]
        rec["cause"] = "label path missing parent" if leaf_ok else "row label differs"
        rec["evidence"] = f"{a1}: gold {gold} vs labels {r['labels'][:4]}"
    else:
        rec["cause"] = "column header path differs"
        rec["evidence"] = f"{a1}: gold {cell['top_path']} vs '{r['header']}'"
    return rec


def run_item(it: dict) -> dict:
    out = {"id": it["id"], "dataset": it["dataset"], "group": it["group"], "side": it["side"], "atoms": []}
    ds = it["dataset"]
    loaded = [acc.load(ds, f) for f in it["files"]]
    tables = [t for (_, _, _, ts) in loaded for t in ts]
    raw = {}
    for (_, rw, _, _) in loaded:
        raw.update(rw)
    if ds == "hitab":
        if it.get("hitab_lookup"):
            out["atoms"].append(hitab_lookup(it, tables, raw))
    else:
        for a in it["atoms"]:
            if a["cls"] != "L":
                continue
            out["atoms"].append(check_atom(it, a, tables, raw))
        if it["atoms"]:
            catalog = "\n".join(cat for (_, _, cat, _) in loaded)
            for k in question_keys(it, raw):
                out["atoms"].append(check_key(it, k, tables, catalog))
    att = [a for a in out["atoms"] if a["attempted"]]
    out["attempted"] = bool(att)
    out["pass"] = bool(att) and all(a["pass"] for a in att)
    return out


def run(datasets, side: str) -> list[dict]:
    rows = []
    for ds in datasets:
        for it in acc.load_items(ds, side):
            if not it.get("atoms"):
                continue
            r = run_item(it)
            if r["attempted"]:
                rows.append(r)
    return rows


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--datasets", default="hitab,mimotable,realhitbench")
    ap.add_argument("--holdout", action="store_true")
    ap.add_argument("--show", type=int, default=0)
    a = ap.parse_args()
    side = "holdout" if a.holdout else "dev"
    rows = run(a.datasets.split(","), side)
    for ds in a.datasets.split(","):
        rr = [r for r in rows if r["dataset"] == ds]
        causes = Counter(x["cause"] for r in rr for x in r["atoms"] if x["attempted"] and not x["pass"])
        print(ds, len(rr), sum(r["pass"] for r in rr), dict(causes.most_common(12)))
        if a.show:
            for r in [r for r in rr if not r["pass"]][: a.show]:
                for x in r["atoms"]:
                    if x["attempted"] and not x["pass"]:
                        print(f"   {r['id']} {x['text'][:40]!r}: {x['cause']} | {x['evidence'][:160]}")


if __name__ == "__main__":
    main()
