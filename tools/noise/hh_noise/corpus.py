"""Workbook- and corpus-level generation (streaming, resumable, ≤ N worker processes)."""
from __future__ import annotations

import json
import random
import resource
import time
from collections import Counter
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

from .generate import GENERATOR_VERSION, Ctx, build_sheet, sheet_name
from .model import SheetDraft
from .profiles import DEFAULT_MIX, PROFILES
from .render import render, write_gt


def _weighted(rng, d):
    keys = list(d)
    return rng.choices(keys, weights=[d[k] for k in keys])[0]


def generate_workbook(out_dir: Path, profile: str, seed: int, idx: int, sources: list[str]) -> dict:
    prof = PROFILES[profile]
    rng = random.Random(f"{profile}:{seed}:{idx}")
    lang = _weighted(rng, prof.lang)
    ctx = Ctx(rng=rng, prof=prof, lang=lang, num_style=_weighted(rng, prof.num_style))
    srcs = [Path(s) for s in sources]
    n_sheets = 1 + (rng.randint(1, 3) if ctx.p("multi_sheet") else 0)
    drafts: list[SheetDraft] = []
    names: set[str] = set()
    for i in range(n_sheets):
        d = build_sheet(ctx, srcs, "tmp")
        nm = sheet_name(rng, i, d.placed[0].block if d.placed else None, lang)
        base, k = nm, 2
        while nm.lower() in names:
            nm = f"{base[:26]} ({k})"
            k += 1
        names.add(nm.lower())
        d.name = nm
        drafts.append(d)
    if ctx.p("empty_sheet"):
        nm = f"Sheet{n_sheets + 1}"
        while nm.lower() in names:
            nm += "_"
        e = SheetDraft(name=nm, kind="empty")
        e.log("empty_sheet")
        drafts.insert(rng.randrange(len(drafts) + 1), e)
    name = f"wb_{idx:05d}"
    sub = out_dir / profile
    sub.mkdir(parents=True, exist_ok=True)
    xlsx = sub / f"{name}.xlsx"
    meta = {"profile": profile, "seed": seed, "index": idx, "generator": GENERATOR_VERSION, "lang": lang,
            "num_style": ctx.num_style}
    gt = render(drafts, xlsx, meta)
    write_gt(gt, sub / f"{name}.gt.json")
    ops = Counter()
    for s in gt["sheets"]:
        for o in s["ops"]:
            ops[o["op"]] += 1
        for t in s["tables"]:
            for o in t["ops"]:
                ops[o["op"]] += 1
    return {"file": f"{profile}/{name}.xlsx", "gt": f"{profile}/{name}.gt.json", "profile": profile, "seed": seed,
            "index": idx, "lang": lang, "sheets": len(gt["sheets"]),
            "tables": sum(len(s["tables"]) for s in gt["sheets"]), "cells": sum(len(s["cells"]) for s in gt["sheets"]),
            "ops": dict(ops)}


def _limit_mem():
    lim = 1200 * 1024**2
    resource.setrlimit(resource.RLIMIT_AS, (lim, lim))


def _job(args):
    out_dir, profile, seed, idx, sources = args
    try:
        return generate_workbook(Path(out_dir), profile, seed, idx, sources)
    except Exception as e:  # keep going; report failures in the index
        import traceback
        return {"error": f"{type(e).__name__}: {e}", "trace": traceback.format_exc()[-2000:], "profile": profile, "index": idx}


def plan(n: int, mix: dict[str, float], seed: int) -> list[tuple[str, int]]:
    rng = random.Random(f"plan:{seed}")
    return [(_weighted(rng, mix), i) for i in range(n)]


def generate_corpus(out_dir: Path, sources_dir: Path, n: int, mix: dict[str, float] | None = None, seed: int = 1,
                    workers: int = 2, log=print) -> dict:
    out_dir.mkdir(parents=True, exist_ok=True)
    sources = sorted(str(p) for p in sources_dir.glob("*.json"))
    if not sources:
        raise SystemExit(f"no cached sources in {sources_dir}; run `python -m hh_noise sources` first")
    jobs = []
    jl = out_dir / "index.jsonl"
    done = {}
    if jl.exists():
        for line in jl.read_text().splitlines():
            if line.strip():
                r = json.loads(line)
                if "error" not in r:
                    done[r["index"]] = r
    for profile, i in plan(n, mix or DEFAULT_MIX, seed):
        if i not in done:
            jobs.append((str(out_dir), profile, seed, i, sources))
    t0 = time.time()
    errors = 0
    with open(jl, "a") as f:
        if workers <= 1:
            results = map(_job, jobs)
            ex = None
        else:
            ex = ProcessPoolExecutor(workers, initializer=_limit_mem)
            results = ex.map(_job, jobs, chunksize=4)
        for k, r in enumerate(results, 1):
            f.write(json.dumps(r) + "\n")
            f.flush()
            if "error" in r:
                errors += 1
                log(f"error wb {r['index']} ({r['profile']}): {r['error']}")
            else:
                done[r["index"]] = r
            if k % 100 == 0:
                log(f"{k}/{len(jobs)} generated · {time.time() - t0:.0f}s")
        if ex:
            ex.shutdown()
    recs = [done[i] for i in sorted(done)]
    ops = Counter()
    for r in recs:
        ops.update(r["ops"])
    index = {"schema": "hh-noise-index/1", "generator": GENERATOR_VERSION, "seed": seed, "mix": mix or DEFAULT_MIX,
             "sources": [Path(s).name for s in sources], "workbooks": len(recs),
             "sheets": sum(r["sheets"] for r in recs), "tables": sum(r["tables"] for r in recs),
             "cells": sum(r["cells"] for r in recs), "operator_counts": dict(ops.most_common()),
             "files": [{k: r[k] for k in ("file", "gt", "profile", "seed", "index", "lang", "sheets", "tables", "cells")} for r in recs]}
    (out_dir / "index.json").write_text(json.dumps(index, ensure_ascii=False, indent=1))
    log(f"done: {len(recs)} workbooks ({errors} errors) in {time.time() - t0:.0f}s")
    return index
