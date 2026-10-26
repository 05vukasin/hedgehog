"""Dev / holdout split of the public QA benchmarks (file lists only; no question text).

    python3 -I eval/accuracy/split.py            # writes eval/accuracy/split/<dataset>.{dev,holdout}.txt
    python3 -I eval/accuracy/split.py --check    # recompute and compare with the committed lists

Rules (docs/evaluation.md, "Question-answering accuracy"):

- HiTab: the official split. train + dev -> dev, test -> holdout. HiTab's splits are file-disjoint
  (every trimmed workbook belongs to exactly one split; checked here).
- MiMoTable and RealHiTBench: a seeded, file-level 80 / 20 split. Files that share a question
  (MiMoTable's multi-file items) are one unit, so all questions of a file land on one side.
  Strata: MiMoTable `table_type` x `lang`; RealHiTBench `structure` (the table-structure class of the
  file's questions; "Backgroundcolor" merged into "BackgroundColor"). Within a stratum, units are
  ordered by sha256(SEED + ":" + unit key) and the first round(20 % x n) go to holdout.

SEED = "hedgehog-accuracy-holdout-2026-10-23". Changing it (or the rules) re-freezes a new holdout; the
plan allows that only at a release, with both holdouts reported.
"""

from __future__ import annotations

import argparse
import hashlib
import sys
from collections import Counter, defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "public"))
from common import data_dir, read_jsonl  # noqa: E402

SEED = "hedgehog-accuracy-holdout-2026-10-23"
HOLDOUT_SHARE = 0.20
OUT = HERE / "split"
DATASETS = ("hitab", "mimotable", "realhitbench")


def _h(key: str) -> str:
    return hashlib.sha256(f"{SEED}:{key}".encode()).hexdigest()


def _units(items: list[dict]) -> list[set[str]]:
    """Connected components of files linked by items (union-find)."""
    parent: dict[str, str] = {}

    def find(x: str) -> str:
        parent.setdefault(x, x)
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    for it in items:
        fs = it.get("files") or [it["file"]]
        for f in fs:
            find(f)
        for f in fs[1:]:
            parent[find(f)] = find(fs[0])
    comp: dict[str, set[str]] = defaultdict(set)
    for f in parent:
        comp[find(f)].add(f)
    return list(comp.values())


def seeded(items: list[dict], stratum_of) -> tuple[list[str], list[str]]:
    by_file: dict[str, list[dict]] = defaultdict(list)
    for it in items:
        for f in it.get("files") or [it["file"]]:
            by_file[f].append(it)
    strata: dict[str, list[set[str]]] = defaultdict(list)
    for u in _units(items):
        votes = Counter(stratum_of(it) for f in u for it in by_file[f])
        s = sorted(votes.items(), key=lambda kv: (-kv[1], kv[0]))[0][0]
        strata[s].append(u)
    dev, hold = [], []
    for s in sorted(strata):
        us = sorted(strata[s], key=lambda u: _h("\x1f".join(sorted(u))))
        k = round(HOLDOUT_SHARE * len(us))
        for i, u in enumerate(us):
            (hold if i < k else dev).extend(u)
    return sorted(dev), sorted(hold)


def compute() -> dict[str, tuple[list[str], list[str]]]:
    out = {}
    hi = read_jsonl(data_dir() / "hitab" / "items.jsonl")
    sp: dict[str, set[str]] = defaultdict(set)
    for it in hi:
        sp[it["file"]].add(it["split"])
    both = [f for f, s in sp.items() if len(s) > 1 and "test" in s]
    if both:
        raise SystemExit(f"HiTab files in test and another split: {len(both)}")
    out["hitab"] = (sorted(f for f, s in sp.items() if "test" not in s), sorted(f for f, s in sp.items() if "test" in s))
    mi = read_jsonl(data_dir() / "mimotable" / "items.jsonl")
    out["mimotable"] = seeded(mi, lambda it: f"{it['table_type']} | {it['lang']}")
    rh = read_jsonl(data_dir() / "realhitbench" / "items.jsonl")
    out["realhitbench"] = seeded(rh, lambda it: (it.get("structure") or "?").replace("Backgroundcolor", "BackgroundColor"))
    return out


def load(dataset: str, side: str) -> set[str]:
    p = OUT / f"{dataset}.{side}.txt"
    return {x for x in p.read_text().splitlines() if x.strip()} if p.exists() else set()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    a = ap.parse_args()
    res = compute()
    bad = 0
    for d, (dev, hold) in res.items():
        assert not set(dev) & set(hold), d
        if a.check:
            same = load(d, "dev") == set(dev) and load(d, "holdout") == set(hold)
            bad += not same
            print(f"{d}: {'same' if same else 'DIFFERENT'}")
            continue
        OUT.mkdir(parents=True, exist_ok=True)
        (OUT / f"{d}.dev.txt").write_text("".join(f + "\n" for f in dev))
        (OUT / f"{d}.holdout.txt").write_text("".join(f + "\n" for f in hold))
        print(f"{d}: dev {len(dev)} files, holdout {len(hold)} files")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
