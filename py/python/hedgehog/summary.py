"""Workbook summaries: deterministic markdown from facts, or an LLM-written one whose citations are
checked against the workbook."""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any, Dict, List, Optional

from .citations import CitationReport, check_citations
from .facts import WorkbookLike, facts as build_facts, load

if TYPE_CHECKING:  # pragma: no cover
    from .llm.base import ChatClient
    from .llm.types import Usage


@dataclass
class Summary:
    text: str
    facts: Dict[str, Any]
    usage: Optional["Usage"] = None
    source: str = "deterministic"  # or "llm"
    model: Optional[str] = None
    citations: Optional[CitationReport] = None
    # Human-readable problems: bad / unknown citations, numbers without a citation, refusals.
    warnings: List[str] = field(default_factory=list)
    # The deterministic markdown summary (always computed; equals ``text`` when no LLM was used).
    deterministic: str = ""
    # Exactly what was sent to the LLM (compact facts), for auditing the cost.
    payload: Optional[Dict[str, Any]] = None
    # Set when the LLM call failed and ``text`` fell back to the deterministic summary.
    llm_error: Optional[str] = None

    def __str__(self) -> str:
        return self.text


def _fmt(v: Any) -> str:
    if isinstance(v, float):
        return f"{v:,.6g}" if abs(v) < 1e6 else f"{v:,.2f}"
    if isinstance(v, int) and not isinstance(v, bool):
        return f"{v:,}"
    return "—" if v is None else str(v)


def _label(t: Dict[str, Any]) -> str:
    return f' "{t["label"]}"' if t.get("label") not in (None, "") else ""


def markdown_summary(f: Dict[str, Any], max_items: int = 10) -> str:
    sheets = f["sheets"]
    cells = sum(s["cells"] for s in sheets)
    formulas = sum(s["formulas"] for s in sheets)
    out = [
        "# Workbook summary",
        "",
        f"{len(sheets)} sheet(s), {cells:,} filled cells, {formulas:,} formulas.",
        "",
        "| Sheet | Used range | Rows × cols | Cells | Formulas | Merged |",
        "|---|---|---|---|---|---|",
    ]
    for s in sheets:
        out.append(
            f"| {s['name']} | {s['used_range'] or 'empty'} | {s['rows']} × {s['cols']} | {s['cells']} | {s['formulas']} | {s['merges']} |"
        )
    t = f["totals"]
    out += ["", "## Totals", ""]
    if not t["found"]:
        out.append("No total or subtotal cells detected.")
        return "\n".join(out) + "\n"
    out.append(
        f"{t['found']} total(s) found: {t['verified']} verified, {t['mismatched']} mismatched, "
        f"{t['unchecked']} could not be checked."
    )
    items = t["items"]
    groups = [
        ("Mismatched", [i for i in items if i["ok"] is False]),
        ("Verified", [i for i in items if i["ok"] is True]),
        ("Not checked", [i for i in items if i["ok"] is None]),
    ]
    for title, group in groups:
        if not group:
            continue
        out += ["", f"### {title}", ""]
        for i in group[:max_items]:
            src = ", ".join(i["sources"]) or "?"
            if i["ok"] is False:
                diff = (i["value"] or 0) - (i["expected"] or 0)
                if i.get("double_counted"):
                    twice = ", ".join(i["double_counted"])
                    out.append(
                        f"- {i['cell']}{_label(i)}: {_fmt(i['value'])} sums {src}, which also contains the subtotals "
                        f"{twice}, so their rows are counted twice; without them the total is "
                        f"{_fmt(i['expected'])} (overstated by {diff:+,.2f})"
                    )
                else:
                    out.append(
                        f"- {i['cell']}{_label(i)}: {_fmt(i['value'])}, but {src} adds up to {_fmt(i['expected'])} "
                        f"(diff {diff:+,.2f}; {i['reason'].replace('_', ' ')})"
                    )
            elif i["ok"] is True:
                out.append(f"- {i['cell']}{_label(i)} = {_fmt(i['value'])} = sum of {src}")
            else:
                how = i.get("formula") or "typed value"
                out.append(f"- {i['cell']}{_label(i)} ({how}): {i['reason'].replace('_', ' ')}")
        if len(group) > max_items:
            out.append(f"- … {len(group) - max_items} more")
    if t.get("omitted"):
        out += ["", f"({t['omitted']} more totals not listed.)"]
    return "\n".join(out) + "\n"


SYSTEM = """You turn spreadsheet facts into a brief summary for another AI agent. Use only the facts;
never invent numbers, labels or meaning.
Order: (1) one sentence on what the workbook holds (sheets, main tables, header labels); (2) problems
first: each mismatched total with its cell, its value, what it should be and why (name double-counted
subtotal cells); (3) at most 5 key figures. Skip cell/formula counts and empty sheets.
Citations: put the source right after each number in parentheses: 24,517,595.83 (Sheet1!G54). An
expected or recomputed value cites the range it comes from, not the total's cell: 18,981,596.93
(Sheet1!G6:G53 without G9, G14, G19). Quote sheet names with spaces ('Plan 2026'!B7).
Under {words} words, {language}, markdown."""


def _chars(obj: Any) -> int:
    return len(json.dumps(obj, ensure_ascii=False, separators=(",", ":")))


def compact_facts(f: Dict[str, Any], budget_tokens: int = 3000) -> Dict[str, Any]:
    """Facts trimmed to about ``budget_tokens`` (estimated at 3 characters per token), filled by
    priority: sheet list > table values / header rows > mismatched totals > other totals. What did not
    fit is listed under ``"omitted"`` so the model (and the reader) know the payload is partial."""
    budget = budget_tokens * 3
    keys = ("name", "used_range", "rows", "cols", "cells", "formulas")
    t = f["totals"]
    out: Dict[str, Any] = {
        "sheets": [{k: s[k] for k in keys} for s in f["sheets"]],
        "totals": {k: t[k] for k in ("found", "verified", "mismatched", "unchecked")},
    }
    omitted: List[str] = []

    def fits(extra: int = 0) -> bool:
        return _chars(out) + extra + 120 <= budget  # 120: room for the "omitted" note

    # tables: values of small sheets / header rows of large ones, row-trimmed if needed
    for src, dst in zip(f["sheets"], out["sheets"]):
        for key in ("values", "top_rows"):
            block = src.get(key)
            if not block:
                continue
            rows = block["rows"]
            n = len(rows)
            while n > 0 and not fits(_chars({"range": block["range"], "rows": rows[:n]}) + len(key) + 4):
                n = n // 2 if n > 8 else n - 1
            if n == 0:
                omitted.append(f"cell values of sheet {src['name']!r}")
                continue
            from .facts import make_range, parse_range  # noqa: PLC0415

            top, left, bottom, right = parse_range(block["range"])
            rng = make_range(top, left, top + n - 1, right)
            dst[key] = {"range": rng, "rows": rows[:n]}
            if n < len(rows):
                omitted.append(f"rows after {rng} on sheet {src['name']!r}")

    items: List[Dict[str, Any]] = []
    out["totals"]["items"] = items
    groups = [("mismatched", [i for i in t["items"] if i["ok"] is False]), ("other", [i for i in t["items"] if i["ok"] is not False])]
    for label, group in groups:
        for k, it in enumerate(group):
            if not fits(_chars(it) + 1):
                omitted.append(f"{len(group) - k} {label} total(s)")
                break
            items.append(it)
    if t.get("omitted"):
        omitted.append(f"{t['omitted']} total(s) beyond the facts limit")
    if omitted:
        out["omitted"] = omitted
    return out


def summarize(
    wb_or_path: WorkbookLike,
    llm: Optional["ChatClient"] = None,
    *,
    max_tokens: int = 800,
    budget_tokens: int = 3000,
    language: str = "en",
    words: int = 200,
    temperature: Optional[float] = 0.2,
    **chat_options: Any,
) -> Summary:
    """Summary of a workbook. Hedgehog computes everything deterministically; the LLM only phrases it.

    Without ``llm``: a deterministic markdown summary of the facts (sheets, ranges, sizes, totals
    found / verified / mismatched with cell refs) — free, instant, no network.

    With ``llm`` (from :func:`hedgehog.llm.connect`): exactly one call, no tools. The model gets
    :func:`compact_facts` (≤ ``budget_tokens``, partial payloads say what was omitted), a short system
    prompt, ``max_tokens`` 800 and a low temperature. Cited cells are checked against the workbook and
    problems listed in ``warnings``. If the call fails, the deterministic summary is returned with
    ``llm_error`` set instead of raising. ``usage`` has the input/output tokens of the call.
    """
    wb = load(wb_or_path)
    f = build_facts(wb)
    markdown = markdown_summary(f)
    if llm is None:
        return Summary(text=markdown, facts=f, deterministic=markdown)
    payload = compact_facts(f, budget_tokens)
    prompt = "Facts (JSON):\n" + json.dumps(payload, ensure_ascii=False, separators=(",", ":"))
    system = SYSTEM.format(words=words, language=language)
    try:
        try:
            res = llm.chat(prompt, system=system, max_tokens=max_tokens, temperature=temperature, **chat_options)
        except Exception as e:  # noqa: BLE001
            # some models only accept their default temperature; retry once without it
            if temperature is None or "temperature" not in str(e).lower():
                raise
            res = llm.chat(prompt, system=system, max_tokens=max_tokens, **chat_options)
    except Exception as e:  # noqa: BLE001 - a summary must not fail because the LLM did
        return Summary(
            text=markdown, facts=f, deterministic=markdown, payload=payload,
            llm_error=f"{type(e).__name__}: {e}", model=getattr(llm, "model", None),
            warnings=["LLM call failed; deterministic summary returned"],
        )
    warnings: List[str] = []
    if res.stop_reason == "refusal":
        warnings.append(f"model refused: {res.refusal}")
    elif res.stop_reason == "max_tokens":
        warnings.append("summary was cut off at max_tokens")
    if not res.text.strip():
        return Summary(
            text=markdown, facts=f, deterministic=markdown, payload=payload, usage=res.usage,
            source="deterministic", model=res.model, llm_error=f"empty reply (stop_reason={res.stop_reason})",
            warnings=warnings,
        )
    report = check_citations(res.text, wb, payload)
    warnings += report.problems()
    if report.uncited_numbers:
        warnings.append("numbers without a cell citation: " + ", ".join(report.uncited_numbers[:20]))
    return Summary(
        text=res.text,
        facts=f,
        usage=res.usage,
        source="llm",
        model=res.model,
        citations=report,
        warnings=warnings,
        deterministic=markdown,
        payload=payload,
    )
