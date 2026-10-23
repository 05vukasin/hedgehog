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
    p = parse_number(s)
    if isinstance(stored, (int, float)) and p is None and re.match(r"^\d{4}-\d{2}-\d{2}", s):
        return "date became a number (serial)"
    if isinstance(stored, (int, float)) and 20000 <= float(stored) <= 70000 and float(stored) == int(stored) and \
            not p:
        return "date left as Excel serial"
    if p is not None and isinstance(stored, str):
        return "number kept as text"
    if p is not None and isinstance(stored, (int, float)):
        return "number changed"
    if stored in (None, ""):
        return "value emptied"
    return "text changed"


def source_hits(raw: dict, atom: "PS.Atom", date: dict | None) -> list[tuple]:
    out = []
    for (sh, r, c), v in raw.items():
        if date is not None:
            if _date_text_match(v, date):
                out.append((sh, r, c, v))
        elif atom.matches_value(v):
            out.append((sh, r, c, v))
    return out


def _date_text_match(v: str, d: dict) -> bool:
    s = str(v)
    m = re.match(r"^(\d{4})-(\d{2})-(\d{2})", s)
    if m:
        return int(m.group(2)) == d["m"] and int(m.group(3)) == d["d"] and (not d.get("y") or int(m.group(1)) == d["y"])
    mm = re.search(r"(?i)\b(" + "|".join(x[:3] for x in MONTHS) + r")[a-z]*\.?\s+(\d{1,2})(?!\d)", s)
    if mm:
        mon = [x[:3] for x in MONTHS].index(mm.group(1).lower()[:3]) + 1
        return mon == d["m"] and int(mm.group(2)) == d["d"]
    return False


def table_date_match(v, d: dict) -> bool:
    if not isinstance(v, str):
        return False
    m = re.match(r"^(\d{4})-(\d{2})-(\d{2})", v)
    return bool(m) and int(m.group(2)) == d["m"] and int(m.group(3)) == d["d"] and (not d.get("y") or int(m.group(1)) == d["y"])


def check_atom(it: dict, a: dict, tables, raw: dict) -> dict:
    """One MiMoTable / RealHiTBench L atom."""
    date = {k: a[k] for k in ("m", "d", "y") if k in a} if a["kind"] == "date" else None
    at = PS.Atom(a["text"], "number" if a["kind"] == "number" else "text")
    hits = source_hits(raw, at, date)
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
        rec["pass"] = r["cell"] and (r["loose"] or at.num is None)
        rec["level"] = "loose" if r["loose"] else "cell" if r["cell"] else "none"
    if not rec["pass"]:
        rec["cause"], rec["evidence"] = miss_signature(it, tables, hits, at, date)
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
    return {f for f in out if f}


def _el_eq(gold: str, label: str) -> bool:
    return bool(_forms(gold) & _forms(label)) or norm_text(gold) in _forms(label)


def hitab_paths(t, i: int, j: int, cell: dict) -> tuple[bool, bool, bool]:
    """(row path exact, leaf exact, column path exact) with HiTab's own normalisations allowed: case,
    punctuation, a bracketed unit or qualifier the annotators dropped, a glued footnote letter; a year
    row label kept as a number counts as a label; gold elements without letters or digits ('-') are
    placeholders and not checked."""
    gold_left = [x for x in cell["left_path"] if re.search(r"\w", x)]
    gold_top = [x for x in cell["top_path"] if re.search(r"\w", x)]
    labels = t.labels(i, j) + [str(int(v)) for k, v in enumerate(t.rows[i]) if k != j and isinstance(v, (int, float))
                               and not isinstance(v, bool) and v == int(v) and 1900 <= v <= 2100]
    # HiTab roots its left tree at the label column's header or at the table's total row ("all
    # industries" above "manufacturing industries"); a root found there is not a missing parent.
    roots = [t.headers[c] for c in t.label_cols]
    if t.row_types:
        roots += [L for k, rt in enumerate(t.row_types) if rt != "data" for L in t.labels(k)]
    row = all(any(_el_eq(g, L) for L in labels) or (n == 0 and len(gold_left) > 1 and any(_el_eq(g, R) for R in roots))
              for n, g in enumerate(gold_left))
    leaf = not gold_left or any(_el_eq(gold_left[-1], L) for L in labels)
    segs = t.headers[j].split(" / ")
    k = 0
    col = True
    for g in gold_top:
        while k < len(segs) and not _el_eq(g, segs[k]):
            k += 1
        if k == len(segs):
            col = False
            break
        k += 1
    return row, leaf, col


def hitab_lookup(it: dict, tables, raw: dict) -> dict:
    r = PS.link_one(tables, {"sheet": it["sheet"], "cells": it["cells"]}, raw)
    rec = {"text": str(it["atoms"][0]["text"]) if it["atoms"] else "", "attempted": True}
    cell = it["cells"][0]
    a1 = f"{it['sheet']}!{cell['a1']}"
    if not r["found"]:
        rec.update(**{"pass": False, "cause": f"cell not in any table ({r['reason']})", "evidence": f"{a1}={cell['raw']!r}"})
        return rec
    val_ok = r["value"] in ("equal", "equal (percent as fraction)", "source not a number")
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
