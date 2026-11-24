"""Hedgehog: understand messy spreadsheets and hand AI agents clean, traceable data."""

import os as _os

from ._native import Workbook
from ._native import open as _open
from ._native import open_bytes as _open_bytes
from .citations import check_citations
from .facts import facts, facts_json
from .normalize import NormalizeResult, normalize
from .session import Session, tool_specs
from .summary import Summary, summarize


def open(path: "str | _os.PathLike[str]") -> Workbook:  # noqa: A001 - mirrors the native name
    """Reads an xlsx / xlsm / xlsb / xls / ods workbook or a CSV / TSV file (format detected from content)
    into a Workbook."""
    return _open(_os.fspath(path))


def open_bytes(data: "bytes | bytearray | memoryview", name: "str | None" = None) -> Workbook:
    """Like `open`, for a workbook or CSV held in memory (an upload, an object from storage).

    `name` is the file name or object key it came from, if known: its extension is used only when
    the content does not tell the format, and its stem names a CSV sheet ("Sheet1" without it)."""
    return _open_bytes(bytes(data), name)


__all__ = [
    "NormalizeResult",
    "Session",
    "Summary",
    "Workbook",
    "check_citations",
    "facts",
    "facts_json",
    "normalize",
    "open",
    "open_bytes",
    "summarize",
    "tool_specs",
]
