"""Provider-neutral chat types shared by every LLM provider."""

from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from typing import Any, Dict, List, Mapping, Optional, Sequence, Union


class LLMError(Exception):
    """Base error of the hedgehog LLM layer."""


class ConfigError(LLMError, ValueError):
    """Missing or invalid provider configuration (API key, model, base URL, region)."""


class MissingDependency(LLMError, ImportError):
    """The provider SDK is not installed."""


@dataclass
class ToolCall:
    id: str
    name: str
    arguments: Dict[str, Any]


@dataclass
class ToolResult:
    tool_call_id: str
    content: str
    is_error: bool = False


@dataclass
class Tool:
    name: str
    description: str
    parameters: Dict[str, Any]  # JSON schema of the arguments object


@dataclass
class Message:
    """One turn. A user turn carries text and/or tool results; an assistant turn carries text and/or
    tool calls. ``raw`` keeps the provider's native assistant content (e.g. thinking or fallback
    blocks) so the same provider can replay it verbatim; other providers use text/tool_calls."""

    role: str  # "user" | "assistant"
    text: str = ""
    tool_calls: List[ToolCall] = field(default_factory=list)
    tool_results: List[ToolResult] = field(default_factory=list)
    raw: Any = None
    raw_provider: Optional[str] = None


MessageLike = Union[Message, Mapping[str, Any], str]


@dataclass
class Usage:
    input_tokens: int = 0
    output_tokens: int = 0
    cache_read_tokens: int = 0
    cache_write_tokens: int = 0
    calls: int = 0

    @property
    def total_tokens(self) -> int:
        return self.input_tokens + self.output_tokens

    def __add__(self, other: "Usage") -> "Usage":
        return Usage(
            self.input_tokens + other.input_tokens,
            self.output_tokens + other.output_tokens,
            self.cache_read_tokens + other.cache_read_tokens,
            self.cache_write_tokens + other.cache_write_tokens,
            self.calls + other.calls,
        )


@dataclass
class ChatResult:
    text: str
    tool_calls: List[ToolCall]
    # Normalized: "end_turn" | "tool_use" | "max_tokens" | "refusal" | "stop_sequence" | provider value.
    stop_reason: str
    usage: Usage
    raw: Any = None
    model: Optional[str] = None
    provider: Optional[str] = None
    # Parsed object when ``json_schema`` was requested (None if the reply was not valid JSON).
    json: Any = None
    # For stop_reason == "refusal": {"category": ..., "explanation": ...}.
    refusal: Optional[Dict[str, Any]] = None
    # Provider-native assistant content to replay (see Message.raw).
    raw_content: Any = None

    @property
    def wants_tools(self) -> bool:
        return bool(self.tool_calls)

    def to_message(self) -> Message:
        return Message(
            role="assistant",
            text=self.text,
            tool_calls=list(self.tool_calls),
            raw=self.raw_content,
            raw_provider=self.provider if self.raw_content is not None else None,
        )


def user(text: str) -> Message:
    return Message(role="user", text=text)


def tool_results(results: Sequence[ToolResult], text: str = "") -> Message:
    """All results of one assistant turn go back in a single user message."""
    return Message(role="user", text=text, tool_results=list(results))


def as_message(m: MessageLike) -> Message:
    if isinstance(m, Message):
        return m
    if isinstance(m, str):
        return user(m)
    role = m.get("role", "user")
    content = m.get("content", m.get("text", ""))
    if not isinstance(content, str):
        raise TypeError("dict messages need string content; use hedgehog.llm.Message for tool turns")
    return Message(role=role, text=content)


def as_tool(t: Union[Tool, Mapping[str, Any]]) -> Tool:
    if isinstance(t, Tool):
        return t
    return Tool(
        name=t["name"],
        description=t.get("description", ""),
        parameters=t.get("parameters") or t.get("input_schema") or {"type": "object", "properties": {}},
    )


_THINK_RE = re.compile(r"<think>.*?</think>\s*", re.DOTALL | re.IGNORECASE)


def strip_think(text: str) -> str:
    """Removes ``<think>…</think>`` blocks (Qwen & co.); an unclosed leading block is dropped too."""
    if not text:
        return text
    out = _THINK_RE.sub("", text)
    low = out.lstrip().lower()
    if low.startswith("<think>"):
        out = ""
    elif "</think>" in out.lower():  # opening tag eaten by the chat template
        out = out[out.lower().rindex("</think>") + len("</think>"):]
    return out.strip()


_JSON_OBJECT_RE = re.compile(r"[\{\[].*[\}\]]", re.DOTALL)


def extract_json(text: str) -> Any:
    """Best-effort JSON from model text (plain, ```json fenced, or embedded). None if not found."""
    if not text or not text.strip():
        return None
    s = text.strip()
    if s.startswith("```"):
        nl = s.find("\n")
        s = s[nl + 1:] if nl != -1 else s[3:]
        if s.rstrip().endswith("```"):
            s = s.rstrip()[:-3]
        s = s.strip()
    try:
        return json.loads(s)
    except ValueError:
        m = _JSON_OBJECT_RE.search(s)
        if not m:
            return None
        try:
            return json.loads(m.group(0))
        except ValueError:
            return None
