"""OpenAI and OpenAI-compatible (vLLM, Ollama, OpenRouter, Groq, …) chat-completions provider."""

from __future__ import annotations

import json
import logging
from typing import Any, Dict, List, Optional

from .base import ChatClient, header_retry_after, quota_exhausted
from .types import ChatResult, LLMError, Message, MissingDependency, Tool, ToolCall, Usage, extract_json, strip_think

logger = logging.getLogger("hedgehog.llm")

_FINISH = {
    "tool_calls": "tool_use",
    "function_call": "tool_use",
    "stop": "end_turn",
    "length": "max_tokens",
    "content_filter": "refusal",
}

# Thinking suppression for Qwen-style models. Unknown extras are usually ignored, and a gateway that
# rejects them gets the request again without (and never again for this client).
THINKING_OFF_TEMPLATE = {"chat_template_kwargs": {"enable_thinking": False}}  # vLLM, Ollama, SGLang
THINKING_OFF_OPENROUTER = {"reasoning": {"enabled": False}}


def _import_sdk():
    try:
        import openai  # noqa: PLC0415
    except ImportError as e:  # pragma: no cover - depends on environment
        raise MissingDependency(
            "The OpenAI / OpenAI-compatible provider needs the `openai` package: pip install 'hedgehog[openai]'"
        ) from e
    return openai


def _merge(a: Dict[str, Any], b: Dict[str, Any]) -> Dict[str, Any]:
    out = dict(a)
    for k, v in b.items():
        out[k] = _merge(out[k], v) if isinstance(out.get(k), dict) and isinstance(v, dict) else v
    return out


class OpenAIClient(ChatClient):
    """``provider="openai"`` talks to api.openai.com; ``"openai_compatible"`` to any ``base_url``.

    * ``strict_json`` — sets ``json_schema.strict`` (OpenAI strict mode needs every property required
      and ``additionalProperties: false``; vLLM ignores it). Default False.
    * ``disable_thinking`` — ``None`` = auto (on for openai_compatible), True/False to force. The
      payload is ``thinking_extra`` if given, else :data:`THINKING_OFF_OPENROUTER` for openrouter.ai URLs,
      else :data:`THINKING_OFF_TEMPLATE`.
    * ``extra_body`` — merged into every request.
    * ``max_tokens_param`` — ``"max_completion_tokens"`` (OpenAI default) or ``"max_tokens"``
      (compatible default; older vLLM / Ollama only know this one).
    """

    default_max_tokens = 4096

    def __init__(
        self,
        *,
        api_key: str,
        model: str,
        base_url: Optional[str] = None,
        compatible: bool = False,
        strict_json: bool = False,
        disable_thinking: Optional[bool] = None,
        thinking_extra: Optional[Dict[str, Any]] = None,
        extra_body: Optional[Dict[str, Any]] = None,
        max_tokens_param: Optional[str] = None,
        timeout: Optional[float] = 600.0,
        client: Any = None,
        **base_opts: Any,
    ) -> None:
        super().__init__(model=model, **base_opts)
        self.provider = "openai_compatible" if compatible else "openai"
        self.base_url = base_url
        self.strict_json = strict_json
        self.extra_body = dict(extra_body or {})
        self.max_tokens_param = max_tokens_param or ("max_tokens" if compatible else "max_completion_tokens")
        if disable_thinking is None:
            disable_thinking = compatible
        self.thinking_extra: Optional[Dict[str, Any]] = None
        if disable_thinking:
            if thinking_extra is None:
                is_openrouter = bool(base_url) and "openrouter" in base_url.lower()
                thinking_extra = THINKING_OFF_OPENROUTER if is_openrouter else THINKING_OFF_TEMPLATE
            self.thinking_extra = thinking_extra
        self.preset = self.provider
        if client is None:
            sdk = _import_sdk()
            kwargs: Dict[str, Any] = {"api_key": api_key, "max_retries": 0}
            if base_url:
                kwargs["base_url"] = base_url
            if timeout is not None:
                kwargs["timeout"] = timeout
            client = sdk.OpenAI(**kwargs)
        self._client = client

    def __repr__(self) -> str:
        return f"OpenAIClient(preset={self.preset!r}, model={self.model!r}, base_url={self.base_url!r})"

    @property
    def name(self) -> str:
        return f"{self.preset}:{self.model}"

    # -- request building -------------------------------------------------------------------
    @staticmethod
    def _messages(messages: List[Message], system: Optional[str]) -> List[Dict[str, Any]]:
        out: List[Dict[str, Any]] = []
        if system:
            out.append({"role": "system", "content": system})
        for m in messages:
            if m.role == "assistant":
                a: Dict[str, Any] = {"role": "assistant", "content": m.text or None}
                if m.tool_calls:
                    a["tool_calls"] = [
                        {
                            "id": tc.id,
                            "type": "function",
                            "function": {"name": tc.name, "arguments": json.dumps(tc.arguments, ensure_ascii=False)},
                        }
                        for tc in m.tool_calls
                    ]
                out.append(a)
                continue
            for r in m.tool_results:
                content = f"ERROR: {r.content}" if r.is_error else r.content
                out.append({"role": "tool", "tool_call_id": r.tool_call_id, "content": content})
            if m.text or not m.tool_results:
                out.append({"role": m.role if m.role in ("user", "system") else "user", "content": m.text})
        return out

    def _chat(
        self,
        messages: List[Message],
        *,
        system: Optional[str],
        tools: List[Tool],
        json_schema: Optional[Dict[str, Any]],
        max_tokens: int,
        temperature: Optional[float],
        schema_name: str = "result",
        **extra: Any,
    ) -> ChatResult:
        kwargs: Dict[str, Any] = {
            "model": self.model,
            "messages": self._messages(messages, system),
            self.max_tokens_param: max_tokens,
        }
        if temperature is not None:
            kwargs["temperature"] = temperature
        if tools:
            kwargs["tools"] = [
                {"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}}
                for t in tools
            ]
            kwargs["tool_choice"] = "auto"
        if json_schema:
            kwargs["response_format"] = {
                "type": "json_schema",
                "json_schema": {"name": schema_name, "schema": json_schema, "strict": self.strict_json},
            }
        body = dict(self.extra_body)
        if self.thinking_extra:
            body = _merge(body, self.thinking_extra)
        user_extra = extra.pop("extra_body", None)
        if user_extra:
            body = _merge(body, user_extra)
        kwargs.update(extra)
        if body:
            kwargs["extra_body"] = body
        try:
            completion = self._create(kwargs)
        except Exception as exc:  # noqa: BLE001
            if not (self.thinking_extra and self._is_bad_request(exc)):
                raise
            body = dict(self.extra_body)
            if user_extra:
                body = _merge(body, user_extra)
            if body:
                kwargs["extra_body"] = body
            else:
                kwargs.pop("extra_body", None)
            # If the request also fails without the switch, the 400 was about something else:
            # keep the switch and raise that error.
            completion = self._create(kwargs)
            logger.warning("%s: gateway rejected the thinking switch; sending requests without it", self.name)
            self.thinking_extra = None
        return self._parse(completion, json_schema is not None)

    def _create(self, kwargs: Dict[str, Any]) -> Any:
        try:
            return self._client.chat.completions.create(**kwargs)
        except Exception as exc:  # noqa: BLE001
            if getattr(exc, "status_code", None) == 404:
                where = self.base_url or "api.openai.com"
                raise LLMError(
                    f"model {self.model!r} was not found at {where} (HTTP 404). Pick one from "
                    f"hedgehog.llm.list_models(client) and pass connect(..., model=...) or set the model env var."
                ) from exc
            raise

    @staticmethod
    def _is_bad_request(exc: BaseException) -> bool:
        code = getattr(exc, "status_code", None)
        return code in (400, 422)

    # -- response parsing -------------------------------------------------------------------
    def _parse(self, completion: Any, want_json: bool) -> ChatResult:
        choice = completion.choices[0]
        msg = choice.message
        text = strip_think(msg.content or "")
        calls: List[ToolCall] = []
        for tc in getattr(msg, "tool_calls", None) or []:
            fn = tc.function
            try:
                args = json.loads(fn.arguments or "{}")
            except ValueError:
                args = {}
            calls.append(ToolCall(id=tc.id, name=fn.name, arguments=args if isinstance(args, dict) else {}))
        stop = _FINISH.get(choice.finish_reason or "", choice.finish_reason or "end_turn")
        refusal = None
        refusal_text = getattr(msg, "refusal", None)
        if refusal_text or stop == "refusal":
            stop = "refusal"
            refusal = {"category": None if refusal_text else "content_filter", "explanation": refusal_text}
        if calls and stop == "end_turn":
            stop = "tool_use"
        u = getattr(completion, "usage", None)
        details = getattr(u, "prompt_tokens_details", None)
        usage = Usage(
            input_tokens=int(getattr(u, "prompt_tokens", 0) or 0),
            output_tokens=int(getattr(u, "completion_tokens", 0) or 0),
            cache_read_tokens=int(getattr(details, "cached_tokens", 0) or 0),
            calls=1,
        )
        return ChatResult(
            text=text,
            tool_calls=calls,
            stop_reason=stop,
            usage=usage,
            raw=completion,
            model=getattr(completion, "model", None) or self.model,
            provider=self.provider,
            json=extract_json(text) if want_json and text else None,
            refusal=refusal,
        )

    def _retry_info(self, exc: BaseException):
        try:
            sdk = _import_sdk()
        except MissingDependency:
            return None
        if quota_exhausted(exc):
            return None
        if isinstance(exc, (sdk.RateLimitError, sdk.APIConnectionError)):
            return (header_retry_after(exc),)
        if isinstance(exc, sdk.APIStatusError):
            code = getattr(exc, "status_code", 0) or 0
            if code == 429 or code >= 500:
                return (header_retry_after(exc),)
        return None
