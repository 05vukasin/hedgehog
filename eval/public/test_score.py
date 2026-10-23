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


if __name__ == "__main__":
    unittest.main()
