"""Tier-0 part of the information scoreboard: small cases only, compared with a stored baseline.

    python3 -I eval/quick/info.py --bin BIN --data DATA --tmp DIR [--update]

Runs eval/info/score.py (model-free) on the cases in CASES and fails when a case answers fewer checks
from the catalog or one query (a+b), a trap check fails, or a catalog grows by more than 5 %.
Baseline: eval/quick/info.tsv (case, a+b, checks, catalog chars, traps passed).
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
# Small and medium files (the 4.5M-cell and multi-million-cell cases stay in the full scoreboard).
CASES = ["at-pension", "enron-henry-hub", "de-truck-toll", "enron-om-2plants", "enron-newco-cashflow",
         "enron-deals-pl", "enron-storage-inventory", "at-salzburg-budget", "ms-financial-sample",
         "nw-orders", "at-cpi", "wb-gdp-wide", "rs-pancevo-budget", "at-tourism-2months", "rs-geodetic-licenses"]


def score(binary: str, data: str, tmp: Path) -> dict[str, tuple[int, int, int, int, int]]:
    out, cache = tmp / "info-out", tmp / "info-cache"
    out.mkdir(parents=True, exist_ok=True)
    # Fresh sessions every run: a cached catalog would hide the change under test.
    if cache.is_dir():
        for f in cache.glob("*.json"):
            f.unlink()
    env = dict(os.environ, HEDGEHOG_BIN=binary, HEDGEHOG_DATA=data)
    cmd = [sys.executable, "-I", str(ROOT / "eval/info/score.py"), "--cases", ",".join(CASES),
           "--no-ablation", "--out", str(out), "--cache", str(cache)]
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
    return rows


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--data", required=True)
    ap.add_argument("--tmp", type=Path, required=True)
    ap.add_argument("--update", action="store_true")
    a = ap.parse_args()
    now = score(a.bin, a.data, a.tmp)
    if a.update or not BASE.exists():
        BASE.write_text("case\tab\tchecks\tchars\ttraps_ok\ttraps\n" + "".join(
            f"{k}\t{v[0]}\t{v[1]}\t{v[2]}\t{v[3]}\t{v[4]}\n" for k, v in sorted(now.items())))
        print(f"info: baseline written for {len(now)} cases")
        return
    base = {}
    for line in BASE.read_text().splitlines()[1:]:
        k, *v = line.split("\t")
        base[k] = tuple(int(x) for x in v)
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
    tot = sum(v[0] for v in now.values()), sum(v[1] for v in now.values()), sum(v[2] for v in now.values())
    if problems:
        print("info FAILED:\n  " + "\n  ".join(problems))
        sys.exit(1)
    print(f"info OK: {tot[0]}/{tot[1]} checks in the catalog or one query on {len(now)} cases, {tot[2]:,} catalog chars")


if __name__ == "__main__":
    main()
