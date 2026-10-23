"""The model-free accuracy scoreboard (docs/ACCURACY_PLAN.md): F, S, L, C, M, K on the dev split.

    python -I eval/accuracy/scoreboard.py [--holdout] [--examples 3] [--update-k] [--no-structure]

No LLM anywhere. Inputs: the tool outputs of eval/public/run_hedgehog.py under $HH_PUBLIC_WORK (default
../scratch-archive/accuracy/work; `make score` refreshes them with the current build), the dev/holdout
file lists of split.py, and audit.tsv. Writes eval/accuracy/RESULTS.md and results.json (dev), or
results-holdout.json (holdout: aggregates only, never failure lists).

Measures
  F  cell fidelity: eval/fidelity/check.py summary when that gate exists on this branch, else skipped.
  S  structure (HiTab dev tables + TableSense): `hedgehog benchmark --corpora public:hitab,public:tablesense`
     on dev-only copies of the label files: cell-role accuracy, table F1 at IoU 0.5 / 0.8, header rows exact.
  L  lookup reachability (lookup.py), C computation reachability (oracle.py), M invariants
     (invariants.py; target 0 violations), K compactness: open_file catalog tokens per dev file against
     k_baseline.json (target: no regression > 1 %).
Misses are clustered by signature. A C or L miss on a file that has an M violation is clustered under that
violation ("... (file has: header missing)"): it names the likely library cause; otherwise the measure's own
cause. Audited misses (audit.tsv: gold_wrong | not_in_file | outside_knowledge, with evidence) leave the
denominator in the "after audit" score; raw scores count them as misses.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import statistics
import subprocess
import sys
import time
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import acc  # noqa: E402
import invariants  # noqa: E402
import lookup  # noqa: E402
import oracle  # noqa: E402
from common import TOKENIZER, count_tokens, data_dir, hedgehog_bin  # noqa: E402

HERE = Path(__file__).resolve().parent
WORK = Path(os.environ["HH_PUBLIC_WORK"])
DATASETS = acc.DATASETS
TARGETS = {"F": "100 %", "S": "100 % of fixable", "L": "100 %", "C": "100 %", "M": "0", "K": "no regression"}
# M kinds that name a library cause for a C / L miss on the same file, most specific first.
M_PRIORITY = ["M4 date day changed", "M4 Excel serial as number", "M6 '(n)' read as negative", "M5 number as header",
              "M5 repeated header names", "M5 duplicate headers", "M5 header missing", "M1 total row in data",
              "M2 percent scale", "M2 percent mixed", "M4 dates collapsed", "M4 date out of range", "M3 measures mixed"]
# The 9 MiMoTable agent failures classified as library causes (AGENT_RESULTS.md), checked by hand again
# for this scoreboard (eval/accuracy/README.md has the evidence). Each must be caught by L, C or M.
LIBRARY_CASES = {
    "en-0118": "'March 25' read as 2025-03-01",
    "en-0119": "'March 26' read as 2026-03-01",
    "en-0519": "header row missed after a key/value line: columns col_B...",
    "en-0516": "header row missed after a key/value line: columns col_B...",
    "en-0508": "sub-header 'This Year' not grouped under its section (Main Income)",
    "en-0496": "Excel date serial (44310) left as a number",
    "en-0549": "a data row ('Repayment Judgment ... Repayment OK') taken as the header, table split",
    "en-0371": "a section row ('One / Land leveling') merged into the header; '(4)' read as -4",
    "en-0048": "first data row taken as the header and the table split in two",
}


# ------------------------------------------------------------------------------------------------


def load_audit() -> dict:
    p = HERE / "audit.tsv"
    out = {}
    if p.exists():
        with open(p, newline="") as f:
            for r in csv.DictReader((x for x in f if not x.startswith("#")), delimiter="\t"):
                out[(r["dataset"], r["item_id"], r["measure"])] = r
    return out


def run_fidelity() -> dict:
    chk = acc.REPO / "eval" / "fidelity" / "check.py"
    if not chk.exists():
        return {"skipped": "eval/fidelity is not on this branch (cell-fidelity gate: branch fidelity)"}
    py = os.environ.get("FIDELITY_PY") or str(acc.PROJECT / "scratch-archive" / "fidelity" / "venv" / "bin" / "python")
    out = WORK / "fidelity_summary.md"
    try:
        subprocess.run([py, "-I", str(chk), "summary", "--out", str(out)], check=True, capture_output=True, text=True, timeout=900)
        s = json.loads(out.with_suffix(".json").read_text())
    except Exception as e:  # noqa: BLE001
        return {"skipped": f"fidelity summary failed: {str(e)[:200]}"}
    tot = Counter()
    for corp in (s.get("corpora") or {}).values() if isinstance(s.get("corpora"), dict) else []:
        for k, v in (corp.get("classes") or {}).items():
            tot[k] += v
    cells = sum(tot.values())
    return {"summary": s, "ok": tot.get("ok", 0), "cells": cells, "classes": dict(tot)} if cells else {"summary": s}


def structure_data_root() -> Path:
    """Dev-only copies of the HiTab label files (absolute file paths) + TableSense."""
    root = WORK / "structure-dev"
    dev = acc.side_files("hitab", "dev")
    for name, keep in (("hitab", lambda f: f in dev), ("tablesense", lambda f: True)):
        src, dst = data_dir() / name, root / "public" / name
        dst.mkdir(parents=True, exist_ok=True)
        for fn in ("labels.tsv", "tables.tsv"):
            if not (src / fn).exists():
                continue
            lines = (src / fn).read_text().splitlines()
            out = [lines[0]]
            for ln in lines[1:]:
                p = ln.split("\t")
                if len(p) > 1 and keep(p[1]):
                    p[1] = str((src / p[1]).resolve())
                    out.append("\t".join(p))
            (dst / fn).write_text("\n".join(out) + "\n")
    return root


def run_structure(binary: str) -> dict:
    root = structure_data_root()
    out = WORK / "structure_dev_model.tsv"
    r = subprocess.run([binary, "benchmark", "--corpora", "public:hitab,public:tablesense", "--analyzer", "model",
                        "--threads", "2", "--out", str(out)], cwd=acc.REPO, capture_output=True, text=True,
                       env={"HEDGEHOG_DATA": str(root), "PATH": "/usr/bin:/bin"}, timeout=3600)
    if r.returncode != 0:
        return {"skipped": f"benchmark failed: {r.stderr[-300:]}"}
    rows = list(csv.DictReader(out.read_text().splitlines(), delimiter="\t"))
    res = {}
    for x in rows:
        if x["source"] != "ALL":
            continue
        c = x["corpus"].split(":")[-1]
        f = lambda k: float(x[k]) if x.get(k) not in (None, "") else None
        res[c] = {"files": int(x["files"]), "cell_role_acc": f("coarse_pooled") if c == "hitab" else None,
                  "table_f1_iou50": f("table_f1_iou50"), "table_f1_iou80": f("table_f1_iou80"),
                  "header_exact": (int(x["header_exact"]), int(x["header_tables"])) if c == "hitab" else None}
    return res


def k_measure(side: str) -> dict:
    per = {}
    for ds in DATASETS:
        toks = []
        for f in sorted(acc.side_files(ds, side)):
            p = WORK / ds / "hh" / acc.PS.key_of(f) / "catalog.txt"
            if p.exists():
                toks.append(count_tokens(p.read_text()))
        if toks:
            toks.sort()
            per[ds] = {"files": len(toks), "mean": round(statistics.fmean(toks), 1), "median": toks[len(toks) // 2],
                       "p90": toks[int(0.9 * len(toks))], "max": toks[-1]}
    return {"tokenizer": TOKENIZER, "per_dataset": per}


# ------------------------------------------------------------------------------------------------


def m_index(mrows: list[dict]) -> dict:
    by = defaultdict(set)
    for r in mrows:
        by[(r["dataset"], r["file"])].add(r["inv"])
    return by


GENERIC = ("outside program space", "only loosely grounded", "only ungrounded", "value in a cell, no grounded")


def cluster_of(ds: str, files: list[str], own: str, mind: dict) -> str:
    """The miss's own cause; a generic C cause gets the file's most specific M violation attached."""
    if not own.startswith(GENERIC):
        return own
    kinds = set().union(*[mind.get((ds, f), set()) for f in files]) if files else set()
    for k in M_PRIORITY:
        if k in kinds:
            return f"{own} (file has: {k[3:]})"
    return own


def score(side: str, examples: int, structure: bool, binary: str) -> dict:
    t0 = time.time()
    audit = load_audit()
    items = {(ds, it["id"]): it for ds in DATASETS for it in acc.load_items(ds, side)}
    res = {"side": side, "binary": binary, "tokenizer": TOKENIZER}
    res["F"] = run_fidelity()
    res["S"] = run_structure(binary) if structure else {"skipped": "--no-structure"}
    print(f"F, S done {time.time() - t0:.0f} s", file=sys.stderr, flush=True)
    mrows = invariants.run(DATASETS, side)
    mind = m_index(mrows)
    print(f"M done {time.time() - t0:.0f} s", file=sys.stderr, flush=True)
    lrows = lookup.run(DATASETS, side)
    print(f"L done {time.time() - t0:.0f} s", file=sys.stderr, flush=True)
    crows = oracle.run(DATASETS, side)
    print(f"C done {time.time() - t0:.0f} s", file=sys.stderr, flush=True)

    def measure(name: str, rows: list[dict], ok_key: str):
        per = {}
        clusters = defaultdict(list)
        audited = []
        for ds in DATASETS:
            rr = [r for r in rows if r["dataset"] == ds]
            n = len(rr)
            ok = sum(bool(r[ok_key]) for r in rr)
            aud = [r for r in rr if not r[ok_key] and (ds, r["id"], name) in audit]
            per[ds] = {"items": n, "pass": ok, "audited": len(aud),
                       "raw": round(100 * ok / n, 1) if n else None,
                       "after_audit": round(100 * ok / (n - len(aud)), 1) if n - len(aud) else None}
            for r in rr:
                if r[ok_key]:
                    continue
                if (ds, r["id"], name) in audit:
                    audited.append((ds, r["id"], audit[(ds, r["id"], name)]["reason"]))
                    continue
                bad = [a for a in r["atoms"] if (a.get("attempted", True) and not a.get("pass", a.get("reachable")))]
                own = bad[0].get("cause", "?") if bad else "?"
                it = items.get((ds, r["id"]), {})
                cl = cluster_of(ds, it.get("files", []), own, mind)
                ev = bad[0].get("evidence") or "; ".join(bad[0].get("witnesses", [])[:1]) or bad[0].get("weak_example", "") if bad else ""
                clusters[cl].append({"dataset": ds, "id": r["id"], "file": (it.get("files") or ["?"])[0],
                                     "atom": bad[0].get("text", "") if bad else "", "evidence": ev})
        tot_n = sum(p["items"] for p in per.values())
        tot_ok = sum(p["pass"] for p in per.values())
        tot_a = sum(p["audited"] for p in per.values())
        per["ALL"] = {"items": tot_n, "pass": tot_ok, "audited": tot_a, "raw": round(100 * tot_ok / tot_n, 1) if tot_n else None,
                      "after_audit": round(100 * tot_ok / (tot_n - tot_a), 1) if tot_n - tot_a else None}
        return {"per": per, "clusters": {k: v for k, v in sorted(clusters.items(), key=lambda kv: -len(kv[1]))},
                "audited": audited}

    res["L"] = measure("L", lrows, "pass")
    res["C"] = measure("C", crows, "reachable")
    res["C"]["oracle"] = {ds: oracle.summarize([r for r in crows if r["dataset"] == ds]) for ds in DATASETS}
    res["C"]["validation"] = oracle.validate(crows, False)
    res["C"]["decoys_by_magnitude"] = decoy_buckets(crows, items)
    mk = Counter(r["inv"] for r in mrows)
    res["M"] = {"violations": len(mrows), "files": len({(r["dataset"], r["file"]) for r in mrows}),
                "per_dataset": {ds: dict(Counter(r["inv"] for r in mrows if r["dataset"] == ds)) for ds in DATASETS},
                "kinds": dict(mk.most_common()),
                "examples": {k: [f"{r['dataset']} {r['file']} {r['table']} {r['cell'] or ''} | {r['evidence']}"
                                 for r in mrows if r["inv"] == k][:examples] for k in mk}}
    res["K"] = k_measure(side)
    kb = HERE / "k_baseline.json"
    if kb.exists():
        base = json.loads(kb.read_text())
        res["K"]["baseline"] = base
        res["K"]["regressions"] = [ds for ds, v in res["K"]["per_dataset"].items()
                                   if ds in base.get("per_dataset", {}) and v["mean"] > 1.01 * base["per_dataset"][ds]["mean"]]
    if side == "dev":
        res["library_cases"] = library_cases(lrows, crows, mind, items)
    res["seconds"] = round(time.time() - t0)
    res["_crows"] = crows
    return res


def decoy_buckets(crows, items) -> dict:
    b = defaultdict(Counter)
    for r in crows:
        for a in r["atoms"]:
            p = acc.parse_number(a["text"])
            v = abs(p[0]) if p else 0
            k = "integer <= 10" if v == int(v) and v <= 10 else "integer 11-100" if v == int(v) and v <= 100 else \
                "integer > 100" if v == int(v) else "decimal"
            b[k]["atoms"] += 1
            b[k]["reachable"] += a["reachable"]
            b[k]["decoys"] += a["decoys"][0]
            b[k]["decoys_reachable"] += a["decoys"][1]
    return {k: dict(v) for k, v in b.items()}


def library_cases(lrows, crows, mind, items) -> list[dict]:
    out = []
    lb = {r["id"]: r for r in lrows if r["dataset"] == "mimotable"}
    cb = {r["id"]: r for r in crows if r["dataset"] == "mimotable"}
    for i, why in LIBRARY_CASES.items():
        it = items.get(("mimotable", i))
        if it is None:
            out.append({"id": i, "why": why, "caught": None, "by": "holdout item (checked with --holdout)"})
            continue
        by = []
        if i in lb and not lb[i]["pass"]:
            by.append("L: " + "; ".join(a.get("cause", "") for a in lb[i]["atoms"] if a.get("attempted") and not a["pass"]))
        if i in cb and not cb[i]["reachable"]:
            by.append("C: " + "; ".join(a.get("cause", "") for a in cb[i]["atoms"] if not a["reachable"]))
        kinds = sorted(set().union(*[mind.get(("mimotable", f), set()) for f in it["files"]]))
        if kinds:
            by.append("M: " + ", ".join(kinds))
        out.append({"id": i, "why": why, "caught": bool(by), "by": " | ".join(by)})
    return out


# ------------------------------------------------------------------------------------------------


def md(res: dict, examples: int) -> str:
    L = ["# Accuracy scoreboard (model-free)", "",
         f"Split: **{res['side']}**. Binary: `{res['binary']}`. Generated by `make score` "
         f"(eval/accuracy/scoreboard.py, {res['seconds']} s). No LLM anywhere. Definitions: "
         "[README.md](README.md); plan: [docs/ACCURACY_PLAN.md](../../docs/ACCURACY_PLAN.md).", "",
         "| Measure | Value (raw) | After audit | Target | Misses |", "|---|---|---|---|---|"]
    F = res["F"]
    L.append(f"| F fidelity | {'skipped: ' + F['skipped'] if 'skipped' in F else (str(round(100 * F['ok'] / F['cells'], 2)) + ' %' if F.get('cells') else 'see eval/fidelity')} | | {TARGETS['F']} | |")
    S = res["S"]
    if "skipped" in S:
        L.append(f"| S structure | skipped: {S['skipped']} | | {TARGETS['S']} | |")
    else:
        h, ts = S.get("hitab", {}), S.get("tablesense", {})
        hx = h.get("header_exact") or (0, 0)
        L.append(f"| S structure (HiTab dev, {h.get('files')} tables) | cell roles {100 * (h.get('cell_role_acc') or 0):.1f} %, "
                 f"table F1@0.8 {100 * (h.get('table_f1_iou80') or 0):.1f} %, header rows exact {hx[0]}/{hx[1]} | | {TARGETS['S']} | |")
        L.append(f"| S structure (TableSense, {ts.get('files')} files) | table F1@0.5 {100 * (ts.get('table_f1_iou50') or 0):.1f} %, "
                 f"F1@0.8 {100 * (ts.get('table_f1_iou80') or 0):.1f} % | | {TARGETS['S']} | |")
    for m, label in (("L", "L lookup"), ("C", "C computation")):
        a = res[m]["per"]["ALL"]
        L.append(f"| {label} | {a['pass']}/{a['items']} = {a['raw']} % | {a['after_audit']} % ({a['audited']} audited) | {TARGETS[m]} | {a['items'] - a['pass'] - a['audited']} |")
    L.append(f"| M misleading | {res['M']['violations']} violations in {res['M']['files']} files | | {TARGETS['M']} | {res['M']['violations']} |")
    K = res["K"]
    kk = ", ".join(f"{ds} {v['mean']:.0f}" for ds, v in K["per_dataset"].items())
    L.append(f"| K compact (mean catalog tokens/file, {K['tokenizer']}) | {kk} | | {TARGETS['K']} | "
             f"{', '.join(K.get('regressions', [])) or ('none' if 'baseline' in K else 'no baseline yet')} |")
    L += ["", "## Per dataset", "", "| Measure | Dataset | Items | Pass | Raw | Audited | After audit |", "|---|---|---|---|---|---|---|"]
    for m in ("L", "C"):
        for ds, p in res[m]["per"].items():
            L.append(f"| {m} | {ds} | {p['items']} | {p['pass']} | {p['raw']} % | {p['audited']} | {p['after_audit']} % |")
    L += ["", "| M | " + " | ".join(DATASETS) + " |", "|---|" + "---|" * len(DATASETS)]
    kinds = sorted(res["M"]["kinds"], key=lambda k: -res["M"]["kinds"][k])
    for k in kinds:
        L.append(f"| {k} | " + " | ".join(str(res["M"]["per_dataset"][ds].get(k, 0)) for ds in DATASETS) + " |")
    if res["side"] == "dev":
        for m in ("L", "C"):
            L += ["", f"## {m} misses by cause cluster", "", "| Cluster | Misses | Example |", "|---|---|---|"]
            for k, v in list(res[m]["clusters"].items())[:20]:
                ex = v[0]
                L.append(f"| {k} | {len(v)} | {ex['dataset']} {ex['id']} · {Path(ex['file']).name} · `{str(ex['atom'])[:30]}` · {str(ex['evidence'])[:140].replace('|', '/')} |")
        L += ["", "## M violations: examples", ""]
        for k in kinds:
            for e in res["M"]["examples"][k][:examples]:
                L.append(f"- **{k}**: {e[:220]}")
        L += ["", "## The 9 MiMoTable library failures of the agent run", "", "| Item | Library cause (agent run) | Caught | By |", "|---|---|---|---|"]
        for x in res.get("library_cases", []):
            L.append(f"| {x['id']} | {x['why']} | {'yes' if x['caught'] else 'holdout' if x['caught'] is None else 'NO'} | {x['by'][:220]} |")
    v = res["C"]["validation"]
    L += ["", "## C oracle: validation", "",
          f"- HiTab computed atoms whose gold the gold operand cells reproduce with a formula of the program space: "
          f"{v.get('replayable', 0)}; the oracle reaches {v.get('replayable_reachable', 0)} of them; a tight witness uses only "
          f"gold operand cells in {v.get('replayable_agree_subset', 0)} (exactly all of them: {v.get('replayable_agree', 0)}).",
          f"- Single-answer items: {v.get('single_answer_replayable', 0)} replayable, {v.get('single_answer_reachable', 0)} reached, "
          f"{v.get('single_answer_agree_exact', 0)} with exactly the gold operands.",
          "- Decoy test (the gold x 1.1 and x 0.9 at the same precision; a reachable decoy is a coincidence the guard let through):", "",
          "| Gold magnitude | Atoms | Reachable | Decoys | Decoys reachable |", "|---|---|---|---|---|"]
    for k, b in sorted(res["C"]["decoys_by_magnitude"].items()):
        L.append(f"| {k} | {b['atoms']} | {b['reachable']} ({100 * b['reachable'] / b['atoms']:.0f} %) | {b['decoys']} | "
                 f"{b['decoys_reachable']} ({100 * b['decoys_reachable'] / max(1, b['decoys']):.0f} %) |")
    L += ["", "## Audited misses", ""]
    for m in ("L", "C"):
        for ds, i, why in res[m]["audited"]:
            L.append(f"- {m} {ds} {i}: {why} (evidence in audit.tsv)")
    return "\n".join(L) + "\n"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--holdout", action="store_true", help="score the frozen holdout (aggregates only)")
    ap.add_argument("--examples", type=int, default=3)
    ap.add_argument("--update-k", action="store_true", help="write k_baseline.json from this run")
    ap.add_argument("--no-structure", action="store_true")
    ap.add_argument("--bin", default=hedgehog_bin())
    a = ap.parse_args()
    side = "holdout" if a.holdout else "dev"
    res = score(side, a.examples, not a.no_structure, a.bin)
    crows = res.pop("_crows")
    if a.update_k and side == "dev":
        (HERE / "k_baseline.json").write_text(json.dumps({"tokenizer": res["K"]["tokenizer"], "per_dataset": res["K"]["per_dataset"]}, indent=1) + "\n")
    if side == "dev":
        (HERE / "RESULTS.md").write_text(md(res, a.examples))
        (WORK / "results-full.json").write_text(json.dumps(res, indent=1, ensure_ascii=False, default=str))
        slim = json.loads(json.dumps(res, default=str))  # committed: counts and a few examples per cluster
        for m in ("L", "C"):
            slim[m]["clusters"] = {k: {"misses": len(v), "examples": v[:a.examples]} for k, v in res[m]["clusters"].items()}
        (HERE / "results.json").write_text(json.dumps(slim, indent=1, ensure_ascii=False, default=str))
        (WORK / "oracle-dev.jsonl").write_text("".join(json.dumps(r, ensure_ascii=False, default=str) + "\n" for r in crows))
        print(md(res, a.examples).split("## Per dataset")[0])
    else:
        slim = {k: v for k, v in res.items() if k in ("side", "binary", "F", "S", "K", "seconds")}
        slim["L"] = res["L"]["per"]
        slim["C"] = res["C"]["per"]
        slim["M"] = {"violations": res["M"]["violations"], "kinds": res["M"]["kinds"]}
        (HERE / "results-holdout.json").write_text(json.dumps(slim, indent=1, ensure_ascii=False, default=str))
        print(json.dumps(slim, indent=1, default=str))


if __name__ == "__main__":
    main()
