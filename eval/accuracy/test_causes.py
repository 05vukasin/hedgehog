"""Unit tests for the C miss causes (no hedgehog calls): python3 -I -m unittest discover -s eval/accuracy -p 'test_*.py'"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import causes as C  # noqa: E402
from acc import PS  # noqa: E402

TITLE = "Table 3: Percentage reporting impacts on life and limitations, household population with multiple sclerosis"


def table(headers, title=TITLE):
    rows = [["Not at all", 14.4], ["A little bit", 21.7]]
    t = PS.Table("T1", "S", headers, rows, [6, 7], ["data"] * 2, {0}, {1}, ["A", "B"])
    t.title = title
    t.manifest = {"source": {"header_range": "A3:B3", "data_range": "A6:B7", "header_rows": 1}}
    return t


class SourceHeader(unittest.TestCase):
    # HiTab 1108: the title in A1 over 'Impact on life/Limitation | %' (row 3); B1 and B2 are empty
    RAW = {("S", 0, 0): TITLE, ("S", 2, 0): "Impact on life/Limitation", ("S", 2, 1): "%"}

    def test_the_title_is_no_header_of_a_lone_column(self):
        self.assertEqual(C.source_header(self.RAW, "S", table(["Impact on life/Limitation", "%"]), 1), "%")

    def test_a_parent_header_above_is_kept(self):
        raw = dict(self.RAW)
        raw[("S", 1, 1)] = "Women"
        self.assertEqual(C.source_header(raw, "S", table(["Impact on life/Limitation", "%"]), 1), "Women / %")


if __name__ == "__main__":
    unittest.main()
