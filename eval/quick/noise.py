"""Tier 0 noise smoke: one pass/fail line per noise family from a `noise_eval --batch-dir` run.

    noise_eval --seeds 1 --intensities 0.5 --batch-dir DIR ...
    python3 eval/quick/noise.py DIR [--update]

Per family and path (rules, model): `ok`, `FAIL(<invariants>)` or `CRASH`. Compared with
noise.tsv: fails on any crash, or on a family/path that passed there and fails now. A family that
now passes where it failed is reported as improved (accept it with --update).
"""

from __future__ import annotations

import argparse
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
PATHS = ("rules", "model")


def summarize(batch: Path) -> dict[str, list[str]]:
    fails: dict[tuple[str, int], set[str]] = defaultdict(set)
    crash: set[tuple[str, int]] = set()
    families: list[str] = []
    for tsv in sorted(batch.glob("*.tsv")):
        for line in tsv.read_text().splitlines():
            c = line.split("\t")
            if c[0] == "S" and c[1] not in families:
                families.append(c[1])
            elif c[0] == "E":
                crash.add((c[1], int(c[4])))
            elif c[0] == "I" and int(c[7]) > 0:
                fails[(c[1], int(c[4]))].add(c[5])
    out = {}
    for f in families:
        out[f] = ["CRASH" if (f, p) in crash else f"FAIL({','.join(sorted(fails[(f, p)]))})" if fails[(f, p)] else "ok" for p in range(2)]
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("batch", type=Path)
    ap.add_argument("--stored", type=Path, default=HERE / "noise.tsv")
    ap.add_argument("--update", action="store_true")
    a = ap.parse_args()
    now = summarize(a.batch)
    if not now:
        print("noise smoke: no results in", a.batch)
        return 1
    if a.update:
        lines = ["# family\trules\tmodel   (noise_eval --seeds 1 --intensities 0.5)"] + [f"{f}\t{r}\t{m}" for f, (r, m) in now.items()]
        a.stored.write_text("\n".join(lines) + "\n")
        print(f"noise smoke: wrote {a.stored.name} ({len(now)} families)")
        return 0
    want = {}
    if a.stored.exists():
        for l in a.stored.read_text().splitlines():
            if l and not l.startswith("#"):
                f, r, m = l.split("\t")
                want[f] = [r, m]
    bad, notes = 0, []
    for f, got in now.items():
        w = want.get(f)
        for p, g in enumerate(got):
            before = w[p] if w else "?"
            if g == "CRASH":
                bad += 1
                print(f"CRASH {f} ({PATHS[p]}): rerun with noise_eval --repro {f} 0.5 0")
            elif g != "ok" and before == "ok":
                bad += 1
                print(f"FAIL  {f} ({PATHS[p]}): {g}, passed before — noise_eval --repro {f} 0.5 0")
            elif g != before:
                notes.append(f"      {f} ({PATHS[p]}): {before} -> {g}")
    for n in notes:
        print(n)
    ok = sum(1 for g in now.values() if g == ["ok", "ok"])
    if bad:
        print(f"noise smoke: {bad} new failures ({ok}/{len(now)} families pass both paths)")
        return 1
    tail = " — changed lines above; accept with `make quick-update`" if notes else ""
    print(f"noise smoke OK: {ok}/{len(now)} families pass both paths, no new failures{tail}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
