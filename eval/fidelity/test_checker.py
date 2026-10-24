"""Unit tests of the cell-fidelity checker's comparisons (no hedgehog, no data):
    python3 -I eval/fidelity/test_checker.py"""

import datetime as dt
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from check import HH, _value_blocks_in_notes, flag_cells, joined_label  # noqa: E402
from compare import compare, date_readings, is_missing_token, number_readings, strip_footnote, ws  # noqa: E402
from source import Src  # noqa: E402


def text(v):
    return Src(kind="text", value=v)


class Compare(unittest.TestCase):
    def test_invisible_characters_are_dropped(self):
        # a soft hyphen (Destatis "Industrie\xadproduktion") renders as nothing; hedgehog drops it
        self.assertEqual(ws("Industrie­produktion "), "Industrieproduktion")
        self.assertEqual(compare(text("PROMPTLY ­- SOFTWARE"), "PROMPTLY - SOFTWARE", "text"), ("ok", "text"))
        self.assertEqual(compare(text("PROMPTLY - SOFTWARE"), "PROMPTLY SOFTWARE", "text")[0], "text_changed")

    def test_bracketed_not_applicable_codes_are_missing(self):
        for t in ("(X)", "(x)", "(NA)", "[x]"):
            self.assertTrue(is_missing_token(t), t)
        self.assertFalse(is_missing_token("(Z)"))  # a bound ("less than half the unit"), kept as text

    def test_footnote_tail_keeps_numbers_in_brackets(self):
        self.assertEqual(strip_footnote("EOTT (Co 1195) **"), "EOTT (Co 1195)")
        self.assertEqual(strip_footnote("Total (1)"), "Total")
        self.assertEqual(strip_footnote("Region 2)"), "Region")
        self.assertEqual(strip_footnote("Group (12)"), "Group")

    def test_quarters_with_two_digit_years(self):
        self.assertEqual(date_readings("3Q03")[0], [(dt.date(2003, 7, 1), "quarter")])
        self.assertEqual(date_readings("Q2 '01")[0], [(dt.date(2001, 4, 1), "quarter")])
        self.assertEqual(date_readings("avg 00")[0], [])  # not a quarter, not a month

    def test_percent_with_a_footnote_mark(self):
        self.assertIn((0.1564, "text_percent"), number_readings("15.64% (1)"))
        self.assertEqual(compare(text("15.64% (1)"), "0.1564", "percent"), ("ok", "text_percent"))
        self.assertEqual(compare(text("15.64% (1)"), "0.164", "percent")[0], "value_changed")


class Checker(unittest.TestCase):
    def test_label_split_over_rows(self):
        cells = {("S", 14, 1): text("CTFP"), ("S", 15, 1): text("Por tempo "), ("S", 16, 1): text("indeterminado"),
                 ("S", 17, 1): text("CTFP")}
        for r in (14, 15, 16):
            self.assertTrue(joined_label(cells, "S", r, 1, "CTFP Por tempo indeterminado"), r)
        self.assertFalse(joined_label(cells, "S", 15, 1, "CTFP indeterminado"))  # not contiguous
        self.assertFalse(joined_label(cells, "S", 15, 1, "Por tempo"))  # one cell is no join
        self.assertFalse(joined_label(cells, "S", 15, 1, "Por tempo indeterminado CTFP extra"))

    def test_in_table_range(self):
        hh = HH.__new__(HH)
        hh.ranges = {"S": [(3, 2, 10, 5)]}
        self.assertTrue(hh.in_table("S", 3, 2) and hh.in_table("S", 10, 5))
        self.assertFalse(hh.in_table("S", 11, 2) or hh.in_table("S", 5, 6) or hh.in_table("T", 5, 3))


class Blindspots(unittest.TestCase):
    def test_flag_columns(self):
        hh = HH.__new__(HH)
        hh.ranges = {"S": [(1, 1, 8, 3)]}
        cells = {("S", 1, 1): text("Deal"), ("S", 1, 3): text("Dump")}
        for r in range(2, 9):
            cells[("S", r, 1)] = text(f"D{r}")
            if r % 3:
                cells[("S", r, 3)] = text("X")
        self.assertEqual(flag_cells(cells, hh), {("S", r, 3) for r in range(2, 9) if r % 3})
        # every row marked: a constant, no flag; marks among numbers: missing values
        full = dict(cells)
        for r in range(2, 9):
            full[("S", r, 3)] = text("X")
        self.assertEqual(flag_cells(full, hh), set())
        # all rows marked below the header: a constant, no flag
        allx = {("S", 1, 3): text("TFV")}
        allx.update({("S", r, 3): text("x") for r in range(2, 9)})
        allx.update({("S", r, 1): text(f"D{r}") for r in range(1, 9)})
        self.assertEqual(flag_cells(allx, hh), set())
        # an X in a number format stands for a number (Destatis): a missing-value mark
        fmt = {k: (Src(kind="text", value="X", fmt="#,##0.0") if v.value == "X" else v) for k, v in cells.items()}
        self.assertEqual(flag_cells(fmt, hh), set())
        mixed = dict(cells)
        mixed[("S", 3, 3)] = Src(kind="number", value=4.0)
        self.assertEqual(flag_cells(mixed, hh), set())

    def test_value_blocks_in_notes(self):
        def run(rows):
            res = {"counts": {"ok": 100}, "subs": __import__("collections").Counter(), "examples": __import__("collections").defaultdict(list)}
            items = [("S", r, c, v, f"S!{r}{c}") for r, row in rows.items() for c, v in row.items()]
            _value_blocks_in_notes(res, items)
            return res["counts"].get("misplaced", 0)
        n = lambda x: Src(kind="number", value=x)
        block = {5: {1: text("A"), 2: n(1.0), 3: n(2.0)}, 6: {1: text("B"), 2: n(3.0), 3: n(4.0)}}
        self.assertEqual(run(block), 6)
        self.assertEqual(run({5: {1: text("Note one")}, 6: {1: text("Note two")}}), 0)  # notes
        self.assertEqual(run({5: {1: text("Income"), 2: n(5.0)}, 6: {1: text("Cost"), 2: n(3.0)}}), 0)  # key/value
        d = lambda: Src(kind="date", value=__import__("datetime").date(1997, 10, 1))
        self.assertEqual(run({5: {1: d(), 2: n(2.44)}, 6: {1: d(), 2: n(2.45)}}), 4)  # date | price
        self.assertEqual(run({5: {8: n(0.0), 13: n(0.0)}, 6: {8: n(0.0), 13: n(0.0)}}), 0)  # two lone columns
        self.assertEqual(run({5: block[5], 7: block[6]}), 0)  # not consecutive


if __name__ == "__main__":
    unittest.main()
