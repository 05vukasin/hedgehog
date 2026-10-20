"""LLM test loop: ``ask`` answers one question about a workbook through the workbook tools (a manual
tool loop, identical for every provider), and ``bench`` compares models on question sets."""

from __future__ import annotations

import json
import re
import time
from dataclasses import dataclass, field
from typing import Any, Dict, Iterable, List, Mapping, Optional, Sequence, Union

from ..citations import CitationReport, check_citations, parse_number
from ..facts import WorkbookLike, load
from .base import ChatClient
from .tools import WorkbookTools
from .types import Message, ToolResult, Usage, tool_results, user

SYSTEM = """You answer questions about one spreadsheet workbook using only the tools provided.
Rules:
- Never guess or invent values. Read every number you use from the tools.
- Cite every number with its cell, like Sales!D43 or 'Sheet name'!B7 (ranges: Sales!B2:B9).
- Totals with status "mismatch" do not add up; say so if they matter. Status "unknown" means not
  verified (not an error): do not count those as wrong.
- If the workbook cannot answer the question, say that instead of guessing.
- End with one last line: `ANSWER: <value>` — just the number (no units, no thousands separators) or a short text.
Workbook sheets: {sheets}"""

LIMIT_NOTE = "Tool-call limit reached. Answer now from what you have, ending with the ANSWER line."


@dataclass
class Answer:
    text: str
    final: Optional[str]  # the value after "ANSWER:", if present
    tool_calls_made: List[Dict[str, Any]]
    usage: Usage
    cited_cells: List[str]
    citations: CitationReport
    stop_reason: str
    turns: int
    seconds: float
    model: Optional[str] = None
    refusal: Optional[Dict[str, Any]] = None
    messages: List[Message] = field(default_factory=list, repr=False)


_ANSWER_RE = re.compile(r"^\s*\**ANSWER\**\s*:\s*(.+?)\s*$", re.IGNORECASE | re.MULTILINE)


def final_answer(text: str) -> Optional[str]:
    hits = _ANSWER_RE.findall(text or "")
    return hits[-1].strip().strip("`*") if hits else None


def ask(
    wb: WorkbookLike,
    question: str,
    client: ChatClient,
    *,
    max_turns: int = 8,
    max_tokens: Optional[int] = None,
    temperature: Optional[float] = None,
    system: Optional[str] = None,
    **chat_options: Any,
) -> Answer:
    """Runs the tool loop until the model answers without tool calls (or ``max_turns`` model calls)."""
    book = load(wb)
    tools = WorkbookTools(book)
    sys_prompt = system or SYSTEM.format(sheets=", ".join(repr(n) for n in book.sheet_names))
    messages: List[Message] = [user(question)]
    usage = Usage()
    calls_made: List[Dict[str, Any]] = []
    started = time.monotonic()
    result = None
    turns = 0
    stop = "end_turn"
    for turn in range(max_turns):
        turns = turn + 1
        result = client.chat(
            messages, system=sys_prompt, tools=tools.specs, max_tokens=max_tokens, temperature=temperature, **chat_options
        )
        usage = usage + result.usage
        messages.append(result.to_message())
        stop = result.stop_reason
        if not result.tool_calls or stop == "refusal":
            break
        outs = []
        for tc in result.tool_calls:
            content, is_err = tools.run(tc.name, tc.arguments)
            calls_made.append({"name": tc.name, "arguments": tc.arguments, "error": is_err})
            outs.append(ToolResult(tc.id, content, is_err))
        last = turn == max_turns - 2
        messages.append(tool_results(outs, text=LIMIT_NOTE if last else ""))
    else:
        stop = "max_turns"
    text = result.text if result is not None else ""
    report = check_citations(text, book)
    return Answer(
        text=text,
        final=final_answer(text),
        tool_calls_made=calls_made,
        usage=usage,
        cited_cells=report.cited,
        citations=report,
        stop_reason=stop,
        turns=turns,
        seconds=round(time.monotonic() - started, 3),
        model=result.model if result is not None else client.model,
        refusal=result.refusal if result is not None else None,
        messages=messages,
    )


# -- bench --------------------------------------------------------------------------------------
_NUM_IN_TEXT = re.compile(r"[-−]?\d[\d,]*(?:\.\d+)?|[-−]?\d{1,3}(?:\.\d{3})+,\d+")


def _numbers(text: str) -> List[float]:
    out = []
    for m in _NUM_IN_TEXT.finditer(text or ""):
        raw = m.group(0).replace("−", "-")
        neg = raw.startswith("-")
        try:
            v, _ = parse_number(raw.lstrip("-"))
        except ValueError:
            continue
        out.append(-v if neg else v)
    return out


def score(answer_text: str, expected: Any, tolerance: float = 0.0) -> bool:
    """Numeric ``expected``: the ANSWER line's number (else any number in the text) within
    ``tolerance`` (absolute; default allows only float noise). Text ``expected``: case-insensitive
    containment in the ANSWER line (else the whole text)."""
    final = final_answer(answer_text)
    if isinstance(expected, (int, float)) and not isinstance(expected, bool):
        candidates = _numbers(final) if final else []
        if not candidates:
            candidates = _numbers(answer_text)
        tol = max(float(tolerance or 0.0), 1e-9 * max(1.0, abs(float(expected))))
        return any(abs(c - float(expected)) <= tol for c in candidates)
    target = str(expected).strip().lower()
    hay = (final or answer_text or "").lower()
    return bool(target) and target in hay


@dataclass
class BenchTable:
    rows: List[Dict[str, Any]]

    def by_model(self) -> List[Dict[str, Any]]:
        agg: Dict[str, Dict[str, Any]] = {}
        for r in self.rows:
            a = agg.setdefault(
                r["model"],
                {"model": r["model"], "cases": 0, "correct": 0, "errors": 0, "tool_calls": 0,
                 "input_tokens": 0, "output_tokens": 0, "bad_citations": 0, "seconds": 0.0},
            )
            a["cases"] += 1
            a["correct"] += int(bool(r["correct"]))
            a["errors"] += int(r["error"] is not None)
            a["tool_calls"] += r["tool_calls"]
            a["input_tokens"] += r["input_tokens"]
            a["output_tokens"] += r["output_tokens"]
            a["bad_citations"] += r["bad_citations"]
            a["seconds"] += r["seconds"]
        for a in agg.values():
            n = max(1, a["cases"])
            a["accuracy"] = round(a["correct"] / n, 3)
            a["avg_tool_calls"] = round(a["tool_calls"] / n, 2)
            a["seconds"] = round(a["seconds"], 2)
        return list(agg.values())

    def to_markdown(self) -> str:
        lines = [
            "| model | accuracy | correct/cases | avg tool calls | input tok | output tok | bad citations | errors | s |",
            "|---|---|---|---|---|---|---|---|---|",
        ]
        for a in self.by_model():
            lines.append(
                f"| {a['model']} | {a['accuracy']:.1%} | {a['correct']}/{a['cases']} | {a['avg_tool_calls']} | "
                f"{a['input_tokens']} | {a['output_tokens']} | {a['bad_citations']} | {a['errors']} | {a['seconds']} |"
            )
        return "\n".join(lines)

    def __str__(self) -> str:
        return self.to_markdown()


def bench(
    cases: Iterable[Mapping[str, Any]],
    clients: Union[ChatClient, Sequence[ChatClient], Mapping[str, ChatClient]],
    *,
    max_turns: int = 8,
    **ask_options: Any,
) -> BenchTable:
    """Runs every case on every client. A case is ``{file, question, expected, tolerance}``.

    Each call costs money on hosted providers. Errors (API or tool) are recorded per row, not raised.
    """
    if isinstance(clients, ChatClient):
        named: Dict[str, ChatClient] = {clients.name: clients}
    elif isinstance(clients, Mapping):
        named = dict(clients)
    else:
        named = {c.name: c for c in clients}
    case_list = list(cases)
    books: Dict[str, Any] = {}
    rows: List[Dict[str, Any]] = []
    for model_name, client in named.items():
        for i, case in enumerate(case_list):
            path = str(case["file"])
            row: Dict[str, Any] = {
                "model": model_name, "case": case.get("id", i), "question": case["question"],
                "expected": case.get("expected"), "answer": None, "final": None, "correct": False,
                "tool_calls": 0, "input_tokens": 0, "output_tokens": 0, "bad_citations": 0,
                "seconds": 0.0, "stop_reason": None, "error": None,
            }
            try:
                if path not in books:
                    books[path] = load(path)
                ans = ask(books[path], case["question"], client, max_turns=max_turns, **ask_options)
                row.update(
                    answer=ans.text, final=ans.final, tool_calls=len(ans.tool_calls_made),
                    input_tokens=ans.usage.input_tokens, output_tokens=ans.usage.output_tokens,
                    bad_citations=len(ans.citations.invalid), seconds=ans.seconds, stop_reason=ans.stop_reason,
                    correct=score(ans.text, case.get("expected"), float(case.get("tolerance") or 0.0)),
                )
            except Exception as e:  # noqa: BLE001 - one failing case must not stop the bench
                row["error"] = f"{type(e).__name__}: {e}"
            rows.append(row)
    return BenchTable(rows)


def load_cases(path: str) -> List[Dict[str, Any]]:
    """Cases from a JSON list or JSON-lines file."""
    with open(path, encoding="utf-8") as f:
        text = f.read()
    text = text.strip()
    if text.startswith("["):
        return json.loads(text)
    return [json.loads(line) for line in text.splitlines() if line.strip()]
