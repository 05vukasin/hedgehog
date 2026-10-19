"""Normalized output for agents with a code sandbox.

``hedgehog.normalize(path, out_dir)`` writes ``<stem>.normalized.xlsx`` (README, one clean typed
sheet per detected table, Totals, Notes) and ``<stem>.manifest.json`` next to it, and returns a
:class:`NormalizeResult`. The source file is never modified. See ``docs/NORMALIZE.md``.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Union

from ._native import normalize as _normalize

PathLike = Union[str, "os.PathLike[str]"]


@dataclass
class NormalizeResult:
    xlsx_path: str
    manifest_path: str
    manifest: Dict[str, Any]
    csv_paths: List[str] = field(default_factory=list)

    @property
    def tables(self) -> List[Dict[str, Any]]:
        """Manifest entries, one per table sheet (``id``, ``sheet``, ``source``, ``columns``…)."""
        return self.manifest["tables"]

    def table(self, key: str) -> Dict[str, Any]:
        """Manifest entry by table id (``"T1"``) or output sheet name (``"T1_sales"``)."""
        for t in self.tables:
            if key in (t["id"], t["sheet"]):
                return t
        raise KeyError(f"no table {key!r}; tables: {[t['sheet'] for t in self.tables]}")

    def read(self, key: str):
        """The table as a pandas DataFrame (needs pandas + openpyxl)."""
        import pandas as pd

        return pd.read_excel(self.xlsx_path, sheet_name=self.table(key)["sheet"])


def normalize(
    path: PathLike,
    out_dir: PathLike,
    *,
    keep_totals: bool = False,
    csv: bool = False,
    locale: Optional[str] = None,
) -> NormalizeResult:
    """Normalize ``path`` into ``out_dir`` (created if missing).

    keep_totals: keep total/subtotal rows in the table sheets, marked in a ``_row_type`` column.
    csv: also write one UTF-8 CSV per table.
    locale: e.g. ``"de-DE"``, ``"en-US"``, ``"sr"``; overrides per-column inference of decimal
        separator and day/month order.
    """
    r = _normalize(os.fspath(path), os.fspath(out_dir), keep_totals, csv, locale)
    return NormalizeResult(
        xlsx_path=r["xlsx_path"],
        manifest_path=r["manifest_path"],
        manifest=json.loads(r["manifest_json"]),
        csv_paths=list(r["csv_paths"]),
    )
