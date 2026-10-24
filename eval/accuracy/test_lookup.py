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

    def test_data_column_with_a_lost_header(self):
        t = table(["col_A", "January / Actual", "January / Budget"], ROWS,
                  colmeta=[{"original_header": [{"text": "Product", "cell": "A6"}]}, {}, {}])
        self.assertFalse(L.unnamed_stub(t, 0, RAW))


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


if __name__ == "__main__":
    unittest.main()
