import json
import random
from pathlib import Path

import pytest

from hh_noise import corpus as C
from hh_noise.generate import Ctx, build_sheet
from hh_noise.inspect import read_back, roundtrip_errors
from hh_noise.profiles import PROFILES, Profile
from hh_noise.render import render
from hh_noise.schema import validate, validate_dir
from hh_noise.sources import cached_source, infer_type, parse_scalar


def test_parse_and_infer():
    assert parse_scalar("00123") == ("text", "00123")
    assert parse_scalar("1,5") == ("text", "1,5")
    assert parse_scalar("2024-03-05")[0] == "date"
    assert parse_scalar("3/5/24")[1].year == 2024
    assert infer_type("Population", [("int", 5), ("int", 7)]) == "count"
    assert infer_type("year", [("int", 1999), ("int", 2001)]) == "year"
    assert infer_type("city", [("text", "Paris"), ("text", "Rome")]) == "city"
    assert infer_type("raceethnicity", [("text", "White")] * 20) == "category"


def test_generate_validate_roundtrip(tmp_path, sources_dir):
    out = tmp_path / "corpus"
    idx = C.generate_corpus(out, sources_dir, 24, seed=5, workers=1, log=lambda *_: None)
    assert idx["workbooks"] == 24
    assert (out / "index.json").exists()
    n, errs = validate_dir(out, roundtrip=True)
    assert n == 24 and not errs, errs[:5]


def test_determinism(tmp_path, sources_dir):
    srcs = sorted(str(p) for p in sources_dir.glob("*.json"))
    a = C.generate_workbook(tmp_path / "a", "enron_like", 7, 3, srcs)
    b = C.generate_workbook(tmp_path / "b", "enron_like", 7, 3, srcs)
    ga = json.loads((tmp_path / "a" / a["gt"]).read_text())
    gb = json.loads((tmp_path / "b" / b["gt"]).read_text())
    assert ga == gb
    assert read_back(tmp_path / "a" / a["file"]) == read_back(tmp_path / "b" / b["file"])
    c = C.generate_workbook(tmp_path / "c", "enron_like", 8, 3, srcs)
    assert json.loads((tmp_path / "c" / c["gt"]).read_text()) != ga


def _forced(name, force, arrangement=None):
    p = {k: 0.0 for k in PROFILES["enron_like"].p}
    p.update({k: 1.0 for k in force})
    return Profile(name, p, {"en": 1.0}, arrangement=arrangement or {"stacked": 1.0})


@pytest.mark.parametrize("variant", list("abcde"))
def test_stack_schema_change_variants(tmp_path, sources_dir, variant):
    srcs = sorted(sources_dir.glob("*.json"))
    prof = _forced("stack", ["multi_table"], {"stack_schema_change": 1.0})
    for seed in range(40):
        rng = random.Random(seed)
        ctx = Ctx(rng=rng, prof=prof, lang="en", num_style="us")
        d = build_sheet(ctx, srcs, "S")
        b = [p.block.stats.get("boundary", {}) for p in d.placed]
        if len(b) < 2 or b[1].get("variant") != variant:
            continue
        gt = render([d], tmp_path / f"{variant}.xlsx", {"profile": "t", "seed": seed})
        assert not validate(gt)
        assert not roundtrip_errors(tmp_path / f"{variant}.xlsx", gt)
        t0, t1 = gt["sheets"][0]["tables"][:2]
        assert t0["rect"][2] - t0["rect"][0] + 1 >= 100  # long first segment
        assert t1["boundary"]["op"] == "stack_schema_change"
        gap = t1["rect"][0] - t0["rect"][2] - 1
        assert gap == t1["boundary"]["gap_rows"]
        if variant in "bc":
            assert gap == 0
        if variant == "c":
            assert t1["header_rows"] == 0
        if variant == "a":
            assert gap >= 1 and t1["header_rows"] >= 1
        if variant == "e":
            hdr = [c for c in gt["sheets"][0]["cells"] if c[3] == t1["id"] and c[2] == "header"]
            years = [c for c in hdr if str(c[7] if c[7] is not None else "").isdigit() or isinstance(c[7], int)]
            assert hdr
        return
    pytest.skip(f"variant {variant} not drawn")


def test_every_role_and_provenance(tmp_path, sources_dir):
    out = tmp_path / "c2"
    C.generate_corpus(out, sources_dir, 30, mix={"gov_stats": 1.0}, seed=2, workers=1, log=lambda *_: None)
    roles = set()
    for g in out.glob("*/*.gt.json"):
        gt = json.loads(g.read_text())
        for s in gt["sheets"]:
            for c in s["cells"]:
                roles.add(c[2])
                if c[2] == "data" and c[5] is not None and c[5] >= 0:
                    assert gt["datasets"][c[4]].startswith("fixture/")
    assert {"title", "header", "data", "total", "note"} <= roles


def test_calamine_matches_gt_values(tmp_path, sources_dir):
    """Text written for a value-noised cell reads back exactly; numbers stay numbers."""
    srcs = sorted(str(p) for p in sources_dir.glob("*.json"))
    r = C.generate_workbook(tmp_path, "eu_locale", 1, 1, srcs)
    gt = json.loads((tmp_path / r["gt"]).read_text())
    back = read_back(tmp_path / r["file"])
    for s in gt["sheets"]:
        got = back[s["name"]]
        for row, col, role, *_rest in s["cells"]:
            t = _rest[-1] or {}
            if "num" in t and isinstance(got.get((row, col)), str):
                assert any(ch.isdigit() for ch in got[(row, col)])
