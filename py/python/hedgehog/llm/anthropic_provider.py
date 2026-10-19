"""Anthropic Messages API provider (``anthropic`` SDK 1.x).

Rules followed for current Claude models:
* exact model ids only (``claude-opus-5-5`` default), no date suffixes;
* ``thinking`` is never sent (no ``{"type": "disabled"}``, no ``budget_tokens``) — depth is set with
  the optional ``effort`` via ``output_config``;
* no forced ``tool_choice`` (auto only) and no assistant prefill;
* structured output via ``output_config={"format": {"type": "json_schema", ...}}``;
* ``stop_reason`` is checked first, including ``"refusal"`` (category from ``stop_details``);
* tool inputs are taken as JSON objects; all tool results of a turn go back in one user message;
* top-level ``cache_control={"type": "ephemeral"}`` caches the stable prefix (workbook context);
* on models that support it, server-side refusal fallbacks are on by default through
  ``client.beta.messages.create(..., betas=["server-side-fallback-2026-07-01"], fallbacks="default")``.
"""

from __future__ import annotations

import logging
from typing import Any, Dict, List, Optional

from .base import ChatClient, header_retry_after, quota_exhausted
from .types import ChatResult, Message, MissingDependency, Tool, ToolCall, Usage, extract_json

logger = logging.getLogger("hedgehog.llm")

DEFAULT_MODEL = "claude-opus-5-5"
MODELS = ("claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5", "claude-fable-5-1")
FALLBACK_MODELS = frozenset({"claude-opus-5-5", "claude-sonnet-5-5", "claude-fable-5-1"})
FALLBACK_BETA = "server-side-fallback-2026-07-01"
EFFORTS = ("low", "medium", "high", "xhigh", "max")


def _import_sdk():
    try:
        import anthropic  # noqa: PLC0415
    except ImportError as e:  # pragma: no cover - depends on environment
        raise MissingDependency(
            "The Anthropic provider needs the `anthropic` package: pip install 'hedgehog-sheets[anthropic]'"
        ) from e
    return anthropic


def _dump(block: Any) -> Dict[str, Any]:
    """SDK content block → request param dict (aliases like ``from`` kept, unset fields dropped)."""
    if isinstance(block, dict):
        return block
    return block.model_dump(mode="json", by_alias=True, exclude_unset=True, exclude_none=True)


def _get(obj: Any, name: str, default: Any = None) -> Any:
    if isinstance(obj, dict):
        return obj.get(name, default)
    return getattr(obj, name, default)


class AnthropicClient(ChatClient):
    provider = "anthropic"
    default_max_tokens = 16000

    def __init__(
        self,
        *,
        api_key: str,
        model: str = DEFAULT_MODEL,
        base_url: Optional[str] = None,
        effort: Optional[str] = None,
        fallbacks: Any = "default",
        cache: bool = True,
        timeout: Optional[float] = 600.0,
        client: Any = None,
        **base_opts: Any,
    ) -> None:
        super().__init__(model=model, **base_opts)
        if effort is not None and effort not in EFFORTS:
            raise ValueError(f"effort must be one of {EFFORTS}, got {effort!r}")
        self.effort = effort
        # True / "default" → server default fallback chain; False / None → off; a list → explicit chain.
        self.fallbacks = "default" if fallbacks is True else (fallbacks or None)
        self.cache = cache
        if client is None:
            sdk = _import_sdk()
            kwargs: Dict[str, Any] = {"api_key": api_key, "max_retries": 0}
            if base_url:
                kwargs["base_url"] = base_url
            if timeout is not None:
                kwargs["timeout"] = timeout
            client = sdk.Anthropic(**kwargs)
        self._client = client
        self._fallbacks_supported = True

    # -- request building -------------------------------------------------------------------
    def _messages(self, messages: List[Message]) -> List[Dict[str, Any]]:
        out: List[Dict[str, Any]] = []
        for m in messages:
            if m.role == "assistant":
                if m.raw is not None and m.raw_provider == self.provider:
                    content = [_dump(b) for b in m.raw]
                else:
                    content = []
                    if m.text:
                        content.append({"type": "text", "text": m.text})
                    for tc in m.tool_calls:
                        content.append({"type": "tool_use", "id": tc.id, "name": tc.name, "input": tc.arguments})
                out.append({"role": "assistant", "content": content})
                continue
            content = [
                {"type": "tool_result", "tool_use_id": r.tool_call_id, "content": r.content, "is_error": r.is_error}
                for r in m.tool_results
            ]
            if m.text:
                content.append({"type": "text", "text": m.text})
            out.append({"role": "user", "content": content if m.tool_results else m.text})
        return out

    def _uses_fallbacks(self) -> bool:
        return bool(self.fallbacks) and self._fallbacks_supported and self.model in FALLBACK_MODELS

    def _chat(
        self,
        messages: List[Message],
        *,
        system: Optional[str],
        tools: List[Tool],
        json_schema: Optional[Dict[str, Any]],
        max_tokens: int,
        temperature: Optional[float],
        effort: Optional[str] = None,
        **extra: Any,
    ) -> ChatResult:
        if messages and messages[-1].role == "assistant":
            raise ValueError("the last message must be a user turn (assistant prefill is not supported)")
        kwargs: Dict[str, Any] = {
            "model": self.model,
            "max_tokens": max_tokens,
            "messages": self._messages(messages),
        }
        if system:
            kwargs["system"] = system
        if tools:
            # tool_choice left out → "auto" (forced any/tool choice is rejected on current models)
            kwargs["tools"] = [
                {"name": t.name, "description": t.description, "input_schema": t.parameters} for t in tools
            ]
        if temperature is not None:
            kwargs["temperature"] = temperature
        output_config: Dict[str, Any] = {}
        eff = effort or self.effort
        if eff:
            if eff not in EFFORTS:
                raise ValueError(f"effort must be one of {EFFORTS}, got {eff!r}")
            output_config["effort"] = eff
        if json_schema:
            output_config["format"] = {"type": "json_schema", "schema": json_schema}
        if output_config:
            kwargs["output_config"] = output_config
        if self.cache:
            kwargs["cache_control"] = {"type": "ephemeral"}
        kwargs.update(extra)

        if self._uses_fallbacks():
            try:
                resp = self._client.beta.messages.create(betas=[FALLBACK_BETA], fallbacks=self.fallbacks, **kwargs)
            except TypeError as e:
                # Older SDK without the beta parameters: run without fallbacks from now on.
                if "fallbacks" not in str(e) and "betas" not in str(e):
                    raise
                logger.warning("anthropic SDK does not accept `fallbacks`; continuing without server-side fallback")
                self._fallbacks_supported = False
                resp = self._client.messages.create(**kwargs)
        else:
            resp = self._client.messages.create(**kwargs)
        return self._parse(resp, json_schema is not None)

    # -- response parsing -------------------------------------------------------------------
    def _parse(self, resp: Any, want_json: bool) -> ChatResult:
        stop = _get(resp, "stop_reason") or ""
        refusal = None
        if stop == "refusal":
            details = _get(resp, "stop_details")
            refusal = {
                "category": _get(details, "category") if details is not None else None,
                "explanation": _get(details, "explanation") if details is not None else None,
            }
        texts: List[str] = []
        calls: List[ToolCall] = []
        content = list(_get(resp, "content") or [])
        for block in content:
            kind = _get(block, "type")
            if kind == "text":
                texts.append(_get(block, "text") or "")
            elif kind == "tool_use":
                args = _get(block, "input")
                calls.append(ToolCall(id=_get(block, "id"), name=_get(block, "name"), arguments=args if isinstance(args, dict) else {}))
        u = _get(resp, "usage")
        usage = Usage(
            input_tokens=int(_get(u, "input_tokens", 0) or 0),
            output_tokens=int(_get(u, "output_tokens", 0) or 0),
            cache_read_tokens=int(_get(u, "cache_read_input_tokens", 0) or 0),
            cache_write_tokens=int(_get(u, "cache_creation_input_tokens", 0) or 0),
            calls=1,
        )
        text = "".join(texts)
        parsed = None
        if want_json and stop != "refusal" and text:
            parsed = extract_json(text)
        return ChatResult(
            text=text,
            tool_calls=calls if stop != "refusal" else [],
            stop_reason=stop,
            usage=usage,
            raw=resp,
            model=_get(resp, "model") or self.model,
            provider=self.provider,
            json=parsed,
            refusal=refusal,
            raw_content=content or None,
        )

    def _retry_info(self, exc: BaseException):
        try:
            sdk = _import_sdk()
        except MissingDependency:
            return None
        if quota_exhausted(exc):
            return None
        if isinstance(exc, (sdk.RateLimitError, sdk.APIConnectionError)):  # APITimeoutError is a subclass
            return (header_retry_after(exc),)
        if isinstance(exc, sdk.APIStatusError):
            code = getattr(exc, "status_code", 0) or 0
            if code == 429 or code >= 500:  # 500, 503, 529 overloaded
                return (header_retry_after(exc),)
        return None
