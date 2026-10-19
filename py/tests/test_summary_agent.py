"""summarize / ask / bench with a scripted ChatClient (no provider SDK, no network)."""

import pytest

import hedgehog
from hedgehog.llm import ChatClient, ChatResult, ToolCall, Usage, ask, bench, score
from hedgehog.summary import compact_facts


class Scripted(ChatClient):
    provider = "scripted"

    def __init__(self, replies, model="script-1"):
        super().__init__(model=model, sleep=lambda s: None)
        self.replies = list(replies)
        self.calls = []

    def _chat(self, messages, **kw):
        self.calls.append({"messages": list(messages), **kw})
        r = self.replies.pop(0)
        if isinstance(r, BaseException):
            raise r
        if isinstance(r, str):
            return ChatResult(text=r, tool_calls=[], stop_reason="end_turn", usage=Usage(100, 10, calls=1), provider=self.provider)
        return ChatResult(text="", tool_calls=r, stop_reason="tool_use", usage=Usage(100, 10, calls=1), provider=self.provider)


def test_deterministic_summary(sales_xlsx):
    s = hedgehog.summarize(sales_xlsx)
    assert s.source == "deterministic" and s.usage is None
    assert "Sales!C5" in s.text and "mismatched" in s.text and "B2:B4" in s.text
    assert "My Notes" in s.text


def test_llm_summary_single_call_checked(sales_xlsx):
    llm = Scripted([
        "Sales has a wrong total: 30 (Sales!C5) should be 29. Q1 total 16 (Sales!B5). "
        "North Q1 is 99 (Sales!B2). See Foo!A1. Also 4,321.5 somewhere."
    ])
    s = hedgehog.summarize(sales_xlsx, llm=llm)
    assert s.source == "llm" and s.usage.input_tokens == 100 and len(llm.calls) == 1
    call = llm.calls[0]
    assert call["tools"] == [] and call["max_tokens"] == 800 and call["temperature"] == 0.2
    assert "Facts (JSON)" in call["messages"][0].text
    probs = " | ".join(s.warnings)
    assert "Sales!B2: text says 99" in probs and "Foo!A1: no such sheet" in probs and "4,321.5" in probs
    ok = {c.ref for c in s.citations.citations if c.status == "ok"}
    assert {"Sales!C5", "Sales!B5"} <= ok


def test_llm_failure_falls_back(sales_xlsx):
    s = hedgehog.summarize(sales_xlsx, llm=Scripted([RuntimeError("boom")]))
    assert s.source == "deterministic" and s.llm_error == "RuntimeError: boom" and "Sales!C5" in s.text


def test_temperature_rejected_is_retried_without(sales_xlsx):
    llm = Scripted([ValueError("Unsupported value: 'temperature'"), "Fine (Sales!B5) 16"])
    s = hedgehog.summarize(sales_xlsx, llm=llm)
    assert s.source == "llm" and llm.calls[1]["temperature"] is None


def test_compact_facts_budget(sales_xlsx):
    f = hedgehog.facts(sales_xlsx)
    full = compact_facts(f, 3000)
    assert "omitted" not in full and len(full["totals"]["items"]) == 3
    tiny = compact_facts(f, 120)
    assert [s["name"] for s in tiny["sheets"]] == ["Sales", "My Notes"]  # sheet list always kept
    assert tiny["totals"]["found"] == 3 and tiny["omitted"]


def test_ask_tool_loop(sales_xlsx):
    llm = Scripted([
        [ToolCall("1", "describe_workbook", {})],
        [ToolCall("2", "get_cell", {"sheet": "Sales", "address": "B5"}),
         ToolCall("3", "get_range", {"sheet": "Sales", "range": "A1:Z9999"}),
         ToolCall("4", "find_text", {"query": "beograd"}),
         ToolCall("5", "check_totals", {"sheet": "Sales"}),
         ToolCall("6", "nope", {})],
        "Q1 total is 16 (Sales!B5).\nANSWER: 16",
    ])
    a = ask(sales_xlsx, "What is the Q1 total?", llm)
    assert a.final == "16" and a.cited_cells == ["Sales!B5"] and a.citations.ok
    assert [c["name"] for c in a.tool_calls_made] == ["describe_workbook", "get_cell", "get_range", "find_text", "check_totals", "nope"]
    assert [c["error"] for c in a.tool_calls_made] == [False, False, True, False, False, True]
    assert a.usage.input_tokens == 300 and a.turns == 3
    # all five results went back in ONE user message
    results_msg = llm.calls[2]["messages"][-1]
    assert len(results_msg.tool_results) == 5
    assert '"value":16' in results_msg.tool_results[0].content
    assert "Београд" in results_msg.tool_results[2].content


def test_ask_max_turns(sales_xlsx):
    llm = Scripted([[ToolCall(str(i), "describe_workbook", {})] for i in range(3)])
    a = ask(sales_xlsx, "q", llm, max_turns=3)
    assert a.stop_reason == "max_turns" and a.turns == 3
    assert "limit" in llm.calls[2]["messages"][-1].text


def test_score():
    assert score("blah 1,234.50 (S!B2)\nANSWER: 1234.5", 1234.5)
    assert not score("ANSWER: 1200", 1234.5)
    assert score("ANSWER: 1200", 1234.5, tolerance=50)
    assert score("the answer is 29", 29)
    assert score("ANSWER: Belgrade", "belgrade")


def test_bench(sales_xlsx):
    cases = [
        {"file": sales_xlsx, "question": "Q1 total?", "expected": 16},
        {"file": sales_xlsx, "question": "Q2 correct total?", "expected": 29, "tolerance": 0.5},
    ]
    good = Scripted(["16 (Sales!B5)\nANSWER: 16", "ANSWER: 29"], model="good")
    bad = Scripted(["ANSWER: 15", RuntimeError("api down")], model="bad")
    table = bench(cases, [good, bad])
    agg = {a["model"]: a for a in table.by_model()}
    assert agg["scripted:good"]["accuracy"] == 1.0 and agg["scripted:bad"]["accuracy"] == 0.0
    assert agg["scripted:bad"]["errors"] == 1
    assert "| scripted:good | 100.0% |" in table.to_markdown()
