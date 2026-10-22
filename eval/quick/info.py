"""Tier-0 part of the information scoreboard: small cases only, compared with a stored baseline.

    python3 -I eval/quick/info.py --bin BIN --data DATA --tmp DIR [--update]

Runs eval/info/score.py (model-free) on the cases in CASES and fails when a case answers fewer checks
from the catalog or one query (a+b), a trap check fails, or a catalog grows by more than 5 %.
Baseline: eval/quick/info.tsv (case, a+b, checks, catalog chars, traps passed).

Second line (B150): the same with each file opened with the case's prompt as `open_file`'s question
(score.py --question), against eval/quick/info_question.tsv, with the catalog size change against the
catalogs without the question.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
BASE = HERE / "info.tsv"
QBASE = HERE / "info_question.tsv"
# Small and medium files (the 4.5M-cell and multi-million-cell cases stay in the full scoreboard).
CASES = ["at-pension", "enron-henry-hub", "de-truck-toll", "enron-om-2plants", "enron-newco-cashflow",
         "enron-deals-pl", "enron-storage-inventory", "at-salzburg-budget", "ms-financial-sample",
         "nw-orders", "at-cpi", "wb-gdp-wide", "rs-pancevo-budget", "at-tourism-2months", "rs-geodetic-licenses"]


Rows = dict[str, tuple[int, int, int, int, int]]


def score(binary: str, data: str, tmp: Path) -> tuple[Rows, Rows]:
    out, cache = tmp / "info-out", tmp / "info-cache"
    out.mkdir(parents=True, exist_ok=True)
    # Fresh sessions every run: a cached catalog would hide the change under test.
    if cache.is_dir():
        for f in cache.glob("*.json"):
            f.unlink()
    env = dict(os.environ, HEDGEHOG_BIN=binary, HEDGEHOG_DATA=data)
    cmd = [sys.executable, "-I", str(ROOT / "eval/info/score.py"), "--cases", ",".join(CASES),
           "--no-ablation", "--question", "--out", str(out), "--cache", str(cache)]
    p = subprocess.run(cmd, env=env, capture_output=True, text=True)
    if p.returncode != 0:
        sys.exit(f"info: score.py failed\n{p.stderr[-2000:]}")
    res = json.loads((out / "results.json").read_text())
    rows = {}
    for c in res["cases"]:
        ab = sum(1 for k in c["checks"] if k["cat"] in ("a", "b"))
        traps = c.get("trap_checks") or []
        ok = sum(1 for t in traps if t.get("ok", t.get("pass", False)))
        rows[c["case"]] = (ab, len(c["checks"]), c["chars"], ok, len(traps))
    qres = json.loads((out / "results-question.json").read_text())
    qrows = {}
    for c in qres["cases"]:
        traps = c.get("trap_checks") or []
        ok = sum(1 for t in traps if t.get("ok", False))
        qrows[c["case"]] = (c["ab_with"], c["checks"], c["chars_with"], ok, len(traps))
    return rows, qrows


def read_base(path: Path) -> Rows:
    base = {}
    for line in path.read_text().splitlines()[1:]:
        k, *v = line.split("\t")
        base[k] = tuple(int(x) for x in v)
    return base


def write_base(path: Path, rows: Rows) -> None:
    path.write_text("case\tab\tchecks\tchars\ttraps_ok\ttraps\n" + "".join(
        f"{k}\t{v[0]}\t{v[1]}\t{v[2]}\t{v[3]}\t{v[4]}\n" for k, v in sorted(rows.items())))


def problems_of(now: Rows, base: Rows) -> list[str]:
    problems = []
    for k, (ab, n, chars, tok, tn) in sorted(now.items()):
        if k not in base:
            continue
        bab, bn, bchars, btok, btn = base[k]
        if ab < bab:
            problems.append(f"{k}: {ab}/{n} checks in the catalog or one query (was {bab}/{bn})")
        if tok < btok:
            problems.append(f"{k}: {tok}/{tn} trap checks pass (was {btok}/{btn})")
        if chars > bchars * 1.05:
            problems.append(f"{k}: catalog {chars} chars (was {bchars}, +{100 * (chars - bchars) / bchars:.0f} %)")
    return problems


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--data", required=True)
    ap.add_argument("--tmp", type=Path, required=True)
    ap.add_argument("--update", action="store_true")
    a = ap.parse_args()
    now, qnow = score(a.bin, a.data, a.tmp)
    if a.update or not BASE.exists():
        write_base(BASE, now)
        write_base(QBASE, qnow)
        print(f"info: baseline written for {len(now)} cases (without and with the question)")
        return
    if not QBASE.exists():
        write_base(QBASE, qnow)
        print(f"info question: baseline written for {len(qnow)} cases")
    failed = False
    for name, rows, base in (("info", now, read_base(BASE)), ("info question", qnow, read_base(QBASE))):
        problems = problems_of(rows, base)
        tot = sum(v[0] for v in rows.values()), sum(v[1] for v in rows.values()), sum(v[2] for v in rows.values())
        if problems:
            print(f"{name} FAILED:\n  " + "\n  ".join(problems))
            failed = True
            continue
        if rows is qnow:
            plain = sum(v[2] for v in now.values())
            change = f" ({100 * (tot[2] - plain) / plain:+.0f} % vs without)" if plain else ""
            print(f"info question OK: {tot[0]}/{tot[1]} checks with the prompt as open_file's question, "
                  f"{tot[2]:,} catalog chars{change}")
        else:
            print(f"info OK: {tot[0]}/{tot[1]} checks in the catalog or one query on {len(rows)} cases, {tot[2]:,} catalog chars")
    if failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
