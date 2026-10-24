"""Unit tests for the L scorer (no hedgehog calls): python3 -I -m unittest discover -s eval/accuracy -p 'test_*.py'"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import lookup as L  # noqa: E402
from acc import PS  # noqa: E402


def table(headers, rows, rng="A5:C8", data="A7:C8", colmeta=None):
    """A table from A5 with source columns A.. and row labels in the text columns."""
    letters = [chr(65 + k) for k in range(len(headers))]
    label = {k for k, r in enumerate(zip(*rows)) if all(isinstance(v, str) for v in r)}
    t = PS.Table("T1", "S", headers, rows, list(range(6, 6 + len(rows))), ["data"] * len(rows), label,
                 set(range(len(headers))) - label, letters)
    t.colmeta = colmeta or [{} for _ in headers]
    t.manifest = {"source": {"range": rng, "data_range": data}}
    return t


ROWS = [["Product 1", 23432.0, 31233.0], ["Product 2", 49887.1, 44737.9]]
# the source: a two-row header (row 5 'January', row 6 'Actual | Budget') over an empty stub cell
RAW = {("S", 4, 1): "January", ("S", 5, 1): "Actual", ("S", 5, 2): "Budget",
       ("S", 6, 0): "Product 1", ("S", 7, 0): "Product 2"}


class UnnamedStub(unittest.TestCase):
    def test_stub_the_source_leaves_unnamed(self):
        # economy-table56: the products' column has no header cell in the source; 'col_A' loses nothing
        t = table(["col_A", "January / Actual", "January / Budget"], ROWS)
        self.assertTrue(L.unnamed_stub(t, 0, RAW))

    def test_header_text_in_the_source_is_lost(self):
        raw = dict(RAW)
        raw[("S", 5, 0)] = "Product"
        t = table(["col_A", "January / Actual", "January / Budget"], ROWS)
        self.assertFalse(L.unnamed_stub(t, 0, raw))

    def test_no_header_rows_detected(self):
        # WorkplaceGeneral(45): the header row 'Project Name | ...' kept as a data row; the table has no header rows
        t = table(["col_A", "col_B", "col_C"], [["Project Name", "Revenue", "Cost"]] + ROWS, rng="A6:C8", data="A6:C8")
        self.assertFalse(L.unnamed_stub(t, 0, RAW))

    def test_named_column_left_of_it(self):
        # a title taken as the header ('NOW Magazine' over job ids): the stub is not where the header says
        t = table(["NOW Magazine", "col_B", "2014-07-24"], [["EM2058", "Corona Sunsets", 1.0], ["EM2059", "Other", 2.0]])
        self.assertFalse(L.unnamed_stub(t, 1, RAW))

    def test_number_columns_left_of_it(self):
        # society-table37: 'Int# | LiST# | (blank) | (blank) | Newborn Lives Saved', the intervention names in E
        # have no header cell in the source; item numbers to their left do not make it a misplaced header
        rows = [[0.0, 0.0, "Family Planning", "Contraceptive Use", 3.0], [9.0, 14.0, "Antenatal Care", "TT", 80.0]]
        t = table(["Int#", "LiST#", "col_C", "col_D", "Newborn Lives Saved"], rows, rng="A5:E8", data="A6:E8")
        raw = {("S", 4, 0): "Int#", ("S", 4, 1): "LiST#", ("S", 4, 4): "Newborn Lives Saved"}
        self.assertTrue(L.unnamed_stub(t, 3, raw))
        e = table(["Selected", "Int#", "col_C", "col_D", "Newborn Lives Saved"], [[None] + r[1:] for r in rows],
                  rng="A5:E8", data="A6:E8")
        self.assertTrue(L.unnamed_stub(e, 3, raw), "an empty named column to its left")
        f = table(["Selected", "Int#", "col_C", "col_D", "Newborn Lives Saved"], [["x"] + r[1:] for r in rows],
                  rng="A5:E8", data="A6:E8")
        f.label_cols = {0, 2, 3}
        self.assertTrue(L.unnamed_stub(f, 3, raw), "a flag column ('x' marks, placement keeps them) to its left")
        g = table(["Selected", "Int#", "col_C", "col_D", "Newborn Lives Saved"], [["Planned"] + r[1:] for r in rows[:1]]
                  + [["Done"] + rows[1][1:]], rng="A5:E8", data="A6:E8")
        g.label_cols = {0, 2, 3}
        self.assertFalse(L.unnamed_stub(g, 3, raw), "a named text column of labels to its left")
        raw[("S", 4, 3)] = "Intervention"
        self.assertFalse(L.unnamed_stub(t, 3, raw))

    def test_data_column_with_a_lost_header(self):
        t = table(["col_A", "January / Actual", "January / Budget"], ROWS,
                  colmeta=[{"original_header": [{"text": "Product", "cell": "A6"}]}, {}, {}])
        self.assertFalse(L.unnamed_stub(t, 0, RAW))


class YearKey(unittest.TestCase):
    # society-table61: 'Period | Total bankruptcies', the years are the rows' own labels
    T = table(["Period", "Total bankruptcies"], [[2010.0, 59173.0], [2011.0, 41928.0], [2012.0, 31775.0]])

    def setUp(self):
        self.T.label_cols = {0}

    def test_year_of_a_year_stub(self):
        self.assertTrue(L.year_key([self.T], PS.Atom("2010", "number"), {"year", "highest", "bankruptcies"}))

    def test_question_without_year(self):
        self.assertFalse(L.year_key([self.T], PS.Atom("2010", "number"), {"highest", "bankruptcies"}))

    def test_measure_is_no_year_key(self):
        t = table(["Period", "Total"], [[2010.0, 2011.0], [2011.0, 1990.0], [2012.0, 1995.0]])
        t.label_cols = {0}
        self.assertFalse(L.year_key([t], PS.Atom("1990", "number"), {"year"}))


class HitabRoots(unittest.TestCase):
    def tbl(self):
        rows = [["All businesses", 17.2, 21.1], ["Businesses majority-owned by visible minorities", 13.7, 24.7]]
        return table(["col_A", "Increase", "Decline 40% or more"], rows)

    def test_total_worded_row_kept_as_data_is_a_root(self):
        # HiTab 128.xlsx: the gold left path is rooted at the 'All businesses' row hedgehog keeps as data
        cell = {"left_path": ["all businesses", "businesses majority-owned by visible minorities"],
                "top_path": ["decline 40% or more"]}
        self.assertEqual(L.hitab_paths(self.tbl(), 1, 2, cell), (True, True, True))

    def test_other_root_is_still_checked(self):
        cell = {"left_path": ["population centre sizes", "businesses majority-owned by visible minorities"],
                "top_path": ["decline 40% or more"]}
        self.assertEqual(L.hitab_paths(self.tbl(), 1, 2, cell)[0], False)

    def test_only_the_root(self):
        # a total-worded row is no stand-in for an inner element of the path
        cell = {"left_path": ["women", "all businesses", "businesses majority-owned by visible minorities"],
                "top_path": ["decline 40% or more"]}
        self.assertEqual(L.hitab_paths(self.tbl(), 1, 2, cell)[0], False)


class HitabStubKeys(unittest.TestCase):
    def test_rank_column_keys_the_row(self):
        # NSF 168_83 tab4: HiTab's left path is the rank ('23'), a number column left of the institutions
        t = table(["Rank", "Institution", "2015"], [[22, "Ohio State U.", 610.0], [23, "New York U.", 602.0]])
        self.assertEqual(L.hitab_paths(t, 1, 2, {"left_path": ["23"], "top_path": ["2015"]}), (True, True, True))
        self.assertFalse(L.hitab_paths(t, 1, 2, {"left_path": ["22"], "top_path": ["2015"]})[0])

    def test_numeric_first_column(self):
        # statcan/1425: 'Growth rate' 0.20 keys the row; a repeated value keys nothing
        t = table(["Growth rate", "Canadian-born-owned / percent"], [[0.1, 70.2], [0.2, 78.7]])
        self.assertTrue(L.hitab_paths(t, 1, 1, {"left_path": ["0.20"], "top_path": ["canadian-born-owned"]})[0])
        t = table(["Growth rate", "Canadian-born-owned / percent"], [[0.2, 70.2], [0.2, 78.7]])
        self.assertFalse(L.hitab_paths(t, 1, 1, {"left_path": ["0.20"], "top_path": ["canadian-born-owned"]})[0])

    def test_measure_column_is_no_key(self):
        # a number column right of the label column is a measure, not a key
        t = table(["Institution", "2014", "2015"], [["Ohio State U.", 23.0, 610.0], ["New York U.", 524.0, 602.0]])
        self.assertFalse(L.hitab_paths(t, 1, 2, {"left_path": ["524"], "top_path": ["2015"]})[0])

    def test_revised_year(self):
        t = table(["Year", "Total crime / rate"], [[2014, 5061.0], [2015, 5210.0]])
        self.assertTrue(L.hitab_paths(t, 1, 1, {"left_path": ["2015r"], "top_path": ["total crime", "rate"]})[0])


class HitabColumns(unittest.TestCase):
    def test_gold_element_with_a_slash(self):
        # statcan/1220: the header cell 'Unemployed / Job vacancies' over 'ratio'
        t = table(["col_A", "Unemployed / Job vacancies / ratio"], [["Canada", 2.4], ["Quebec", 2.1]])
        cell = {"left_path": ["canada"], "top_path": ["unemployed / job vacancies", "ratio"]}
        self.assertEqual(L.hitab_paths(t, 0, 1, cell), (True, True, True))
        cell = {"left_path": ["canada"], "top_path": ["unemployed / job vacancies", "rate"]}
        self.assertFalse(L.hitab_paths(t, 0, 1, cell)[2])
        t2 = table(["col_A", "Unemployed / ratio"], [["Canada", 2.4], ["Quebec", 2.1]])
        cell = {"left_path": ["canada"], "top_path": ["unemployed / job vacancies", "ratio"]}
        self.assertFalse(L.hitab_paths(t2, 0, 1, cell)[2])


if __name__ == "__main__":
    unittest.main()
