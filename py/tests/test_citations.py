"""hedgehog.check_citations: pairing cited cells with the numbers they stand for."""

import pytest

import hedgehog

openpyxl = pytest.importorskip("openpyxl")


@pytest.fixture
def book(tmp_path):
    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "Data"
    ws["A1"] = "Item"
    ws["B1"] = "Amount"
    ws["A2"] = "Rent"
    ws["B2"] = 1200
    ws["A3"] = "Food"
    ws["B3"] = 478737
    ws["D7"] = 7582
    ws["T1"] = 5
    other = wb.create_sheet("Other")
    other["A1"] = "x"
    path = tmp_path / "book.xlsx"
    wb.save(path)
    return hedgehog.open(path)


def _by_ref(report):
    return {c.ref: c for c in report.citations}


def test_bare_table_id_is_not_a_cell(book):
    r = hedgehog.check_citations("The totals are in table (T1) and [T12]; the rent is 1,200 (Data!B2).", book)
    assert [c.ref for c in r.citations] == ["Data!B2"]
    assert r.ok


def test_adjacent_number_still_checked(book):
    r = hedgehog.check_citations("Rent was 1,250 (Data!B2).", book)
    c = _by_ref(r)["Data!B2"]
    assert c.status == "mismatch" and c.pairing == "adjacent" and c.written == 1250


def test_number_in_previous_sentence(book):
    text = "The highest value was 478,737. This value is cited from cell Data!D7."
    c = _by_ref(hedgehog.check_citations(text, book))["Data!D7"]
    assert c.status == "mismatch" and c.pairing == "previous_sentence" and c.actual == 7582
    ok = hedgehog.check_citations("The highest value was 478,737. This value is cited from cell Data!B3.", book)
    assert ok.ok and ok.citations[0].pairing == "previous_sentence"


def test_number_in_same_sentence(book):
    r = hedgehog.check_citations("Food cost 478,737 in total, see Data!B3 for the source.", book)
    c = r.citations[0]
    assert c.status == "ok" and c.pairing == "sentence"
    bad = hedgehog.check_citations("Food cost 470,000 in total, see Data!B3 for the source.", book)
    assert bad.citations[0].status == "mismatch"
    assert "paired by sentence" in bad.problems()[0]


def test_markdown_table_neighbour_cell(book):
    text = "| Item | Amount | Cell |\n|---|---|---|\n| Rent | 1,200 | Data!A2 |\n| Food | 478,737 | Data!B3 |\n"
    r = _by_ref(hedgehog.check_citations(text, book))
    assert r["Data!A2"].status == "mismatch" and r["Data!A2"].pairing == "table"  # A2 holds the text "Rent"
    assert r["Data!B3"].status == "ok" and r["Data!B3"].pairing == "table"


def test_no_number_no_comparison(book):
    r = hedgehog.check_citations("The item names are in Data!A2 and Data!A3.", book)
    assert r.ok and all(c.written is None and c.pairing is None for c in r.citations)


def test_years_and_counts_are_not_paired(book):
    r = hedgehog.check_citations("In 2024 there were 3 rows; the rent is in Data!B2.", book)
    assert r.ok and r.citations[0].written is None
