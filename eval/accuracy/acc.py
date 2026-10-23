"""Shared pieces of the accuracy scoreboard: the split, the gold atoms per item, hedgehog's tables.

Model-free and deterministic. Tool outputs come from eval/public/run_hedgehog.py (cells.tsv, normalize
--csv --keep-totals + manifest, catalog.txt) under $HH_PUBLIC_WORK (default here:
../scratch-archive/accuracy/work, so a scoreboard run never mixes with eval/public's own outputs).

Gold atoms (one item -> several atoms; each atom is measured by L or by C):

- HiTab: `lookup` items -> L with the gold cell and header paths; `computed` items with numeric answers
  -> C (the gold operand cells are kept for the oracle's validation); `header_answer` and text answers of
  computed items -> not attempted.
- RealHiTBench: Data Analysis and Visualization -> not attempted (free text, charts). Numerical Reasoning:
  number atoms -> C, text atoms (labels a comparison or ranking names) -> L. Fact Checking, Structure
  Comprehending -> L.
- MiMoTable (English): the gold key of eval/public/agent_mimotable.py (`gold_key`: quoted phrases, dates,
  numbers of the narrative that the question does not contain; KEY_KEEP trims; items of AUDIT_EXCLUDE
  are not attempted). Lookup items -> L; Compare / Calculate / Reasoning: number atoms -> C, others -> L.
  Chinese narratives -> not attempted (no deterministic key extractor for them).
- An L atom that is a number and not a cell of the source file (raw `hedgehog cells`) needs a computation:
  it moves to C. An L text atom that is no source cell is not attempted (a paraphrase; nothing to find).
"""

from __future__ import annotations

import json
import os
import re
import sys
from functools import lru_cache
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
PROJECT = REPO.parent
os.environ.setdefault("HH_PUBLIC_WORK", str(PROJECT / "scratch-archive" / "accuracy" / "work"))
sys.path.insert(0, str(REPO / "eval" / "public"))
sys.path.insert(0, str(HERE))

import agent_mimotable as AM  # noqa: E402
import score as PS  # noqa: E402  (eval/public/score.py)
import split as SPLIT  # noqa: E402
from common import data_dir, norm_text, parse_number, read_jsonl  # noqa: E402

DATASETS = ("hitab", "mimotable", "realhitbench")


# ------------------------------------------------------------------------------------------------
# split


@lru_cache(maxsize=None)
def side_files(dataset: str, side: str) -> frozenset[str]:
    return frozenset(SPLIT.load(dataset, side))


def item_side(dataset: str, it: dict) -> str:
    fs = it.get("files") or [it["file"]]
    dev, hold = side_files(dataset, "dev"), side_files(dataset, "holdout")
    if all(f in dev for f in fs):
        return "dev"
    if all(f in hold for f in fs):
        return "holdout"
    return "?"


# ------------------------------------------------------------------------------------------------
# gold atoms


def _num(text: str):
    return parse_number(text)


def _atom(text: str, kind: str, cls: str, **kw) -> dict:
    a = {"text": str(text), "kind": kind, "cls": cls}
    a.update(kw)
    return a


def hitab_items(items: list[dict]) -> list[dict]:
    out = []
    for it in items:
        rec = {"id": it["id"], "files": [it["file"]], "question": it["question"], "group": it["kind"],
               "sheet": it["sheet"], "cells": it.get("cells") or [], "aggregation": it.get("aggregation")}
        if it["kind"] == "lookup":
            rec["atoms"] = [_atom(x, "number" if parse_number(x) else "text", "L") for x in it["answer"]]
            rec["hitab_lookup"] = bool(it["cells"]) and not it["cells"][0]["in_header_rows"]
        elif it["kind"] == "computed" and it["answer"] and all(isinstance(x, (int, float)) or parse_number(x) for x in it["answer"]):
            rec["atoms"] = [_atom(_fmt_float(x), "number", "C") for x in it["answer"]]
        else:
            rec["atoms"] = []
            rec["not_attempted"] = "header answer" if it["kind"] == "header_answer" else "text answer of a computation"
        out.append(rec)
    return out


def _fmt_float(x) -> str:
    if isinstance(x, float):
        if x == int(x) and abs(x) < 1e15:
            return str(int(x))
        return repr(x)
    return str(x)


def rhtb_items(items: list[dict]) -> list[dict]:
    out = []
    for it in items:
        rec = {"id": it["id"], "files": it["files"], "question": it["question"], "group": it["qtype"]}
        if it.get("missing"):
            rec.update(atoms=[], not_attempted="file missing")
        elif it["qtype"] in ("Data Analysis", "Visualization") or it.get("task") == "not_measured":
            rec.update(atoms=[], not_attempted="free text or chart")
        else:
            comp = it["qtype"] == "Numerical Reasoning"
            rec["atoms"] = [_atom(a["text"], a["kind"], "C" if comp and a["kind"] == "number" else "L") for a in it["atoms"]]
            if not rec["atoms"]:
                rec["not_attempted"] = "no answer atoms"
        out.append(rec)
    return out


def mimo_items(items: list[dict]) -> list[dict]:
    out = []
    for it in items:
        rec = {"id": it["id"], "files": it["files"], "question": it["question"], "group": "",
               "table_type": it["table_type"]}
        if it["lang"] != "english":
            rec.update(atoms=[], not_attempted="Chinese narrative (no key extractor)")
        elif it.get("missing"):
            rec.update(atoms=[], not_attempted="file missing")
        elif {"Visualize", "Edit"} & set(it["ops"]):
            rec.update(atoms=[], not_attempted="visualize / edit")
        elif it["id"] in AM.AUDIT_EXCLUDE:
            rec.update(atoms=[], not_attempted="key unfair (hand audit of agent_mimotable)")
        else:
            key = AM.gold_key(it)
            tc = AM.task_class(it["ops"])
            rec["group"] = tc
            if key is None:
                rec.update(atoms=[], not_attempted="no checkable key in the narrative")
            else:
                atoms = []
                for k in key:
                    cls = "C" if tc != "lookup" and k["kind"] == "number" else "L"
                    extra = {x: k[x] for x in ("scale", "m", "d", "y") if x in k}
                    atoms.append(_atom(k["text"], k["kind"], cls, **extra))
                rec["atoms"] = atoms
                rec["audited_key"] = it["id"] in _agent_subset_ids()
        out.append(rec)
    return out


@lru_cache(maxsize=1)
def _agent_subset_ids() -> frozenset[str]:
    try:
        return frozenset(x["id"] for x in AM.subset())
    except Exception:  # noqa: BLE001
        return frozenset()


def load_items(dataset: str, side: str = "dev") -> list[dict]:
    raw = read_jsonl(data_dir() / dataset / "items.jsonl")
    raw = [it for it in raw if item_side(dataset, it) == side]
    conv = {"hitab": hitab_items, "realhitbench": rhtb_items, "mimotable": mimo_items}[dataset](raw)
    for r in conv:
        r["dataset"] = dataset
        r["side"] = side
    return conv


# ------------------------------------------------------------------------------------------------
# hedgehog's tables (eval/public/score.py `hh_load`, plus dtypes and manifest metadata)


@lru_cache(maxsize=96)
def load(dataset: str, rel: str):
    """(meta, raw cells {(sheet,row,col): text}, catalog text, [Table]) with each Table carrying
    `.dtypes` (per kept column), `.manifest` (the manifest's table entry), `.file`."""
    meta, raw, catalog, tables = PS.hh_load(dataset, rel)
    d = PS.work_dir() / dataset / "hh" / PS.key_of(rel)
    mf = next((d / "norm").glob("*.manifest.json"), None) if (d / "norm").exists() else None
    mt = {}
    if mf:
        for t in json.loads(mf.read_text()).get("tables", []):
            mt[t["id"]] = t
    for t in tables:
        m = mt.get(t.id, {})
        cols = [c for c in m.get("columns", []) if c["role"] in ("data", "group")]
        t.dtypes = [c["dtype"] for c in cols] + ["?"] * (len(t.headers) - len(cols))
        t.colmeta = cols
        t.manifest = m
        t.file = rel
    return meta, raw, catalog, tables


def raw_values(dataset: str, files: list[str]) -> list[str]:
    return [v for f in files for v in load(dataset, f)[1].values()]


TOTAL_Q = re.compile(r"\b(total|totals|overall|in all|altogether|combined|grand)\b|合计|总计|总共|一共")


def has_total_word(q: str) -> bool:
    return bool(TOTAL_Q.search(norm_text(q)))
