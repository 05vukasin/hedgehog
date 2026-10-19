"""hedgehog.normalize: the output must load cleanly with openpyxl and pandas."""

import datetime as dt
import json

import pytest

import hedgehog

openpyxl = pytest.importorskip("openpyxl")


@pytest.fixture
def messy_xlsx(tmp_path):
    """Title, 2-row merged header, group headers, subtotals + grand total, numbers/dates as text."""
    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "Prodaja"
    ws["A1"] = "Prodaja po regionima 2024"
    ws["A3"] = "Artikal"
    ws.merge_cells("A3:A4")
    ws["B3"] = "Iznos"
    ws.merge_cells("B3:C3")
    ws["B4"] = "Q1"
    ws["C4"] = "Q2"
    ws["D3"] = "Datum"
    ws.merge_cells("D3:D4")
    ws["E3"] = "Udeo"
    ws.merge_cells("E3:E4")
    rows = [
        ("Region: Sever",),
        ("Jabuka", "1.200,50", 1000, "15.03.2024.", "12%"),
        ("Kruška", 300, "2.000,00", dt.datetime(2024, 3, 20), "n/a"),
        ("Ukupno Sever", 1500.5, 3000),
        ("Region: Jug",),
        ("Banana", 50, 60, "01.04.2024.", "5%"),
        ("Kivi", "-", 70, "02.04.2024.", "7,5%"),
        ("Ukupno Jug", 50, 130),
        ("Sveukupno", 1550.5, 3130),
    ]
    for i, row in enumerate(rows):
        for j, v in enumerate(row):
            ws.cell(row=5 + i, column=1 + j, value=v)
    ws["A15"] = "Izvor: interna evidencija"
    path = tmp_path / "prodaja.xlsx"
    wb.save(path)
    return path


def test_normalize_openpyxl(messy_xlsx, tmp_path):
    out = tmp_path / "out"
    r = hedgehog.normalize(messy_xlsx, out)
    assert r.xlsx_path.endswith("prodaja.normalized.xlsx")
    assert json.load(open(r.manifest_path, encoding="utf-8")) == r.manifest
    t = r.table("T1")
    assert t["sheet"] == "T1_prodaja_po_regionima_2024"
    assert t["source"]["sheet"] == "Prodaja" and t["source"]["header_rows"] == 2
    assert t["source"]["title"] == "Prodaja po regionima 2024"
    assert t["group_column"] == "Region"
    by_name = {c["name"]: c for c in t["columns"]}
    assert [c["name"] for c in t["columns"]] == ["Region", "Artikal", "Iznos / Q1", "Iznos / Q2", "Datum", "Udeo", "_src_row"]
    assert by_name["Iznos / Q1"]["dtype"] == "number" and by_name["Iznos / Q1"]["locale"]["decimal"] == ","
    assert by_name["Iznos / Q1"]["original_header"] == [{"text": "Iznos", "cell": "B3"}, {"text": "Q1", "cell": "B4"}]
    assert by_name["Datum"]["dtype"] == "date"
    assert by_name["Udeo"]["dtype"] == "percent"
    assert t["transformations"]["total_rows_removed"] == 3

    wb = openpyxl.load_workbook(r.xlsx_path)
    assert wb.sheetnames == ["README", t["sheet"], "Totals", "Notes"]
    ws = wb[t["sheet"]]
    assert t["sheet"] in ws.tables
    assert ws.tables[t["sheet"]].ref == "A1:G5"
    assert ws.freeze_panes == "A2"
    rows = list(ws.iter_rows(min_row=2, values_only=True))
    assert rows[0] == ("Sever", "Jabuka", 1200.5, 1000, dt.datetime(2024, 3, 15), 0.12, 6)
    assert rows[1][3] == 2000 and rows[1][5] is None
    assert rows[3][2] is None and rows[3][5] == pytest.approx(0.075)
    assert ws["E2"].number_format == "yyyy-mm-dd"
    assert ws["F2"].number_format == "0.00%"

    totals = wb["Totals"]
    header = [c.value for c in totals[1]]
    recs = [dict(zip(header, row)) for row in totals.iter_rows(min_row=2, values_only=True)]
    grand = [x for x in recs if x["label"] == "Sveukupno"]
    assert grand and all(x["table"] == "T1" for x in recs)
    notes = wb["Notes"]
    texts = [row[4] for row in notes.iter_rows(min_row=2, values_only=True)]
    assert "Izvor: interna evidencija" in texts and "Prodaja po regionima 2024" in texts
    assert "Totals" in totals.tables and "Notes" in notes.tables


def test_normalize_pandas(messy_xlsx, tmp_path):
    pd = pytest.importorskip("pandas")
    r = hedgehog.normalize(messy_xlsx, tmp_path, keep_totals=True, csv=True, locale="sr")
    df = r.read("T1")
    assert list(df.columns) == ["Region", "Artikal", "Iznos / Q1", "Iznos / Q2", "Datum", "Udeo", "_src_row", "_row_type"]
    assert pd.api.types.is_float_dtype(df["Iznos / Q1"])
    assert pd.api.types.is_integer_dtype(df["Iznos / Q2"])
    assert pd.api.types.is_datetime64_any_dtype(df["Datum"])
    assert pd.api.types.is_integer_dtype(df["_src_row"])
    assert df["_row_type"].tolist() == ["data", "data", "subtotal", "data", "data", "subtotal", "total"]
    data = df[df["_row_type"] == "data"]
    assert data["Iznos / Q2"].sum() == 1000 + 2000 + 60 + 70
    assert data.groupby("Region")["Iznos / Q2"].sum().to_dict() == {"Jug": 130, "Sever": 3000}
    assert len(r.csv_paths) == 1
    csv = pd.read_csv(r.csv_paths[0])
    assert csv["Iznos / Q1"].iloc[0] == 1200.5
    assert csv["Datum"].iloc[0] == "2024-03-15"
    assert r.manifest["options"]["locale"] == "rs"


def test_normalize_errors(tmp_path):
    with pytest.raises(ValueError):
        hedgehog.normalize(tmp_path / "missing.xlsx", tmp_path)
    with pytest.raises(ValueError):
        hedgehog.normalize(tmp_path / "missing.xlsx", tmp_path, locale="xx")
