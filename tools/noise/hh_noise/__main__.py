"""CLI: python -m hh_noise {sources,generate,calibrate,validate,show,ops} ...

  sources   --clean DIR --out DIR                   cache clean tables (bounded samples + types)
  generate  --out DIR --n N [--mix default|PROFILE|p1=w,p2=w] [--seed S] [--workers 2] [--sources DIR]
  calibrate --dir DIR [--sample 300]                 generated vs target statistics (markdown)
  validate  --dir DIR [--limit N] [--roundtrip]      schema check of every GT (+ calamine round trip)
  show      FILE.xlsx [--sheet K]                    text view with roles
  ops                                                list operators
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

DATA = Path("/home/vukasin/projects/excell-lib-project/testdata")


def main(argv=None):
    ap = argparse.ArgumentParser(prog="hh_noise")
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("sources")
    s.add_argument("--clean", type=Path, default=DATA / "clean")
    s.add_argument("--out", type=Path, default=DATA / "synthetic" / "_sources")
    g = sub.add_parser("generate")
    g.add_argument("--out", type=Path, required=True)
    g.add_argument("--n", type=int, default=100)
    g.add_argument("--mix", default="default")
    g.add_argument("--seed", type=int, default=1)
    g.add_argument("--workers", type=int, default=1)
    g.add_argument("--sources", type=Path, default=DATA / "synthetic" / "_sources")
    c = sub.add_parser("calibrate")
    c.add_argument("--dir", type=Path, required=True)
    c.add_argument("--sample", type=int, default=300)
    c.add_argument("--profiles", default="enron_like,gov_stats,eu_locale,clean_ish")
    v = sub.add_parser("validate")
    v.add_argument("--dir", type=Path, required=True)
    v.add_argument("--limit", type=int)
    v.add_argument("--roundtrip", action="store_true")
    sh = sub.add_parser("show")
    sh.add_argument("file", type=Path)
    sh.add_argument("--sheet", type=int, default=0)
    sh.add_argument("--rows", type=int, default=60)
    sub.add_parser("ops")
    a = ap.parse_args(argv)

    if a.cmd == "sources":
        from .sources import build_cache
        build_cache(a.clean, a.out)
    elif a.cmd == "generate":
        from .corpus import generate_corpus
        from .profiles import DEFAULT_MIX, PROFILES
        if a.mix == "default":
            mix = DEFAULT_MIX
        elif a.mix in PROFILES:
            mix = {a.mix: 1.0}
        else:
            mix = {k: float(w) for k, w in (x.split("=") for x in a.mix.split(","))}
        generate_corpus(a.out, a.sources, a.n, mix, a.seed, a.workers)
    elif a.cmd == "calibrate":
        from .stats import report
        print(report(a.dir, a.profiles.split(","), a.sample))
    elif a.cmd == "validate":
        from .schema import validate_dir
        n, errs = validate_dir(a.dir, a.limit, a.roundtrip)
        print(f"{n} ground-truth files checked, {len(errs)} with errors")
        for e in errs[:30]:
            print(" ", e)
        return 1 if errs else 0
    elif a.cmd == "show":
        from .inspect import show
        print(show(a.file, a.file.with_suffix(".gt.json"), a.sheet, max_rows=a.rows))
    elif a.cmd == "ops":
        from .generate import LAYOUT_OPS, PRESENTATION_OPS
        from .ops_structure import STRUCT_OPS
        from .ops_value import VALUE_OPS
        for stage, d in (("A value", {k: v[1] for k, v in VALUE_OPS.items()}), ("B structure", {k: v[1] for k, v in STRUCT_OPS.items()}),
                         ("C layout", LAYOUT_OPS), ("D presentation", PRESENTATION_OPS)):
            print(f"## {stage} ({len(d)})")
            for k, doc in d.items():
                print(f"- `{k}`: {doc}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
