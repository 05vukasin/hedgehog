"""Calibration: measure generated corpora with the same counting rules as the targets.

Two views:
* `gt_stats` — DECO-style shares from the ground truth (titles, multi-row headers, multi-table,
  totals, mid-table totals, notes, group headers), compared with DECO §0.
* `profile_stats` — the Enron profiler's own heuristics (`scripts/profile_corpus.py`, run on the
  generated xlsx files) compared with the Enron profile `docs/research/enron_profile/sheets.jsonl`.
"""
from __future__ import annotations

import importlib.util
import json
import random
from collections import defaultdict
from pathlib import Path

from .profiles import TARGETS

PROFILER = Path("/home/vukasin/projects/excell-lib-project/scripts/profile_corpus.py")
ENRON_PROFILE = Path("/home/vukasin/projects/excell-lib-project/docs/research/enron_profile/sheets.jsonl")


def sheet_flags(s: dict) -> dict | None:
    if not s["tables"]:
        return None
    roles = {c[2] for c in s["cells"]}
    mid = 0
    for t in s["tables"]:
        kinds = t.get("row_kinds", [])
        seen_total = False
        for k in kinds:
            if k == "total":
                seen_total = True
            elif k == "data" and seen_total:
                mid += 1
                break
    return {
        "sheet_title": "title" in roles,
        "sheet_multi_table": len(s["tables"]) > 1,
        "sheet_total": "total" in roles,
        "sheet_notes": "note" in roles,
        "sheet_group_header": "group_header" in roles,
        "tables": len(s["tables"]),
        "multirow": sum(1 for t in s["tables"] if t["header_rows"] >= 2),
        "mid": mid,
    }


def gt_stats(root: Path, profile: str | None = None, limit: int | None = None) -> dict:
    files = sorted(root.glob(f"{profile or '*'}/*.gt.json"))[: limit or None]
    agg = defaultdict(float)
    n_sheets = n_tables = 0
    for g in files:
        gt = json.loads(g.read_text())
        for s in gt["sheets"]:
            f = sheet_flags(s)
            if not f:
                continue
            n_sheets += 1
            n_tables += f["tables"]
            for k in ("sheet_title", "sheet_multi_table", "sheet_total", "sheet_notes", "sheet_group_header"):
                agg[k] += f[k]
            agg["table_multirow_header"] += f["multirow"]
            agg["table_midtable_total"] += f["mid"]
    out = {"workbooks": len(files), "sheets": n_sheets, "tables": n_tables}
    for k in TARGETS:
        den = n_tables if k.startswith("table_") else n_sheets
        out[k] = agg[k] / den if den else 0.0
    return out


def _profiler():
    spec = importlib.util.spec_from_file_location("profile_corpus", PROFILER)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


PROFILE_METRICS = [
    ("More than one table", lambda r: r["regions_ge4"] > 1),
    ("Stray single cells", lambda r: r["singleton_cells"] > 0),
    ("Merged cells", lambda r: r["merged_ranges"] > 0),
    ("Multi-row header (heuristic)", lambda r: r["multirow_header_regions"] > 0),
    ("Total/subtotal labels", lambda r: r["total_labels"] > 0),
    ("SUM formulas", lambda r: r["sum_formulas"] > 0),
    ("Numbers stored as text", lambda r: r["num_as_text"] > 0),
    ("Dates stored as text", lambda r: r["date_as_text"] > 0),
    ("Leading/trailing whitespace", lambda r: r["padded_text"] > 0),
    ("Note/source lines", lambda r: r["note_labels"] > 0),
    ("Hidden rows", lambda r: r["hidden_rows"] > 0),
    ("Hidden columns", lambda r: r["hidden_cols"] > 0),
]


def _shares(recs):
    data = [r for r in recs if "error" not in r and "skipped" not in r and not r.get("empty") and r.get("regions_ge4", 0) >= 1]
    n = len(data) or 1
    return len(data), {name: sum(1 for r in data if f(r)) / n for name, f in PROFILE_METRICS}


def profile_stats(root: Path, profile: str | None = None, sample: int = 300, seed: int = 0) -> dict:
    mod = _profiler()
    files = sorted(root.glob(f"{profile or '*'}/*.xlsx"))
    random.Random(seed).shuffle(files)
    recs = []
    for p in files[:sample]:
        recs.extend(mod.profile_file(p))
    n, gen = _shares(recs)
    enron = []
    if ENRON_PROFILE.exists():
        enron = [json.loads(line) for line in ENRON_PROFILE.read_text().splitlines() if line.strip()]
    ne, ref = _shares(enron)
    return {"generated_sheets": n, "enron_sheets": ne, "metrics": {k: {"enron": ref.get(k, 0.0), "generated": gen[k]} for k in gen}}


def report(root: Path, profiles: list[str], sample: int = 300) -> str:
    lines = ["## DECO-style shares from ground truth (target = DECO §0)", "",
             "| metric | target | " + " | ".join(profiles) + " |", "|---|---|" + "---|" * len(profiles)]
    res = {p: gt_stats(root, p) for p in profiles}
    for k, t in TARGETS.items():
        lines.append(f"| {k} | {t:.1%} | " + " | ".join(f"{res[p][k]:.1%}" for p in profiles) + " |")
    lines.append("| sheets / tables | | " + " | ".join(f"{res[p]['sheets']} / {res[p]['tables']}" for p in profiles) + " |")
    lines += ["", f"## Enron profiler heuristics (scripts/profile_corpus.py), {sample} workbooks per profile", ""]
    ps = {p: profile_stats(root, p, sample) for p in profiles}
    first = ps[profiles[0]]
    lines += ["| metric | Enron (n=%d) | " % first["enron_sheets"] + " | ".join(profiles) + " |", "|---|---|" + "---|" * len(profiles)]
    for k in first["metrics"]:
        lines.append(f"| {k} | {first['metrics'][k]['enron']:.1%} | " + " | ".join(f"{ps[p]['metrics'][k]['generated']:.1%}" for p in profiles) + " |")
    return "\n".join(lines) + "\n"
