"""Model-free scores on public spreadsheet benchmarks: hedgehog vs naive baselines (no LLM anywhere).

    python -I eval/public/score.py [--datasets mimotable,realhitbench,hitab] [--examples 3]

Reads the converters' items (<data>/public/<name>/items.jsonl) and the tool outputs of run_hedgehog.py
and run_baselines.py (<work>/<name>/{hh,pandas,markitdown}/), writes results/<name>.json and
RESULTS.md. Run it with an interpreter that has tiktoken (o200k token counts, as eval/info/score.py),
e.g. $SWAP_DIR/venv-markitdown/bin/python -I; without it tokens are characters / 4 (the report says
which). Metrics (README.md has the full definitions):

  B  value-label linking (HiTab lookups: one answer cell with its gold header paths):
     found / value fidelity / column header exact|lenient / row labels exact|lenient / both.
  C  answer reachability: per QA item whose answer atoms are all cells of the source file ("verbatim"):
     (i) in the catalog text (hedgehog) or the full markdown (MarkItDown); (ii) a cell of one table
     (hedgehog normalized table / pandas DataFrame), loose (+ a row label or the column header shares a
     word with the question) and strict (+ both a row label and the column header do).
     Items whose atoms are not all in the file need a computation ("needs a query"): for those,
     operand columns are checked for being clean numeric columns (HiTab: the gold operand cells;
     MiMoTable / RealHiTBench: columns whose header shares a word with the question - a heuristic).
  D  cost: catalog / markdown size (chars, tokens), open time, peak RSS, failures by error class.
"""

from __future__ import annotations

import argparse
import csv
import gzip
import json
import re
import statistics
import sys
import time
from collections import Counter, defaultdict
from functools import lru_cache
from pathlib import Path

csv.field_size_limit(1 << 30)
sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import (HERE, TOKENIZER, col_letter, count_tokens, data_dir, norm_text, parse_number,  # noqa: E402
                    read_jsonl, words, work_dir, _NUMBER)
from run_hedgehog import key_of  # noqa: E402

DATASETS = ("mimotable", "realhitbench", "hitab")
TOTAL_RX = re.compile(r"^(all|total)\b|\btotals?\b|\bsubtotal\b|\bgrand total\b|合计|总计|小计|共计|总和|\bsumme\b|\bgesamt\b|\binsgesamt\b")
NUMERIC_HH = {"number", "integer", "percent", "currency", "float"}


# ------------------------------------------------------------------------------------------------
# matching


def num_eq(ans: tuple[float, int, bool], x: float) -> bool:
    """Exact numeric equality, or the answer rounded as shown (only when it shows decimals); a percent
    answer also matches its fraction."""
    v, d, pct = ans
    for c, dd in [(v, d)] + ([(v / 100.0, d + 2)] if pct else []):
        if abs(c - x) <= 1e-9 * max(1.0, abs(c)):
            return True
        if dd > 0 and round(x, min(dd, 12)) == round(c, min(dd, 12)):
            return True
    return False


class Atom:
    def __init__(self, text: str, kind: str):
        self.text = text
        self.num = parse_number(text) if kind == "number" else None
        self.norm = norm_text(text)
        self.nospace = self.norm.replace(" ", "")

    def matches_value(self, v) -> bool:
        """A cell value (number or text)."""
        if v is None or v == "":
            return False
        if self.num is not None:
            x = v if isinstance(v, (int, float)) and not isinstance(v, bool) else None
            if x is None:
                p = parse_number(v)
                x = p[0] if p else None
            return x is not None and num_eq(self.num, float(x))
        return bool(self.norm) and norm_text(v) == self.norm

    def in_text(self, text_norm: str, numbers: list[float]) -> bool:
        """Verbatim in a text (catalog, markdown): a number among its numbers, a phrase at word bounds."""
        if self.num is not None:
            return any(num_eq(self.num, x) for x in numbers)
        if not self.norm:
            return False
        if re.search(r"[㐀-鿿]", self.norm):
            return self.norm in text_norm
        return f" {self.norm} " in f" {text_norm} "


def text_numbers(text: str) -> list[float]:
    out = []
    for m in _NUMBER.finditer(text):
        p = parse_number(m.group(0))
        if p:
            out.append(p[0])
    return out


def is_numeric_value(v) -> bool:
    return isinstance(v, (int, float)) and not isinstance(v, bool)


# ------------------------------------------------------------------------------------------------
# tool outputs


class Table:
    """One table of a tool: rows of values, column headers, row source numbers, row types."""

    def __init__(self, tid: str, sheet: str, headers: list[str], rows: list[list], src_rows: list[int] | None,
                 row_types: list[str] | None, label_cols: set[int], numeric_cols: set[int], letters: list[str | None]):
        self.id, self.sheet, self.headers, self.rows = tid, sheet, headers, rows
        self.src_rows, self.row_types, self.label_cols = src_rows, row_types, label_cols
        self.numeric_cols, self.letters = numeric_cols, letters
        self.by_src = {r: i for i, r in enumerate(src_rows)} if src_rows else {}
        self.title = ""

    def labels(self, i: int, skip: int | None = None) -> list[str]:
        return [str(self.rows[i][c]) for c in sorted(self.label_cols) if c != skip and c < len(self.rows[i])
                and self.rows[i][c] not in (None, "")]

    def col_clean(self, c: int) -> tuple[bool, bool]:
        """(numeric column, no total-labelled row counted as data)."""
        numeric = c in self.numeric_cols
        mixed = False
        for i, row in enumerate(self.rows):
            lab = " ".join(norm_text(x) for x in self.labels(i, c))
            if TOTAL_RX.search(lab) and (self.row_types is None or self.row_types[i] == "data"):
                if c < len(row) and row[c] not in (None, ""):
                    mixed = True
                    break
        return numeric, not mixed


def _cast(v: str):
    if v == "":
        return None
    try:
        return int(v) if re.fullmatch(r"-?\d+", v) else float(v) if re.fullmatch(r"-?\d+\.\d*(e-?\d+)?|-?\d+e-?\d+", v, re.I) else v
    except ValueError:
        return v


@lru_cache(maxsize=64)
def hh_load(dataset: str, rel: str):
    d = work_dir() / dataset / "hh" / key_of(rel)
    meta = json.loads((d / "meta.json").read_text()) if (d / "meta.json").exists() else None
    if not meta or meta.get("skipped"):
        return meta, {}, "", []
    raw = {}
    for line in (d / "cells.tsv").read_text().splitlines():
        f = line.split("\t", 4)
        if len(f) == 5:
            raw[(f[0], int(f[1]), int(f[2]))] = f[4]
    catalog = (d / "catalog.txt").read_text() if (d / "catalog.txt").exists() else ""
    tables = []
    mf = next((d / "norm").glob("*.manifest.json"), None)
    if mf:
        m = json.loads(mf.read_text())
        csvs = m.get("output", {}).get("csv", [])
        for t in m.get("tables", []):
            path = next((d / "norm" / c for c in csvs if re.search(rf"\.{re.escape(t['id'])}(_|\.csv$)", c)), None)
            if not path or not path.exists():
                continue
            with open(path, newline="") as f:
                rd = list(csv.reader(f))
            if not rd:
                continue
            cols = t["columns"]
            names = rd[0]
            src_i = next((i for i, c in enumerate(cols) if c["role"] == "source_row"), None)
            rt_i = next((i for i, c in enumerate(cols) if c["role"] == "row_type"), None)
            body = [[_cast(v) for v in r] for r in rd[1:]]
            src = [int(r[src_i]) if src_i is not None and r[src_i] is not None else -1 for r in body]
            rts = [str(r[rt_i]) for r in body] if rt_i is not None else None
            keep = [i for i, c in enumerate(cols) if c["role"] in ("data", "group")]
            label_cols = {k for k, i in enumerate(keep) if cols[i]["role"] == "group" or cols[i]["dtype"] in ("text", "string")}
            numeric = {k for k, i in enumerate(keep) if cols[i]["dtype"] in NUMERIC_HH}
            rows = [[r[i] if i < len(r) else None for i in keep] for r in body]
            tables.append(Table(t["id"], t["source"]["sheet"], [names[i] for i in keep], rows, src, rts, label_cols,
                                numeric, [cols[i].get("source_column") for i in keep]))
            tables[-1].title = t["source"].get("title") or ""
    return meta, raw, catalog, tables


@lru_cache(maxsize=64)
def pandas_load(dataset: str, rel: str):
    p = work_dir() / dataset / "pandas" / f"{key_of(rel)}.json.gz"
    if not p.exists():
        return None, []
    with gzip.open(p, "rt") as f:
        r = json.load(f)
    tables = []
    for s in r.get("sheets") or []:
        rows = s["rows"]
        ncol = len(s["columns"])
        numeric = {c for c, dt in enumerate(s.get("dtypes", [])) if dt.startswith(("int", "float"))}
        label = {c for c in range(ncol) if c not in numeric}
        tables.append(Table(s["name"], s["name"], s["columns"], rows, s.get("src_rows"), None, label, numeric,
                            [col_letter(c) for c in range(ncol)]))
    return r, tables


def md_load(dataset: str, rel: str):
    p = work_dir() / dataset / "markitdown" / f"{key_of(rel)}.json.gz"
    if not p.exists():
        return None
    with gzip.open(p, "rt") as f:
        return json.load(f)


# ------------------------------------------------------------------------------------------------
# metric C: reachability


def find_cells(tables: list[Table], atom: Atom):
    """Yield (table, row, col) where a table cell equals the atom."""
    for t in tables:
        for i, row in enumerate(t.rows):
            for c, v in enumerate(row):
                if atom.matches_value(v):
                    yield t, i, c


def in_header_or_title(tables: list[Table], a: Atom) -> bool:
    """An atom that names a column (a header or one ' / ' segment of it, e.g. a year) or a table title."""
    def same(x: str) -> bool:
        if a.num is not None:
            p = parse_number(x)
            return p is not None and num_eq(a.num, p[0])
        return norm_text(x) == a.norm

    for t in tables:
        if a.num is None and norm_text(t.title) == a.norm:
            return True
        for h in t.headers:
            if same(h) or any(same(s) for s in h.split(" / ")):
                return True
    return False


def reach_tables(tables: list[Table], atoms: list[Atom], qwords: set[str]) -> dict[str, bool]:
    """(ii) every atom a table cell (cell), with a row label or the column header sharing a question
    word (loose), with both sharing one (strict; for a text atom: the column header). An atom that
    is a column header or a table title counts at every level (narrative answers quote column names;
    "which year…" answers are year columns)."""
    res = {"cell": True, "loose": True, "strict": True}
    for a in atoms:
        if in_header_or_title(tables, a):
            continue
        best = {"cell": False, "loose": False, "strict": False}
        for t, i, c in find_cells(tables, a):
            best["cell"] = True
            hdr = words(t.headers[c]) & qwords
            lab = set().union(*[words(x) for x in t.labels(i, c)]) & qwords if t.labels(i, c) else set()
            if hdr or lab:
                best["loose"] = True
            if (hdr and lab) if a.num is not None else hdr:
                best["strict"] = True
                break
        for k in res:
            res[k] = res[k] and best[k]
    return res


def operands_by_question(tables: list[Table], qwords: set[str]) -> tuple[int, int]:
    """Heuristic operand columns: numeric-looking columns whose header shares a question word.
    Returns (matched columns, of those clean: numeric dtype and no total-labelled data row)."""
    n = ok = 0
    for t in tables:
        for c, h in enumerate(t.headers):
            if not (words(h) & qwords):
                continue
            vals = [r[c] for r in t.rows if c < len(r) and r[c] not in (None, "")]
            if not vals or sum(is_numeric_value(v) or parse_number(v) is not None for v in vals) < 0.5 * len(vals):
                continue
            n += 1
            numeric, unmixed = t.col_clean(c)
            ok += numeric and unmixed
    return n, ok


# ------------------------------------------------------------------------------------------------
# metric B: value-label linking (HiTab)


def locate(tables: list[Table], sheet: str, r0: int, c0: int):
    """(table, row index, col index) of source cell (r0, c0) on `sheet`, or a reason string."""
    letter = col_letter(c0)
    row_hit = False
    for t in tables:
        if t.sheet != sheet or (r0 + 1) not in t.by_src:
            continue
        row_hit = True
        if letter in t.letters:
            return t, t.by_src[r0 + 1], t.letters.index(letter)
    return "column not in the row's table" if row_hit else "row not in any table"


def seg_exact(gold: list[str], header: str) -> bool:
    segs = [norm_text(s) for s in header.split(" / ")]
    it = iter(segs)
    return all(any(g == s for s in it) for g in (norm_text(x) for x in gold))


def lenient_in(gold: list[str], text: str) -> bool:
    t = norm_text(text).replace(" ", "")
    return all(norm_text(g).replace(" ", "") in t for g in gold)


def link_one(tables: list[Table], it: dict, raw: dict) -> dict:
    cell = it["cells"][0]
    r0, c0 = cell["row"], cell["col"]
    loc = locate(tables, it["sheet"], r0, c0)
    out = {"found": not isinstance(loc, str)}
    if isinstance(loc, str):
        out["reason"] = loc
        return out
    t, i, c = loc
    v = t.rows[i][c] if c < len(t.rows[i]) else None
    rawv = raw.get((it["sheet"], r0, c0), cell.get("raw", ""))
    rp = parse_number(rawv)
    if rp is None:
        out["value"] = "source not a number"
    elif is_numeric_value(v) and abs(float(v) - rp[0]) <= 1e-9 * max(1.0, abs(rp[0])):
        out["value"] = "equal"
    elif is_numeric_value(v) and rp[2] and abs(float(v) - rp[0] / 100) <= 1e-9:
        out["value"] = "equal (percent as fraction)"
    elif isinstance(v, str):
        out["value"] = "kept as text"
    else:
        out["value"] = "different"
    out["stored"], out["source"] = v, rawv
    gold_top = [x for x in cell["top_path"] if x.strip()]
    gold_left = [x for x in cell["left_path"] if x.strip()]
    header = t.headers[c]
    labels = t.labels(i, c)
    out["header"], out["labels"] = header, labels
    out["col_exact"] = seg_exact(gold_top, header) if gold_top else None
    out["col_lenient"] = lenient_in(gold_top, header) if gold_top else None
    lab_norm = {norm_text(x) for x in labels}
    out["row_exact"] = all(norm_text(g) in lab_norm for g in gold_left) if gold_left else None
    out["row_lenient"] = lenient_in(gold_left, " ".join(labels)) if gold_left else None
    out["row_leaf"] = (norm_text(gold_left[-1]) in lab_norm) if gold_left else None
    return out


# ------------------------------------------------------------------------------------------------


def pct(a: int, b: int) -> str:
    return f"{100 * a / b:.1f} %" if b else "—"


def stats(xs: list[float]) -> dict:
    xs = sorted(x for x in xs if x is not None)
    if not xs:
        return {}
    q = lambda p: xs[min(len(xs) - 1, int(p * len(xs)))]
    return {"n": len(xs), "median": q(0.5), "p90": q(0.9), "max": xs[-1], "sum": sum(xs), "mean": statistics.fmean(xs)}


def err_class(e: str) -> str:
    m = re.match(r"([A-Za-z_][\w.]*(Error|Exception|Warning)|timeout|child died)", e or "")
    return m.group(1) if m else (e or "?")[:60]


def cost(dataset: str, files: list[str]) -> dict:
    """Metric D per tool."""
    hh, pdx, md = defaultdict(list), defaultdict(list), defaultdict(list)
    fails = {"hedgehog": Counter(), "pandas": Counter(), "markitdown": Counter()}
    ex = defaultdict(list)
    for f in files:
        meta, _, catalog, _ = hh_load(dataset, f)
        if meta is None:
            fails["hedgehog"]["not run"] += 1
            continue
        if meta.get("skipped"):
            fails["hedgehog"]["skipped > 200 MB"] += 1
        else:
            o = meta["open"]
            if o["rc"] != 0 or o.get("is_error") or o.get("timeout"):
                k = "timeout" if o.get("timeout") else "open_file error"
                fails["hedgehog"][k] += 1
                ex[f"hedgehog {k}"].append(f"{f}: {(catalog or o['stderr'])[:160]}")
            else:
                hh["chars"].append(len(catalog))
                hh["tokens"].append(count_tokens(catalog))
                hh["secs"].append(o["secs"])
                hh["rss_mb"].append((o["maxrss_kb"] or 0) / 1024)
                hh["normalize_secs"].append(meta["normalize"]["secs"])
        r, _ = pandas_load(dataset, f)
        if r is None:
            fails["pandas"]["not run"] += 1
        elif r.get("error"):
            k = err_class(r["error"])
            fails["pandas"][k] += 1
            ex[f"pandas {k}"].append(f"{f}: {r['error'][:160]}")
        else:
            pdx["secs"].append(r["secs"])
            pdx["rss_mb"].append(r["maxrss_kb"] / 1024)
        m = md_load(dataset, f)
        if m is None:
            fails["markitdown"]["not run"] += 1
        elif m.get("error"):
            k = err_class(m["error"])
            fails["markitdown"][k] += 1
            ex[f"markitdown {k}"].append(f"{f}: {m['error'][:160]}")
        else:
            md["chars"].append(m["chars"])
            md["tokens"].append(count_tokens(m["text"]))
            md["secs"].append(m["secs"])
            md["rss_mb"].append(m["maxrss_kb"] / 1024)
    return {"files": len(files),
            "hedgehog": {k: stats(v) for k, v in hh.items()}, "pandas": {k: stats(v) for k, v in pdx.items()},
            "markitdown": {k: stats(v) for k, v in md.items()},
            "failures": {k: dict(v) for k, v in fails.items()}, "failure_examples": {k: v[:3] for k, v in ex.items()}}


def reachability(dataset: str, items: list[dict], group_key) -> dict:
    """Metric C over items with atoms; grouped by `group_key(item)`."""
    res = defaultdict(lambda: Counter())
    examples = defaultdict(list)
    for it in items:
        files = it.get("files") or [it["file"]]
        g = group_key(it)
        c = res[g]
        c["items"] += 1
        if it.get("task") == "not_measured":
            c["not_measured"] += 1
            continue
        if it.get("missing"):
            c["missing_file"] += 1
            continue
        raw_atoms = it.get("atoms")
        if raw_atoms is None:  # HiTab: the answer list
            raw_atoms = [{"text": str(x), "kind": "number" if parse_number(x) else "text"} for x in it["answer"]]
        atoms = [Atom(a["text"], a["kind"]) for a in raw_atoms]
        atoms = [a for a in atoms if a.norm]
        loaded = [hh_load(dataset, f) for f in files]
        raw_vals = [v for (_, raw, _, _) in loaded for v in raw.values()]
        in_file = [a for a in atoms if any(a.matches_value(v) for v in raw_vals)]
        qwords = words(it["question"])
        hh_tables = [t for (_, _, _, ts) in loaded for t in ts]
        pd_tables = [t for f in files for t in pandas_load(dataset, f)[1]]
        computed = it.get("kind") == "computed" or not atoms or len(in_file) < len(atoms)
        if computed and it.get("kind") == "header_answer":  # HiTab rewrote the label: not a value question
            c["answer_not_in_file"] += 1
            continue
        if not computed:
            c["verbatim"] += 1
            catalog = "\n".join(cat for (_, _, cat, _) in loaded)
            cn, cnum = norm_text(catalog), text_numbers(catalog)
            mds = [md_load(dataset, f) for f in files]
            mtext = "\n".join((m or {}).get("text") or "" for m in mds)
            mn, mnum = norm_text(mtext), text_numbers(mtext)
            ok_cat = all(a.in_text(cn, cnum) for a in atoms)
            ok_md = all(a.in_text(mn, mnum) for a in atoms)
            c["hh_catalog"] += ok_cat
            c["md_text"] += ok_md
            for tool, tables in (("hh", hh_tables), ("pd", pd_tables)):
                r = reach_tables(tables, atoms, qwords)
                for k, v in r.items():
                    c[f"{tool}_{k}"] += v
                if tool == "hh" and not r["cell"] and len(examples[(g, "hedgehog: answer value in no table cell")]) < 6:
                    examples[(g, "hedgehog: answer value in no table cell")].append(
                        f"{files[0]} · item {it['id']} · answer atoms {[a.text[:40] for a in atoms][:4]}")
            if not ok_cat and len(examples[(g, "hedgehog: answer not in catalog")]) < 6:
                examples[(g, "hedgehog: answer not in catalog")].append(
                    f"{files[0]} · item {it['id']} · atoms {[a.text[:40] for a in atoms][:4]} · catalog {len(catalog)} chars")
        else:
            c["needs_query"] += 1
            if it.get("kind") == "computed" and it.get("cells"):  # HiTab: gold operand cells
                c["ops_known"] += 1
                for tool, tables in (("hh", hh_tables), ("pd", pd_tables)):
                    good = True
                    for cell in it["cells"]:
                        loc = locate(tables, it["sheet"], cell["row"], cell["col"])
                        if isinstance(loc, str):
                            good = False
                            break
                        t, _, col = loc
                        numeric, unmixed = t.col_clean(col)
                        good = good and numeric and unmixed
                    c[f"{tool}_ops_clean_known"] += good
            else:
                for tool, tables in (("hh", hh_tables), ("pd", pd_tables)):
                    n, ok = operands_by_question(tables, qwords)
                    c[f"{tool}_ops_matched"] += n > 0
                    c[f"{tool}_ops_clean"] += n > 0 and ok == n
    total = Counter()
    for c in res.values():
        total.update(c)
    out = {g: dict(c) for g, c in sorted(res.items())}
    out["ALL"] = dict(total)
    return {"groups": out, "examples": {f"{g} | {k}": v for (g, k), v in examples.items()}}


def linking(dataset: str, items: list[dict]) -> dict:
    """Metric B over HiTab lookups."""
    out = {}
    ex = defaultdict(list)
    lookups = [it for it in items if it["kind"] == "lookup" and it["cells"] and not it["cells"][0]["in_header_rows"]]
    for tool in ("hedgehog", "pandas"):
        c = Counter()
        for it in lookups:
            meta, raw, _, hh_tables = hh_load(dataset, it["file"])
            tables = hh_tables if tool == "hedgehog" else pandas_load(dataset, it["file"])[1]
            if tool == "pandas" and tables and all(t.src_rows is None for t in tables):
                c["pandas_rows_unmapped"] += 1
            r = link_one(tables, it, raw)
            c["n"] += 1
            if not r["found"]:
                c["not_found"] += 1
                c[f"not_found: {r['reason']}"] += 1
                key = f"{tool}: cell not in a table ({r['reason']})"
                if len(ex[key]) < 6:
                    ex[key].append(f"{it['file']} {it['sheet']}!{it['cells'][0]['a1']} = {it['cells'][0]['raw']!r}")
                continue
            c["found"] += 1
            c[f"value: {r['value']}"] += 1
            for k in ("col_exact", "col_lenient", "row_exact", "row_lenient", "row_leaf"):
                if r[k] is not None:
                    c[f"{k}_n"] += 1
                    c[k] += r[k]
            both = [r["col_exact"], r["row_exact"]]
            bothl = [r["col_lenient"], r["row_lenient"]]
            if None not in both:
                c["both_n"] += 1
                c["both_exact"] += all(both)
                c["both_lenient"] += all(bothl)
            cell = it["cells"][0]
            if r["col_exact"] is False and len(ex[f"{tool}: column header differs"]) < 6:
                ex[f"{tool}: column header differs"].append(
                    f"{it['file']} !{cell['a1']}: gold {cell['top_path']} vs {r['header']!r}")
            if r["row_exact"] is False and len(ex[f"{tool}: row labels differ"]) < 6:
                ex[f"{tool}: row labels differ"].append(
                    f"{it['file']} !{cell['a1']}: gold {cell['left_path']} vs {r['labels'][:4]}")
            if r["value"] not in ("equal", "equal (percent as fraction)", "source not a number") and len(ex[f"{tool}: value {r['value']}"]) < 6:
                ex[f"{tool}: value {r['value']}"].append(
                    f"{it['file']} !{cell['a1']}: source {r['source']!r} stored {r['stored']!r}")
        out[tool] = dict(c)
    return {"lookups": len(lookups), "tools": out, "examples": dict(ex)}


# ------------------------------------------------------------------------------------------------
# report


def md_reach(name: str, r: dict, label: str) -> list[str]:
    L = [f"#### C. Answer reachability by {label}", "",
         "| group | items | not measured | verbatim | hh catalog | MarkItDown text | hh cell | hh loose | hh strict | pandas cell | pandas loose | pandas strict |",
         "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for g, c in r["groups"].items():
        v = c.get("verbatim", 0)
        L.append(f"| {g} | {c.get('items', 0)} | {c.get('not_measured', 0) + c.get('missing_file', 0) + c.get('answer_not_in_file', 0)} | {v} | "
                 + " | ".join(pct(c.get(k, 0), v) for k in ("hh_catalog", "md_text", "hh_cell", "hh_loose", "hh_strict",
                                                            "pd_cell", "pd_loose", "pd_strict")) + " |")
    L += ["", "Not measured: the benchmark's free-text / chart / edit tasks, items whose file is missing, and HiTab header "
          "answers whose text is not a cell of the file (HiTab rewrote the label). Verbatim = every answer atom is a cell of "
          "the file; rates are over verbatim items.",
          "", "Needs a query (answer not verbatim in the file): are the operand columns clean numeric columns "
          "(numeric dtype, no total-labelled row counted as data)? With gold operand cells (HiTab) exactly; "
          "otherwise operand columns are guessed as numeric columns whose header shares a word with the question.", "",
          "| group | needs a query | gold operands | hh clean | pandas clean | guessed | hh: columns found | hh: all clean | pandas: columns found | pandas: all clean |",
          "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for g, c in r["groups"].items():
        n, k = c.get("needs_query", 0), c.get("ops_known", 0)
        h = n - k
        hk = lambda key: pct(c.get(key, 0), k) if k else "—"
        hh_ = lambda key: pct(c.get(key, 0), h) if h else "—"
        L.append(f"| {g} | {n} | {k} | {hk('hh_ops_clean_known')} | {hk('pd_ops_clean_known')} | {h} | "
                 f"{hh_('hh_ops_matched')} | {hh_('hh_ops_clean')} | {hh_('pd_ops_matched')} | {hh_('pd_ops_clean')} |")
    return L + [""]


def md_cost(c: dict) -> list[str]:
    f = lambda d, k, s: (f"{d[k][s]:,.0f}" if k in d and d[k] else "—")
    g = lambda d, k, s: (f"{d[k][s]:.3f}" if k in d and d[k] else "—")
    h, p, m = c["hedgehog"], c["pandas"], c["markitdown"]
    L = ["#### D. Cost per file", "",
         f"Tokens: {TOKENIZER}. Times are wall-clock per file, cold (hedgehog's disk cache off; pandas and MarkItDown "
         "time the read/convert call only, inside the sandbox, without interpreter start-up).", "",
         "| tool | files ok | chars median | chars p90 | tokens median | tokens p90 | tokens total | time median s | time p90 s | peak RSS median MB | peak RSS max MB | failures |",
         "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|"]
    for name, d in (("hedgehog open_file catalog", h), ("MarkItDown markdown", m), ("pandas read_excel", p)):
        fails = c["failures"]["hedgehog" if name.startswith("hedgehog") else "markitdown" if name.startswith("Mark") else "pandas"]
        n = (d.get("secs") or {}).get("n", 0)
        L.append(f"| {name} | {n} | {f(d, 'chars', 'median')} | {f(d, 'chars', 'p90')} | {f(d, 'tokens', 'median')} | "
                 f"{f(d, 'tokens', 'p90')} | {f(d, 'tokens', 'sum')} | {g(d, 'secs', 'median')} | {g(d, 'secs', 'p90')} | "
                 f"{f(d, 'rss_mb', 'median')} | {f(d, 'rss_mb', 'max')} | "
                 f"{', '.join(f'{k} {v}' for k, v in sorted(fails.items())) or '0'} |")
    return L + [""]


def md_link(b: dict) -> list[str]:
    L = ["#### B. Value-label linking (HiTab lookups: one answer cell, gold header paths)", "",
         f"{b['lookups']} lookup questions whose answer is one data cell.", "",
         "| tool | cell found in a table | number stored = source | column header exact | lenient | row labels exact | lenient | row leaf label | both exact | both lenient |",
         "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for tool, c in b["tools"].items():
        n = c.get("n", 0)
        eq = c.get("value: equal", 0) + c.get("value: equal (percent as fraction)", 0)
        num = c.get("found", 0) - c.get("value: source not a number", 0)
        L.append(f"| {tool} | {pct(c.get('found', 0), n)} | {pct(eq, num)} | {pct(c.get('col_exact', 0), c.get('col_exact_n', 0))} | "
                 f"{pct(c.get('col_lenient', 0), c.get('col_lenient_n', 0))} | {pct(c.get('row_exact', 0), c.get('row_exact_n', 0))} | "
                 f"{pct(c.get('row_lenient', 0), c.get('row_lenient_n', 0))} | {pct(c.get('row_leaf', 0), c.get('row_leaf_n', 0))} | "
                 f"{pct(c.get('both_exact', 0), c.get('both_n', 0))} | "
                 f"{pct(c.get('both_lenient', 0), c.get('both_n', 0))} |")
    L.append("")
    L.append("Header and label rates are over cells found; 'number stored = source' over found cells whose source "
             "reads as a number (stored as a number equal to it; text that looks like a number does not count). "
             "Not found: " + "; ".join(f"{tool}: " + (", ".join(f"{k[11:]} {v}" for k, v in c.items() if k.startswith("not_found: ")) or "0")
                                       for tool, c in b["tools"].items()) + ".")
    vals = {tool: {k[7:]: v for k, v in c.items() if k.startswith("value: ")} for tool, c in b["tools"].items()}
    L.append("Stored values: " + "; ".join(f"{tool}: {d}" for tool, d in vals.items()) + ".")
    return L + [""]


def examples_md(title: str, ex: dict, k: int) -> list[str]:
    if not ex:
        return []
    L = [f"<details><summary>{title}</summary>", ""]
    for key, v in sorted(ex.items()):
        L.append(f"- **{key}**")
        L += [f"  - `{x}`" for x in v[:k]]
    return L + ["", "</details>", ""]


def md_structure() -> list[str]:
    """Metric A from run_structure.py's TSVs (hedgehog benchmark --corpora public:…)."""
    rows = []
    for an in ("model", "layout", "baseline"):
        p = work_dir() / f"structure_{an}.tsv"
        if not p.exists():
            continue
        for r in csv.DictReader(p.open(), delimiter="\t"):
            rows.append((an, r))
    if not rows:
        return []
    f = lambda x: f"{100 * float(x):.1f}" if x not in ("", None) else "—"
    L = ["## A. Structure (`hedgehog benchmark --corpora public:hitab,public:tablesense`)", "",
         "Analyzers: `model` = rules + learned row model (what normalize and the MCP server use), `layout` = rules "
         "only, `baseline` = 8-connected flood fill. HiTab: coarse cell roles (title / header / data; group rows count as "
         "header) on the trimmed StatCan/NSF sheets, table = the HiTab matrix; TableSense: table ranges only. "
         "Table F1 = greedy one-to-one matching at IoU >= 0.5 / 0.8 / 1.0. Header rows exact = tables (matched at IoU >= 0.5) "
         "whose predicted header rows are exactly the gold header rows.", "",
         "| corpus · source | analyzer | files | sheets | cell role accuracy | table F1 IoU .5 | .8 | 1.0 | header rows exact |",
         "|---|---|---:|---:|---:|---:|---:|---:|---:|"]
    for an, r in sorted(rows, key=lambda x: (x[1]["corpus"], x[1]["source"] != "ALL", x[1]["source"], ("model", "layout", "baseline").index(x[0]))):
        acc = f(r["accuracy"]) if int(r["cells"] or 0) else "—"
        ht, he = int(r.get("header_tables") or 0), int(r.get("header_exact") or 0)
        L.append(f"| {r['corpus']} · {r['source']} | {an} | {r['files']} | {r['sheets']} | {acc} | {f(r['table_f1_iou50'])} | "
                 f"{f(r['table_f1_iou80'])} | {f(r['table_f1_iou100'])} | {pct(he, ht) + f' ({he}/{ht})' if ht else '—'} |")
    return L + [""]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--datasets", default=",".join(DATASETS))
    ap.add_argument("--examples", type=int, default=3)
    a = ap.parse_args()
    t0 = time.time()
    outdir = HERE / "results"
    outdir.mkdir(exist_ok=True)
    md = md_structure() + ["## B–D. Value linking, answer reachability, cost", ""]
    for name in a.datasets.split(","):
        root = data_dir() / name
        items = read_jsonl(root / "items.jsonl")
        if not items:
            print(f"{name}: no items (run the fetch and convert scripts)", file=sys.stderr)
            continue
        files = sorted({f for it in items for f in (it.get("files") or [it["file"]]) if (root / f).exists()})
        res: dict = {"dataset": name, "items": len(items), "files": len(files), "tokenizer": TOKENIZER}
        res["cost"] = cost(name, files)
        if name == "mimotable":
            res["reach_task"] = reachability(name, items, lambda it: f"{it['lang']} · {it['task']}")
            res["reach_type"] = reachability(name, items, lambda it: it["table_type"])
        elif name == "realhitbench":
            res["reach_task"] = reachability(name, items, lambda it: it["qtype"] + (" (long tables)" if it["long"] else ""))
            res["reach_type"] = reachability(name, items, lambda it: it["subtype"] if it["task"] != "not_measured" else "(not measured)")
        else:
            res["reach_task"] = reachability(name, items, lambda it: it["kind"])
            res["link"] = linking(name, items)
        (outdir / f"{name}.json").write_text(json.dumps(res, indent=1, ensure_ascii=False, default=str))
        md += [f"### {name}", "", f"{len(items):,} QA items over {len(files):,} spreadsheet files.", ""]
        if "link" in res:
            md += md_link(res["link"])
        md += md_reach(name, res["reach_task"], "the benchmark's task type")
        if "reach_type" in res:
            md += md_reach(name, res["reach_type"], "table type / sub-type")
        md += md_cost(res["cost"])
        ex = dict(res["reach_task"]["examples"])
        ex.update(res.get("link", {}).get("examples", {}))
        ex.update({f"open failure: {k}": v for k, v in res["cost"]["failure_examples"].items()})
        md += examples_md(f"Failure examples ({name})", ex, a.examples)
        print(f"{name}: scored in {time.time() - t0:.0f} s", file=sys.stderr)
    head = (HERE / "RESULTS.head.md").read_text() if (HERE / "RESULTS.head.md").exists() else "# Public benchmark results\n\n"
    (HERE / "RESULTS.md").write_text(head + "\n".join(md) + "\n")
    print(f"wrote {HERE / 'RESULTS.md'}", file=sys.stderr)


if __name__ == "__main__":
    main()
