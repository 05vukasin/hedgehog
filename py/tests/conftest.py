"""Shared fixtures. Spreadsheets are generated with openpyxl in tmp_path; nothing is committed.
No test talks to a real API: SDK clients are fakes or use mock HTTP transports."""

from __future__ import annotations

import os
from typing import Any, List

import pytest

openpyxl = pytest.importorskip("openpyxl")

ENV_VARS = [
    "AI_PROVIDER", "AI_CHAT_MODEL", "OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENAI_MODEL",
    "ANTHROPIC_API_KEY", "ANTHROPIC_MODEL", "AI_BEDROCK_BEARER_TOKEN", "AI_BEDROCK_REGION", "AI_BEDROCK_MODEL",
    "AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION", "AWS_DEFAULT_REGION", "OPENROUTER_API_KEY", "OPENROUTER_BASE_URL",
    "OPENROUTER_MODEL", "GROQ_API_KEY", "GROQ_BASE_URL", "GROQ_MODEL", "API_URL", "API_TOKEN", "API_MODEL",
    "AI_MAX_CONCURRENT_LLM",
]


@pytest.fixture(autouse=True)
def clean_env(monkeypatch, tmp_path):
    """No real credentials leak into tests, no repo .env is picked up, and the agent tools' disk
    cache stays in the test's folder."""
    for v in ENV_VARS:
        monkeypatch.delenv(v, raising=False)
    monkeypatch.setenv("HEDGEHOG_CACHE_DIR", str(tmp_path / "hh-cache"))
    monkeypatch.chdir(tmp_path)
    import hedgehog.llm.env as env_mod

    monkeypatch.setattr(env_mod, "find_env_file", lambda path=None: _explicit(path))
    from hedgehog.llm import base

    base.set_max_concurrent(None)
    yield


def _explicit(path):
    from pathlib import Path

    if path is None:
        p = Path.cwd() / ".env"
        return p if p.is_file() else None
    p = Path(path)
    return p if p.is_file() else None


@pytest.fixture
def sales_xlsx(tmp_path):
    """Sales: a small table with a correct typed total (B5), a wrong one (C5) and a formula (C6)."""
    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "Sales"
    ws.append(["Region", "Q1", "Q2"])
    for row in [("North", 10, 20), ("South", 5, 7), ("Београд", 1, 2)]:
        ws.append(row)
    ws.append(["Total", 16, 30])
    ws.append(["Formula", 17, "=SUM(C2:C4)"])
    notes = wb.create_sheet("My Notes")
    notes["A1"] = "Prepared by finance"
    notes["B3"] = 1234.5
    path = tmp_path / "sales.xlsx"
    wb.save(path)
    return str(path)


class Recorder:
    """Callable that records kwargs and returns queued responses (or raises queued exceptions)."""

    def __init__(self, responses: List[Any]):
        self.responses = list(responses)
        self.calls: List[dict] = []

    def __call__(self, **kwargs):
        self.calls.append(kwargs)
        r = self.responses.pop(0)
        if isinstance(r, BaseException):
            raise r
        return r


def no_sleep(_s: float) -> None:
    return None
