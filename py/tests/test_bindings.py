import json

import pytest

import hedgehog


def test_sheets_and_cells(sales_xlsx):
    wb = hedgehog.open(sales_xlsx)
    sheets = {s["name"]: s for s in wb.sheets()}
    assert sheets["Sales"]["used_range"] == "A1:C6"
    assert sheets["Sales"]["formulas"] == 1
    assert sheets["My Notes"]["used_range"] == "A1:B3"

    c = wb.get_cell("Sales", "B5")
    assert c == {"sheet": "Sales", "address": "B5", "value": 16.0, "type": "number", "formula": None}
    f = wb.get_cell(None, "Sales!$C$6")
    assert f["formula"] == "=SUM(C2:C4)" and f["type"] == "empty"  # openpyxl saves no cached value
    q = wb.get_cell(None, "'My Notes'!B3")
    assert q["value"] == 1234.5
    assert wb.get_cell("sales", "A1")["value"] == "Region"  # sheet lookup is case-insensitive
    assert wb.get_cell("Sales", "Z99")["type"] == "empty"


def test_bad_refs(sales_xlsx):
    wb = hedgehog.open(sales_xlsx)
    with pytest.raises(ValueError):
        wb.get_cell("Sales", "not a ref")
    with pytest.raises(KeyError):
        wb.get_cell("Nope", "A1")
    with pytest.raises(ValueError):
        wb.get_range("Sales", "A1:Z1000", 100)


def test_range_and_find(sales_xlsx):
    wb = hedgehog.open(sales_xlsx)
    r = wb.get_range("Sales", "A1:C2")
    assert r["rows"] == [["Region", "Q1", "Q2"], ["North", 10.0, 20.0]]
    hits = wb.find_text("beograd")  # Cyrillic cell found by Latin query
    assert [(h["sheet"], h["address"]) for h in hits] == [("Sales", "A4")]
    assert wb.find_text("finance", "My Notes")[0]["address"] == "A1"


def test_totals(sales_xlsx):
    wb = hedgehog.open(sales_xlsx)
    by_cell = {t["address"]: t for t in wb.totals("Sales")}
    assert by_cell["B5"]["ok"] is True and by_cell["B5"]["sources"] == ["B2:B4"]
    assert by_cell["C5"]["ok"] is False and by_cell["C5"]["expected"] == 29.0 and by_cell["C5"]["actual"] == 30.0
    assert by_cell["C6"]["reason"] == "no_cached_value" and by_cell["C6"]["origin"] == "formula"
    assert by_cell["B5"]["label"] == "Total"


def test_facts_json(sales_xlsx):
    f = json.loads(hedgehog.facts_json(sales_xlsx))
    assert f["totals"]["found"] == 3 and f["totals"]["mismatched"] == 1
    assert f["totals"]["items"][0]["cell"] == "Sales!C5"  # mismatches first
    notes = [s for s in f["sheets"] if s["name"] == "My Notes"][0]
    assert notes["values"]["rows"][2][1] == 1234.5
