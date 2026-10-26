"""Self-check for cases.json: schema, files, and reproducible reference answers.

    python3 -I eval/agents/check_cases.py                 # everything (run under scripts/run_capped.sh 4G …)
    python3 -I eval/agents/check_cases.py --schema-only   # no hedgehog calls: schema + files exist
    python3 -I eval/agents/check_cases.py --cases at-cpi,nw-orders

What it checks:
1. schema: every case has id, title, file, prompt, checks; ids are unique; `files` (if any) starts with
   `file`; every check has an id and exactly one kind (num, pct_range, text_any, judge) with the right types;
2. files: each file exists under $HEDGEHOG_DATA/<root> (root defaults to irregular) and opens with
   `hedgehog mcp` (open_file returns a catalog);
3. references: every `num` check (and every `pct_range` check that has one) carries a reference that is
   recomputed here and must match `num` within the check's `tol` (or lie inside `pct_range`); `abs`
   compares the absolute value. A reference is one of:
   - `ref_sql`: an SQL string run on the case's first file, or a list of {"file", "sql"} whose results
     are summed (multi-file cases). Each query returns one value (first column of the first row);
   - `ref_cells`: source cells ("Sheet!A1" or "'Sheet name'!A1"), summed, from `hedgehog cells` on the
     case file (or `ref_file`); for numbers hedgehog's catalog gets wrong, the cells are the ground truth;
   - `ref_py`: a Python expression over `rows` (csv.DictReader of the raw CSV) and `statistics`, for CSV
     values hedgehog misreads (so the reference does not repeat the bug).
   `ref_scale` multiplies the reference (e.g. 100 / total for a share).

Uses `hedgehog` from $HEDGEHOG_BIN or PATH, like sql.py. Each file is opened in its own server session
so memory stays bounded. Exit status 1 if anything fails.
"""

from __future__ import annotations

import argparse
import csv
import io
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import sql as S  # noqa: E402

KINDS = ("num", "pct_range", "text_any", "judge")
REFS = ("ref_sql", "ref_cells", "ref_py")


def data_root() -> Path:
    return Path(os.environ.get("HEDGEHOG_DATA") or HERE.parents[2] / "testdata")


def case_files(case: dict) -> list[str]:
    return case.get("files") or [case["file"]]


def schema(cases: list) -> list[str]:
    errs = []
    if not isinstance(cases, list):
        return ["cases.json is not a list"]
    seen = set()
    for i, c in enumerate(cases):
        where = f"case {i} ({c.get('id', '?')})"
        for k, t in (("id", str), ("title", str), ("file", str), ("prompt", str), ("checks", list)):
            if not isinstance(c.get(k), t):
                errs.append(f"{where}: missing or wrong type: {k}")
        if c.get("id") in seen:
            errs.append(f"{where}: duplicate id")
        seen.add(c.get("id"))
        if "files" in c and (not isinstance(c["files"], list) or len(c["files"]) < 2 or c["files"][0] != c.get("file")):
            errs.append(f"{where}: `files` must list ≥ 2 files, the first equal to `file`")
        if "root" in c and not isinstance(c["root"], str):
            errs.append(f"{where}: `root` must be a folder name under $HEDGEHOG_DATA")
        if not isinstance(c.get("backlog", []), list):
            errs.append(f"{where}: backlog must be a list")
        cids = set()
        for ch in c.get("checks") or []:
            w = f"{where} check {ch.get('id', '?')}"
            if not isinstance(ch.get("id"), str) or not isinstance(ch.get("desc"), str):
                errs.append(f"{w}: needs id and desc")
            if ch.get("id") in cids:
                errs.append(f"{w}: duplicate check id")
            cids.add(ch.get("id"))
            kinds = [k for k in KINDS if k in ch]
            if len(kinds) != 1:
                errs.append(f"{w}: needs exactly one of {KINDS}, has {kinds}")
                continue
            k = kinds[0]
            if k == "num" and not (isinstance(ch["num"], (int, float)) and isinstance(ch.get("tol", 0.005), (int, float))):
                errs.append(f"{w}: num/tol must be numbers")
            if k == "num" and not any(r in ch for r in REFS):
                errs.append(f"{w}: num check without a reference ({', '.join(REFS)})")
            if k == "pct_range" and not (isinstance(ch["pct_range"], list) and len(ch["pct_range"]) == 2 and ch["pct_range"][0] <= ch["pct_range"][1]):
                errs.append(f"{w}: pct_range must be [lo, hi]")
            if k == "text_any" and not (isinstance(ch["text_any"], list) and ch["text_any"] and all(isinstance(t, str) for t in ch["text_any"])):
                errs.append(f"{w}: text_any must be a non-empty list of strings")
            if k == "judge" and ch["judge"] is not True:
                errs.append(f"{w}: judge must be true")
            if "near" in ch and not (isinstance(ch["near"], list) and all(isinstance(t, str) for t in ch["near"])):
                errs.append(f"{w}: near must be a list of strings")
    return errs


def first_value(text: str) -> float:
    """The first cell of the first data row of a query result."""
    lines = text.splitlines()
    if len(lines) < 2 or text.startswith("SQL error") or lines[1].startswith("("):
        raise ValueError(text.strip()[:300])
    return float(next(csv.reader(io.StringIO(lines[1])))[0])


def hedgehog() -> str:
    return os.environ.get("HEDGEHOG_BIN") or shutil.which("hedgehog") or "hedgehog"


def cell_values(path: Path) -> dict[tuple[str, str], str]:
    out = subprocess.run([hedgehog(), "cells", str(path)], capture_output=True, text=True, check=True).stdout
    cells = {}
    for line in out.splitlines():
        p = line.split("\t", 4)
        if len(p) == 5:
            cells[(p[0], p[3])] = p[4]
    return cells


def raw_rows(path: Path) -> list[dict]:
    raw = path.read_bytes()
    for enc in ("utf-8-sig", "cp1252"):
        try:
            text = raw.decode(enc)
            break
        except UnicodeDecodeError:
            continue
    head = text.split("\n", 1)[0]
    delim = max(";,\t", key=head.count)
    return list(csv.DictReader(io.StringIO(text, newline=""), delimiter=delim))


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cases", default="")
    ap.add_argument("--schema-only", action="store_true")
    args = ap.parse_args()
    cases = json.loads((HERE / "cases.json").read_text())
    errs = schema(cases)
    want = set(filter(None, args.cases.split(",")))
    data = data_root()
    for c in cases:
        if want and c["id"] not in want:
            continue
        for f in case_files(c):
            if not (data / c.get("root", "irregular") / f).is_file():
                errs.append(f"{c['id']}: missing file {c.get('root', 'irregular')}/{f}")
    if args.schema_only or errs:
        report(errs, cases, want, 0)
        return
    n_refs = 0
    for c in cases:
        if want and c["id"] not in want:
            continue
        root = data / c.get("root", "irregular")
        # queries grouped by file: one server session per file
        per_file: dict[str, list[tuple[str, int, str]]] = {f: [] for f in case_files(c)}
        for ch in c["checks"]:
            refs = ch.get("ref_sql")
            if isinstance(refs, str):
                refs = [{"file": c["file"], "sql": refs}]
            for j, r in enumerate(refs or []):
                per_file.setdefault(r["file"], []).append((ch["id"], j, r["sql"]))
        results: dict[tuple[str, int], float | str] = {}
        for f, qs in per_file.items():
            texts = S.call(root, [("open_file", {"path": f})] + [("query", {"path": f, "sql": q, "limit": 5}) for _, _, q in qs])
            if not texts[0].startswith("FILE "):
                errs.append(f"{c['id']}: {f} does not open: {texts[0][:200]}")
                continue
            for (cid, j, _), t in zip(qs, texts[1:]):
                try:
                    results[(cid, j)] = first_value(t)
                except ValueError as e:
                    results[(cid, j)] = f"query failed: {e}"
        cells_cache: dict[str, dict] = {}
        for ch in c["checks"]:
            if not any(r in ch for r in REFS):
                continue
            try:
                if "ref_sql" in ch:
                    k = 1 if isinstance(ch["ref_sql"], str) else len(ch["ref_sql"])
                    vals = [results.get((ch["id"], j), "no result") for j in range(k)]
                    bad = [v for v in vals if isinstance(v, str)]
                    if bad:
                        raise ValueError(bad[0])
                    ref = sum(vals)
                elif "ref_cells" in ch:
                    f = ch.get("ref_file", c["file"])
                    if f not in cells_cache:
                        cells_cache[f] = cell_values(root / f)
                    ref = 0.0
                    for a in ch["ref_cells"]:
                        m = re.fullmatch(r"'?(.*?)'?!([A-Z]+[0-9]+)", a)
                        ref += float(cells_cache[f][(m.group(1), m.group(2))])
                else:
                    ref = eval(ch["ref_py"], {"__builtins__": {"sum": sum, "len": len, "float": float, "max": max, "min": min},  # noqa: S307
                                              "rows": raw_rows(root / ch.get("ref_file", c["file"])), "statistics": statistics})
                ref *= ch.get("ref_scale", 1)
            except Exception as e:  # noqa: BLE001
                errs.append(f"{c['id']}/{ch['id']}: reference failed: {e}")
                continue
            n_refs += 1
            v = abs(ref) if ch.get("abs") else ref
            if "num" in ch:
                tol = max(ch.get("tol", 0.005), 1e-9)
                if abs(v - ch["num"]) > tol * abs(ch["num"]) + 1e-9:
                    errs.append(f"{c['id']}/{ch['id']}: reference {ref!r} does not match num {ch['num']} (tol {tol})")
            elif "pct_range" in ch:
                lo, hi = ch["pct_range"]
                if not lo <= v <= hi:
                    errs.append(f"{c['id']}/{ch['id']}: reference {ref!r} outside pct_range [{lo}, {hi}]")
        print(f"{c['id']}: checked", flush=True)
    report(errs, cases, want, n_refs)


def report(errs: list[str], cases: list, want: set, n_refs: int) -> None:
    n = sum(1 for c in cases if not want or c["id"] in want)
    for e in errs:
        print("FAIL", e)
    print(f"{n} cases, {n_refs} references reproduced, {len(errs)} problems")
    sys.exit(1 if errs else 0)


if __name__ == "__main__":
    main()


# The answer checks' number reader (eval/info/score.py).
MULT = [(r"bn|b\b|billion|mil milhões|mrd\.?|milliard\w*", 1e9), (r"m\b|mn\b|million\w*|mio\.?|milhões", 1e6),
        (r"k\b|thousand|tsd\.?", 1e3)]
NUM = re.compile(r"(?<![\w.])[-−]?\d(?:[\d.,  ]*\d)?")


def numbers(text: str, pct_only: bool = False) -> list[float]:
    """Every reading of every number in the text (thousands/decimal separators are ambiguous)."""
    out = []
    for m in NUM.finditer(text):
        s = m.group().replace("−", "-").replace(" ", "").replace(" ", "")
        rest = text[m.end():m.end() + 16].lstrip().lower()
        if pct_only and not rest.startswith("%"):
            continue
        cands = set()
        if "," in s and "." in s:
            dec = "," if s.rfind(",") > s.rfind(".") else "."
            cands.add(float(s.replace("." if dec == "," else ",", "").replace(",", ".")))
        elif "," in s:
            if all(len(g) == 3 for g in s.split(",")[1:]):
                cands.add(float(s.replace(",", "")))
            if s.count(",") == 1:
                cands.add(float(s.replace(",", ".")))
        elif s.count(".") > 1:
            cands.add(float(s.replace(".", "")))
        else:
            cands.add(float(s))
            if "." in s and len(s.split(".")[1]) == 3:
                cands.add(float(s.replace(".", "")))
        mult = next((f for pat, f in MULT if re.match(pat, rest)), 1)
        out += [c * mult for c in cands] + ([c for c in cands] if mult != 1 else [])
    return out
