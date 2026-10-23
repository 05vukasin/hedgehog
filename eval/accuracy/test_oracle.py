"""Unit tests for the C oracle's grounding (no hedgehog calls): python3 -I -m unittest discover -s eval/accuracy -p 'test_*.py'"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle as O  # noqa: E402
from acc import PS  # noqa: E402

Q = "in 2015, what was the difference between the percent of women with a university degree and women who are high school graduates?"


def table(with_parent: bool):
    hd = ["group"] + (["group_parent"] if with_parent else []) + ["col_A", "2015 / percent"]
    rows = [["Women"] + (["Total, all education levels"] if with_parent else []) + [lab, v]
            for lab, v in (("High school graduate", 69.3), ("University degree", 83.1), ("College", 90.0))]
    t = PS.Table("T1", "S", hd, rows, [5, 6, 7], ["data"] * 3, set(range(len(hd) - 1)), {len(hd) - 1},
                 ["A"] + ([None] if with_parent else []) + ["B", "C"])
    t.parent_cols = {1} if with_parent else set()
    t.dtypes = ["text"] * (len(hd) - 1) + ["number"]
    t.colmeta = [{"role": "group"}] + [{}] * (len(hd) - 1)
    t.manifest, t.file = {}, "x.xlsx"
    return t


class ParentColumns(unittest.TestCase):
    def test_unnamed_ancestor_column_does_not_block_a_witness(self):
        # branch labels adds '<label>_parent' / 'group_parent' columns ('Total, all education levels'); the
        # question names only the leaf labels: the difference must stay tightly reachable
        for wp in (False, True):
            r = O.Programs([table(wp)], O.Question(Q)).search({"text": "13.8", "kind": "number"}, Q)
            self.assertGreater(r["tight"], 0, wp)


if __name__ == "__main__":
    unittest.main()
