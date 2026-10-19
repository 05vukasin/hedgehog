"""Workbook tools for LLM agents, identical across providers: describe_workbook, get_cell, get_range,
find_text, check_totals. Every result is compact JSON with cell addresses."""

from __future__ import annotations

import json
from typing import Any, Callable, Dict, List, Optional, Tuple

from .._native import Workbook
from ..facts import all_totals, facts, parse_range, qualify, _num, _total_item
from .types import Tool

MAX_RANGE_CELLS = 400

TOOLS: List[Tool] = [
    Tool(
        "describe_workbook",
        "Overview of the workbook: sheets with used ranges and sizes, values of small sheets, the first rows "
        "(headers) of large sheets, and every detected total with its verification. Call this first.",
        {"type": "object", "properties": {}, "additionalProperties": False},
    ),
    Tool(
        "get_cell",
        "Value, type and formula of one cell.",
        {
            "type": "object",
            "properties": {
                "sheet": {"type": "string", "description": "Sheet name"},
                "address": {"type": "string", "description": "A1-style address, e.g. D43"},
            },
            "required": ["sheet", "address"],
            "additionalProperties": False,
        },
    ),
    Tool(
        "get_range",
        f"Values of a rectangular range, row by row (null = empty). At most {MAX_RANGE_CELLS} cells per call.",
        {
            "type": "object",
            "properties": {
                "sheet": {"type": "string", "description": "Sheet name"},
                "range": {"type": "string", "description": "A1-style range, e.g. A1:F20"},
            },
            "required": ["sheet", "range"],
            "additionalProperties": False,
        },
    ),
    Tool(
        "find_text",
        "Cells containing the text (case-, accent- and Cyrillic/Latin-insensitive). Use it to locate labels "
        "and headers, then read the numbers next to them.",
        {
            "type": "object",
            "properties": {
                "query": {"type": "string"},
                "sheet": {"type": "string", "description": "Optional: search only this sheet"},
                "limit": {"type": "integer", "description": "Max hits (default 20)"},
            },
            "required": ["query"],
            "additionalProperties": False,
        },
    ),
    Tool(
        "check_totals",
        "Detected total / subtotal cells with their source ranges, recomputed sum and whether they match.",
        {
            "type": "object",
            "properties": {"sheet": {"type": "string", "description": "Optional: only this sheet"}},
            "additionalProperties": False,
        },
    ),
]


class WorkbookTools:
    """Runs the tools against one workbook and remembers which cells were returned."""

    def __init__(self, wb: Workbook) -> None:
        self.wb = wb
        self.cells_seen: List[str] = []
        self._facts: Optional[Dict[str, Any]] = None
        self._handlers: Dict[str, Callable[[Dict[str, Any]], Any]] = {
            "describe_workbook": self.describe_workbook,
            "get_cell": self.get_cell,
            "get_range": self.get_range,
            "find_text": self.find_text,
            "check_totals": self.check_totals,
        }

    @property
    def specs(self) -> List[Tool]:
        return TOOLS

    @property
    def facts(self) -> Dict[str, Any]:
        if self._facts is None:
            self._facts = facts(self.wb)
        return self._facts

    def run(self, name: str, args: Dict[str, Any]) -> Tuple[str, bool]:
        """(JSON result, is_error). Errors are returned to the model, not raised."""
        handler = self._handlers.get(name)
        if handler is None:
            return json.dumps({"error": f"unknown tool {name!r}"}), True
        try:
            result = handler(args if isinstance(args, dict) else {})
        except (KeyError, ValueError, TypeError) as e:
            msg = e.args[0] if e.args else str(e)
            return json.dumps({"error": str(msg)}, ensure_ascii=False), True
        return json.dumps(result, ensure_ascii=False, separators=(",", ":")), False

    # -- tools --------------------------------------------------------------------------------
    def describe_workbook(self, _args: Dict[str, Any]) -> Any:
        return self.facts

    def get_cell(self, args: Dict[str, Any]) -> Any:
        c = self.wb.get_cell(args.get("sheet"), str(args["address"]))
        ref = qualify(c["sheet"], c["address"])
        self.cells_seen.append(ref)
        out = {"cell": ref, "value": _num(c["value"]), "type": c["type"]}
        if c["formula"]:
            out["formula"] = c["formula"]
        return out

    def get_range(self, args: Dict[str, Any]) -> Any:
        rng = str(args["range"])
        try:
            r = self.wb.get_range(args.get("sheet"), rng, MAX_RANGE_CELLS)
        except ValueError as e:
            raise ValueError(f"{e}; request a smaller range (≤ {MAX_RANGE_CELLS} cells)") from None
        self.cells_seen.append(qualify(r["sheet"], r["range"]))
        top, left, _, _ = parse_range(r["range"])
        return {"range": qualify(r["sheet"], r["range"]), "first_row": top + 1, "rows": [[_num(v) for v in row] for row in r["rows"]]}

    def find_text(self, args: Dict[str, Any]) -> Any:
        limit = int(args.get("limit") or 20)
        hits = self.wb.find_text(str(args["query"]), args.get("sheet"), max(1, min(limit, 100)))
        return [{"cell": qualify(h["sheet"], h["address"]), "value": _num(h["value"])} for h in hits]

    def check_totals(self, args: Dict[str, Any]) -> Any:
        sheet = args.get("sheet")
        items = [_total_item(t) for t in all_totals(self.wb, sheet)]
        return {"found": len(items), "items": items[:100]}
