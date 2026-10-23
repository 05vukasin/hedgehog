"""Unit tests for score.py / common.py (no hedgehog calls): python3 -I -m unittest discover -s eval/public -p 'test_*.py'"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import score as S  # noqa: E402
from common import clock_fraction, typed_number  # noqa: E402


def table(headers, rows, numeric=()):
    return S.Table("T1", "Sheet1", headers, rows, list(range(2, 2 + len(rows))), ["data"] * len(rows), {0}, set(numeric),
                   [None] * len(headers))


class Clock(unittest.TestCase):
    def test_clock_times_are_day_fractions(self):
        self.assertAlmostEqual(clock_fraction("09:30:00"), 9.5 / 24)
        self.assertAlmostEqual(clock_fraction("9:30"), 9.5 / 24)
        self.assertAlmostEqual(clock_fraction("00:01:23"), 83 / 86400)
        self.assertAlmostEqual(clock_fraction("36:00:00"), 1.5)  # elapsed time [h]:mm:ss

    def test_not_clock_times(self):
        for s in ("1:2", "12:3", "2015-05-04 00:00:00", "30 minutes", "48:00", "9:60", "", None, 0.5):
            self.assertIsNone(clock_fraction(s), s)

    def test_typed_number(self):
        self.assertEqual(typed_number("1,234"), 1234.0)
        self.assertAlmostEqual(typed_number("18:00:00"), 0.75)
        self.assertIsNone(typed_number("Thursday"))

    def test_numeric_gold_matches_a_clock_cell(self):
        self.assertTrue(S.Atom("0.395833", "number").matches_value("09:30:00"))
        self.assertFalse(S.Atom("570", "number").matches_value("09:30:00"))  # minutes are no stored value
        self.assertFalse(S.Atom("9", "number").matches_value("09:30:00"))


class Operands(unittest.TestCase):
    def test_clock_column_is_an_operand_column_but_not_clean(self):
        t = table(["Driver", "Driving Time"], [["a", "00:01:23"], ["b", "04:59:29"]])
        self.assertEqual(S.operands_by_question([t], {"driving", "time"}), (1, 0))

    def test_text_and_ratio_columns_stay_out(self):
        t = table(["Driver", "Driving Time", "Water ratio"], [["a", "fast", "1:2"], ["b", "slow", "1:3"]])
        self.assertEqual(S.operands_by_question([t], {"driving", "time", "water", "ratio"}), (0, 0))

    def test_numeric_column_unchanged(self):
        t = table(["Driver", "Driving hours"], [["a", 1.5], ["b", 2.0]], numeric={1})
        self.assertEqual(S.operands_by_question([t], {"driving"}), (1, 1))


class RowLabels(unittest.TestCase):
    def test_numeric_labels_read_as_text(self):
        t = table(["Age", "Year", "Mean"], [["Aged 2 to 8", 2015.0, 104]], numeric={2})
        t.label_cols = {0, 1}
        self.assertEqual(t.labels(0, 2), ["Aged 2 to 8", "2015"])

    def test_year_column_in_the_stub_is_a_label(self):
        cols = [{"role": "group", "dtype": "text"}, {"role": "data", "dtype": "integer", "min": 2004, "max": 2015},
                {"role": "data", "dtype": "integer", "min": 36, "max": 128}, {"role": "data", "dtype": "integer", "min": 2000, "max": 2010}]
        self.assertEqual(S._year_stub_cols(cols, [0, 1, 2, 3], {0}), {1})  # a year column after a measure stays a measure


class CatalogNumbers(unittest.TestCase):
    CAT = ("T1  'x' ← 'A'!A1:F3 · 3 rows\n    columns (6): a b c d e f\n    all 3 rows (CSV):\n    a,b,c,d,e,f\n"
           "    1,M435,20,-3.5,\"1,234\",13:00:00\n    2,\"Smith, J\",\"-12,345.67\",0.25,7%,x\n"
           "T2  'y' ← 'A'!A9:B9 · 1 row\n    first 1 rows (CSV, as in T2_all; rows with _row_type total are not in T2):\n"
           "    k,v\n    total,4242\nNOTES 3 cells; e.g. A!A1: \"x\"")

    def nums(self):
        return S.catalog_numbers(self.CAT)

    def test_unquoted_after_a_comma(self):
        self.assertIn(20.0, self.nums())
        self.assertNotIn(20.0, S.text_numbers(self.CAT))  # the free-text pattern skips it

    def test_negative_and_decimal(self):
        self.assertIn(-3.5, self.nums())
        self.assertIn(0.25, self.nums())

    def test_quoted_thousands_separator(self):
        self.assertIn(1234.0, self.nums())
        self.assertIn(-12345.67, self.nums())
        self.assertNotIn(345.67, self.nums())

    def test_quoted_text_with_comma_is_no_number(self):
        self.assertNotIn(-12.0, self.nums())

    def test_percent_and_clock(self):
        self.assertIn(7.0, self.nums())
        self.assertTrue(any(abs(x - 13 / 24) < 1e-9 for x in self.nums()))

    def test_second_block_with_a_qualified_csv_line(self):
        self.assertIn(4242.0, self.nums())

    def test_lines_outside_csv_blocks_are_free_text(self):
        self.assertIn(3.0, self.nums())  # "NOTES 3 cells"


if __name__ == "__main__":
    unittest.main()
