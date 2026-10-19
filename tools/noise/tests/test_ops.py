import copy
import datetime as dt
import random

import pytest
from conftest import tiny_block

from hh_noise import ops_structure as S
from hh_noise import ops_value as V
from hh_noise.generate import Ctx
from hh_noise.profiles import BASE, Profile


def ctx(force=(), num_style="us", lang="en", seed=0):
    p = {k: 0.0 for k in BASE}
    for k in force:
        p[k] = 1.0
    return Ctx(rng=random.Random(seed), prof=Profile("test", p, {lang: 1.0}), lang=lang, num_style=num_style)


def provenance(b):
    return {id(c): (c.src, c.orig) for c in b.cells.values()}


def check_invariants(b, before):
    # provenance of surviving cells never changes; every cell with a source is accounted for
    for c in b.cells.values():
        if id(c) in before:
            assert (c.src, c.orig) == before[id(c)]
    for (r, c), cell in b.cells.items():
        assert 0 <= r < b.n_rows and 0 <= c < b.n_cols, (r, c, b.n_rows, b.n_cols)
        assert cell.v is not None
    assert len(b.row_kind) == b.n_rows and len(b.columns) == b.n_cols


# ---------------------------------------------------------------- number / date formatting


@pytest.mark.parametrize("style,expect", [("us", "1,234.50"), ("eu", "1.234,50"), ("fr", "1 234,50"),
                                          ("fr_space", "1 234,50"), ("ch", "1'234.50"), ("plain", "1234.50")])
def test_fmt_num(style, expect):
    assert V.fmt_num(1234.5, style, 2) == expect
    assert V.fmt_num(-1234.5, style, 2) == "-" + expect


@pytest.mark.parametrize("fmt,lang,expect", [("iso", "en", "2024-03-05"), ("us", "en", "3/5/2024"), ("eu_dot", "de", "05.03.2024"),
                                             ("sr_dot", "sr", "05.03.2024."), ("de_long", "de", "5. März 2024"),
                                             ("es_long", "es", "5 de marzo de 2024"), ("d_month_y", "fr", "5 mars 2024"),
                                             ("d_month_y", "it", "5 marzo 2024"), ("d_month_y", "nl", "5 maart 2024"),
                                             ("d_mon_yy", "en", "5-Mar-24")])
def test_date_text(fmt, lang, expect):
    assert V.date_text(dt.date(2024, 3, 5), fmt, lang) == expect


# ---------------------------------------------------------------- stage A operators

VALUE_CASES = {
    "missing_tokens": lambda b: any("missing_tokens" in c.ops for c in b.cells.values()) or any("missing_tokens" in e["ops"] for e in b.implied),
    "numbers_as_text": lambda b: any(isinstance(c.v, str) and c.true and float(c.v) == c.true["num"] for c in b.cells.values()),
    "locale_numbers": lambda b: any(c.true and "num" in c.true and "," in c.v for c in b.cells.values()),
    "currency_units": lambda b: any("currency_units" in c.ops for c in b.cells.values()),
    "negative_formats": lambda b: any("negative_formats" in c.ops and ("(" in c.v or c.v.endswith("-") or "−" in c.v) for c in b.cells.values()),
    "dates_as_text": lambda b: any(c.true and "date" in c.true and isinstance(c.v, str) for c in b.cells.values()),
    "date_serials": lambda b: any(c.true and "serial" in c.true and c.v == c.true["serial"] for c in b.cells.values()),
    "whitespace_noise": lambda b: any(isinstance(c.v, str) and c.v != c.v.strip() or "  " in str(c.v) or " " in str(c.v) for c in b.cells.values()),
    "case_noise": lambda b: any("case_noise" in c.ops and c.v != c.orig and c.v.lower() == c.orig.lower() for c in b.cells.values()),
    "categorical_variants": lambda b: any("categorical_variants" in c.ops and c.true["canon"] == c.orig for c in b.cells.values()),
    "typos": lambda b: any("typos" in c.ops and c.v != c.orig for c in b.cells.values()),
    "value_drift": lambda b: any("value_drift" in c.ops for c in b.cells.values()),
    "footnote_markers": lambda b: any("footnote_markers" in c.ops for c in b.cells.values()) and b.stats.get("footnote"),
    "excel_errors": lambda b: any(c.error for c in b.cells.values()),
    "header_rename": lambda b: any("header_rename" in c.ops for c in b.cells.values()),
}


@pytest.mark.parametrize("op", sorted(VALUE_CASES))
def test_value_operator(op, block):
    fn = V.VALUE_OPS[op][0]
    for seed in range(20):
        b = copy.deepcopy(block)
        before = provenance(b)
        if fn(b, random.Random(seed), ctx(num_style="eu" if op == "locale_numbers" else "us")) and VALUE_CASES[op](b):
            check_invariants(b, before)
            assert any(o["op"] == op for o in b.ops)
            # the cell's truth matches its provenance for numbers that became text
            for c in b.cells.values():
                if c.true and "num" in c.true and isinstance(c.orig, (int, float)) and not isinstance(c.orig, bool) \
                        and op in ("numbers_as_text", "locale_numbers", "currency_units", "negative_formats"):
                    assert abs(c.true["num"] - c.orig) <= 0.51, (c.v, c.true, c.orig)
            return
    pytest.fail(f"{op} never applied")


def test_percent_text(block):
    b = copy.deepcopy(block)
    assert V.percent_text(b, random.Random(0), ctx())
    for c in b.cells.values():
        if "percent_text" in c.ops:
            assert c.v.endswith("%")
            assert abs(c.true["num"] - c.orig) < 0.01  # fraction source → fraction truth


def test_scale_thousands():
    b = tiny_block()
    assert V.scale_thousands(b, random.Random(0), ctx())
    assert b.scale == 1000
    c = b.cells[(6, 1)]
    assert c.v in (1234.6, 1235, 1234.567, 1234.5) or abs(c.v - 1234.567) < 1
    assert c.orig == 1234567.0


def test_value_drift_keeps_true_column():
    b = tiny_block()
    for seed in range(10):
        bb = copy.deepcopy(b)
        if V.value_drift(bb, random.Random(seed), ctx()):
            for (r, c), cell in bb.cells.items():
                if "value_drift" in cell.ops:
                    assert cell.true["true_col"] == cell.src[1]
                    assert c == cell.true["true_col"] + cell.true["drift"]
            return
    pytest.fail("no drift")


def test_bool_variants(block):
    b = copy.deepcopy(block)
    if not V.bool_variants(b, random.Random(2), ctx()):
        pytest.skip("no bool column sampled")
    for c in b.cells.values():
        if "bool_variants" in c.ops:
            assert c.true["bool"] == c.orig


# ---------------------------------------------------------------- stage B operators


def _sum_ok(b):
    for (r, c), cell in b.cells.items():
        if cell.formula is not None:
            assert cell.formula["refs"], (r, c)
            for rr, cc in cell.formula["refs"]:
                assert cc == c or rr == r
                assert b.row_kind[rr] in ("data", "total")


def test_subtotals_and_group_rows():
    b = tiny_block()
    applied = S.grouping(b, random.Random(0), ctx(force=("group_header_rows", "subtotals", "grand_total_after_subtotals")))
    assert "group_header_rows" in applied and "subtotals" in applied and "grand_total" in applied
    kinds = b.row_kind
    assert kinds.count("group_header") == 2 and kinds.count("total") == 3
    assert b.columns[0].header == "Amount"  # key column moved into group rows
    _sum_ok(b)
    # each subtotal references exactly the data rows of its group
    for r, k in enumerate(kinds):
        if k == "total" and r != b.n_rows - 1:
            refs = b.cells[(r, 0)].formula["refs"] if (r, 0) in b.cells and b.cells[(r, 0)].formula else None
            if refs:
                assert all(kinds[rr] == "data" for rr, _ in refs) and len(refs) == 3
    assert len([e for e in b.implied if e.get("moved_to_group")]) == 6


def test_group_label_column_period():
    b = tiny_block()
    applied = S.grouping(b, random.Random(0), ctx(force=("group_label_column",)))
    assert applied == ["group_label_column"]
    assert b.columns[0].derived == "group_label"
    labels = [b.cells[(r, 0)] for r, k in enumerate(b.row_kind) if k == "group_header"]
    assert len(labels) == 3 and all(c.role == "group_header" for c in labels)
    assert [c.orig for c in labels] == ["2024-03", "2024-04", "2024-05"]
    # data rows have nothing in the label column
    assert all((r, 0) not in b.cells for r, k in enumerate(b.row_kind) if k == "data")


def test_blank_repeats_and_vmerge():
    for op in ("blank_repeats", "vertical_merge_keys"):
        b = tiny_block()
        applied = S.grouping(b, random.Random(1), ctx(force=(op,)))
        assert op in applied
        assert b.implied and all(op in e["ops"] for e in b.implied)
        if op == "vertical_merge_keys":
            assert b.merges


def test_grand_total_and_wrong_total():
    b = tiny_block()
    assert S.grand_total(b, random.Random(0), ctx())
    assert b.row_kind[-1] == "total"
    tot = b.cells[(b.n_rows - 1, 1)]
    assert len(tot.formula["refs"]) == 6
    assert S.wrong_total(b, random.Random(0), ctx())
    from hh_noise.render import _resolve_totals
    _resolve_totals(b)
    assert tot.true["wrong"] and tot.v != tot.true["num"]
    assert abs(tot.true["num"] - sum(b.cells[(r, 1)].orig for r in range(1, 7))) < 1e-6


def test_multirow_header():
    b = tiny_block()
    b.columns.append(copy.deepcopy(b.columns[1]))
    for r in range(7):
        if (r, 1) in b.cells:
            b.cells[(r, 3)] = copy.deepcopy(b.cells[(r, 1)])
    b.n_cols = 4
    assert S.multirow_header(b, random.Random(0), ctx())
    assert b.header_rows >= 2
    assert all(b.row_kind[r] == "header" for r in range(b.header_rows))
    assert any(len(ci.header_path) >= 2 for ci in b.columns)


def test_row_label_column():
    b = tiny_block()
    b.cells[(0, 0)].v = "Item"
    b.columns[0].stype = "name"
    for r in range(1, 7):
        b.cells[(r, 0)].v = b.cells[(r, 0)].orig = f"Item {r}"
    assert S.row_label_column(b, random.Random(3), ctx())
    assert b.columns[0].derived == "row_label"
    for r in range(1, 7):
        c = b.cells[(r, 0)]
        assert c.role == "data" and c.v.startswith(c.orig)


def test_transpose():
    b = tiny_block()
    assert S.transpose(b, random.Random(0), ctx())
    assert b.orientation == "row" and b.header_cols == 1 and b.header_rows == 0
    assert b.cells[(1, 0)].v == "Amount" and b.cells[(1, 0)].role == "header"


def test_spacers_and_repeat_header(block):
    b = copy.deepcopy(block)
    before = provenance(b)
    assert S.spacer_rows(b, random.Random(0), ctx())
    assert "spacer" in b.row_kind
    assert S.spacer_cols(b, random.Random(0), ctx())
    assert any(ci.stype == "spacer" for ci in b.columns)
    check_invariants(b, before)
