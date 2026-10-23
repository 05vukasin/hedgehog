"""Unit tests for the M invariants (no hedgehog calls): python3 -I -m unittest discover -s eval/accuracy -p 'test_*.py'

Each false-positive class found in the 2026-10-24 sample review (eval/accuracy/RESULTS.md, "M classes") has a
test that it is no longer flagged, next to a true positive of the same class that still is.
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import invariants as I  # noqa: E402
from acc import PS  # noqa: E402

NUM = "number"


def letter(c: int) -> str:
    s = ""
    c += 1
    while c:
        c, r = divmod(c - 1, 26)
        s = chr(65 + r) + s
    return s


def table(headers, rows, dtypes, label_cols=(0,), first_row=5, first_col=0, header_range=None, colmeta=None, sheet="S"):
    letters = [letter(first_col + k) for k in range(len(headers))]
    t = PS.Table("T1", sheet, headers, rows, list(range(first_row, first_row + len(rows))), ["data"] * len(rows),
                 set(label_cols), {k for k, d in enumerate(dtypes) if d in ("integer", NUM)}, letters)
    t.dtypes = list(dtypes)
    t.colmeta = colmeta or [{} for _ in headers]
    hr = header_range or f"{letters[0]}{first_row - 1}:{letters[-1]}{first_row - 1}"
    t.manifest = {"source": {"header_range": hr, "data_range": f"{letters[0]}{first_row}:{letters[-1]}{first_row + len(rows) - 1}"}}
    return t


def kinds(t, raw=None) -> list[str]:
    return [v["inv"] for v in I.check_table(t, raw or {})]


class M3MeasuresMixed(unittest.TestCase):
    def test_stacked_number_and_percentage_blocks(self):
        rows = [["Number of people", 5546400.0], ["Number of people", 5571600.0],
                ["Percentage of people", 98.1], ["Percentage of people", 98.6]]
        self.assertIn("M3 measures mixed", kinds(table(["group", "Food store"], rows, ["text", NUM])))

    def test_percent_sign_in_a_product_name_is_no_rate_row(self):
        # economy-table126: 'Production %' is a product; its row holds dollar amounts (Quantity 150, Amount 262.5)
        rows = [["Production %", 150.0, 262.5, 60.375], ["Production %", 150.0, 262.5, 60.375],
                ["Flow Back", 400.0, 1200.0, 360.0], ["Flow Back", 200.0, 600.0, 120.0], ["Flow Back", 300.0, 900.0, 240.0]]
        t = table(["Product", "Quantity", "Amount Billed", "Employee Take"], rows, ["text", NUM, NUM, NUM])
        self.assertNotIn("M3 measures mixed", kinds(t))

    def test_rank_named_rate_is_no_rate_row(self):
        rows = [["Leading Rate/Corporal", 19180.0], ["Leading Rate/Corporal > Asian", 50.0],
                ["Leading Rate/Corporal > Mixed", 25.0], ["Officers", 5710.0], ["Other Ranks", 28350.0]]
        self.assertNotIn("M3 measures mixed", kinds(table(["Rank", "2012-04-01 / Number"], rows, ["text", NUM])))


class M4ExcelSerial(unittest.TestCase):
    def test_date_column_of_serials(self):
        t = table(["Device", "Completion Date"], [["Printer", 44377], ["Device 1", 44387]], ["text", "integer"])
        self.assertIn("M4 Excel serial as number", kinds(t))

    def test_full_time_dollars_is_no_date(self):
        t = table(["Group", "Employed full-year, full-time / dollars"], [["a", 49380], ["b", 52011]], ["text", "integer"])
        self.assertNotIn("M4 Excel serial as number", kinds(t))


class M5NumberAsHeader(unittest.TestCase):
    def test_a_value_as_header(self):
        t = table(["Project", "883"], [["B", 801.0], ["C", 747.0]], ["text", NUM])
        self.assertIn("M5 number as header", kinds(t))

    def test_rate_parameter_under_a_text_header(self):
        t = table(["Month", "Profit / 0.25", "Commission / 0.1"], [["Nov", 12.0, 3.0], ["Dec", 15.0, 4.0]], ["text", NUM, NUM])
        self.assertNotIn("M5 number as header", kinds(t))


class M5DuplicateHeaders(unittest.TestCase):
    def colmeta(self, cells):
        return [{"original_header": [{"cell": c, "text": "x"}]} if c else {} for c in cells]

    def test_lost_parent_group(self):
        # row 3: groups 'Main Income' (B) and 'Income Tax' (D); row 4: Month / This Year under each
        raw = {("S", 2, 1): "Main Income", ("S", 2, 3): "Income Tax", ("S", 3, 1): "Month", ("S", 3, 2): "This Year",
               ("S", 3, 3): "Month", ("S", 3, 4): "This Year"}
        t = table(["Label", "Month", "This Year", "Month (2)", "This Year (2)"], [["r", 1.0, 2.0, 3.0, 4.0]] * 2,
                  ["text", NUM, NUM, NUM, NUM], header_range="A3:E4", colmeta=self.colmeta([None, "B4", "C4", "D4", "E4"]))
        self.assertIn("M5 duplicate headers", kinds(t, raw))

    def test_unit_line_above_the_stub_is_no_parent(self):
        # 163_81_tab2: '(Thousands of current dollars)' in A2 above a side-by-side state block
        raw = {("S", 1, 0): "(Thousands of current dollars)", ("S", 2, 0): "State", ("S", 2, 1): "All R&D",
               ("S", 2, 2): "State", ("S", 2, 3): "All R&D"}
        t = table(["State", "All R&D", "State (2)", "All R&D (2)"], [["Alabama", 1.0, "Missouri", 2.0]] * 2,
                  ["text", NUM, "text", NUM], first_row=4, header_range="A3:D3", colmeta=self.colmeta(["A3", "B3", "C3", "D3"]))
        self.assertNotIn("M5 duplicate headers", kinds(t, raw))

    def test_key_value_row_is_no_parent(self):
        # a key/value line above the header ('Gross Floor Area' 211675.9 m2 'Floor Area Ratio' 1.87)
        raw = {("S", 3, 1): "Gross Floor Area", ("S", 3, 2): "211675.9", ("S", 3, 3): "Floor Area Ratio",
               ("S", 4, 1): "Road", ("S", 4, 3): "Asphalt"}
        t = table(["Label", "Road", "Road (2)", "Asphalt", "Asphalt (2)"], [["r", 1.0, None, 2.0, None]] * 2,
                  ["text", NUM, NUM, NUM, NUM], first_row=6, header_range="A5:E5", colmeta=self.colmeta([None, "B5", "C5", "D5", "E5"]))
        self.assertNotIn("M5 duplicate headers", kinds(t, raw))

    def test_unit_row_taken_as_header_under_named_columns(self):
        # 8泵类机械: row 6 '元 元 元' was taken as the header; row 3 names the columns (折旧费, 大修理费, 经常修理费)
        raw = {("S", 2, 1): "折旧费", ("S", 2, 2): "大修理费", ("S", 2, 3): "经常修理费", ("S", 4, 1): "定额",
               ("S", 4, 4): "5.65", ("S", 5, 1): "元", ("S", 5, 2): "元", ("S", 5, 3): "元"}
        t = table(["No", "元", "元 (2)", "元 (3)", "kg"], [["8-1", 2.66, 1.6, 3.85, 23.0]] * 2,
                  ["text", NUM, NUM, NUM, NUM], first_row=7, header_range="A6:E6", colmeta=self.colmeta([None, "B6", "C6", "D6", "E6"]))
        self.assertIn("M5 duplicate headers", kinds(t, raw))

    def test_form_labels_above_are_no_names(self):
        raw = {("S", 1, 1): "Date: ________", ("S", 1, 2): "Inv. By: ________"}
        t = table(["Item", "UP", "UP (2)"], [["a", 1.0, 2.0]] * 2, ["text", NUM, NUM], first_row=4, header_range="A3:C3",
                  colmeta=self.colmeta([None, "B3", "C3"]))
        self.assertNotIn("M5 duplicate headers", kinds(t, raw))


class M5RepeatedHeaderNames(unittest.TestCase):
    def test_lost_sub_header_under_a_spanning_header(self):
        # 1521: 'Total' spans B..E over '000 / % / 95% CI from / to' - the sub-header row was lost
        raw = {("S", 2, 1): "Total"}
        t = table(["Label", "Total", "Total (2)", "Total (3)", "Total (4)"], [["a", 1.0, 2.0, 3.0, 4.0]] * 3,
                  ["text", NUM, NUM, NUM, NUM], first_row=4, header_range="A3:E3")
        self.assertIn("M5 repeated header names", kinds(t, raw))

    def test_placeholder_row_taken_as_header(self):
        raw = {("S", 15, 1): "F", ("S", 15, 2): "F", ("S", 15, 3): "F"}
        t = table(["Label", "F", "F (2)", "F (3)"], [["a", 1.2, 6.8, 570.0]] * 3, ["text", NUM, NUM, NUM],
                  first_row=17, header_range="A16:D16")
        self.assertIn("M5 repeated header names", kinds(t, raw))

    def test_source_repeats_the_header(self):
        # economy-table27: B6:D6 are all 'Time period' in the source
        raw = {("S", 5, 1): "Time period", ("S", 5, 2): "Time period", ("S", 5, 3): "Time period"}
        t = table(["group", "Time period", "Time period (2)", "Time period (3)"], [["g", "x", "y", "z"]] * 3,
                  ["text", "text", "text", "text"], first_row=7, header_range="A6:D6")
        self.assertNotIn("M5 repeated header names", kinds(t, raw))

    def test_spanning_header_over_text_columns(self):
        # sport-table06: 'Actual Results' (merged I1:L2) over team / score / score / team
        raw = {("S", 0, 0): "Actual Results"}
        t = table(["Actual Results", "Actual Results (2)", "Actual Results (3)", "Actual Results (4)"],
                  [["USA", 0, 2, "Colombia"]] * 3, ["text", "integer", "integer", "text"], first_row=3, header_range="A1:D2")
        self.assertNotIn("M5 repeated header names", kinds(t, raw))

    def test_report_key_value_line_taken_as_header(self):
        # labor-table39: the report line 'Time Frame:' / '01/26/2012 06:10:22 PM' became the header
        raw = {("S", 1, 0): "Time Frame:", ("S", 1, 3): "01/26/2012 06:10:22 PM"}
        t = table(["Time Frame:", "Time Frame: (2)", "Time Frame: (3)", "01/26/2012 06:10:22 PM", "01/26/2012 06:10:22 PM (2)",
                   "01/26/2012 06:10:22 PM (3)"], [["Sort By:", None, None, "User Name", None, None]] * 3,
                  ["text"] * 6, first_row=3, header_range="A2:F2")
        self.assertEqual(kinds(t, raw).count("M5 repeated header names"), 2)

    def test_real_header_row_just_below(self):
        # 生产制造 (531): a summary line ('订单数量' over 1000) was taken as the header; row 4 is the real one
        raw = {("S", 1, 0): "订单数量", ("S", 2, 0): "1000", ("S", 3, 0): "生量已产", ("S", 3, 1): "(天)"}
        t = table(["订单数量", "订单数量 (2)", "订单数量 (3)"], [["1000", None, None]], ["text", "text", "text"],
                  first_row=3, header_range="A2:C2")
        self.assertIn("M5 repeated header names", kinds(t, raw))


if __name__ == "__main__":
    unittest.main()
