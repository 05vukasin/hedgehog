"""Hedgehog: understand messy spreadsheets and hand AI agents clean, traceable data."""

from ._native import Workbook, open
from .citations import check_citations
from .facts import facts, facts_json
from .normalize import NormalizeResult, normalize
from .summary import Summary, summarize

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
