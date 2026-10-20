"""Hedgehog: understand messy spreadsheets and hand AI agents clean, traceable data."""

import os as _os

from ._native import Workbook
from ._native import open as _open
from .citations import check_citations
from .facts import facts, facts_json
from .normalize import NormalizeResult, normalize
from .summary import Summary, summarize



def open(path: "str | _os.PathLike[str]") -> Workbook:  # noqa: A001 - mirrors the native name
    """Reads an xlsx / xlsm / xlsb / xls / ods workbook or a CSV / TSV file (format detected from content)
    into a Workbook."""
    return _open(_os.fspath(path))


__all__ = [
    "NormalizeResult",
    "Summary",
    "Workbook",
    "check_citations",
    "facts",
    "facts_json",
    "normalize",
    "open",
    "summarize",
]
