"""ChatClient base: one provider-neutral ``chat`` call with a concurrency limit and retries."""

from __future__ import annotations

import logging
import random
import threading
import time
from typing import Any, Callable, Dict, List, Mapping, Optional, Sequence, Tuple, Union

from .types import ChatResult, Message, MessageLike, Tool, as_message, as_tool

logger = logging.getLogger("hedgehog.llm")

# Process-wide cap on in-flight LLM calls across all clients (AI_MAX_CONCURRENT_LLM); None = no cap.
_global_sem: Optional[threading.BoundedSemaphore] = None
_global_limit: Optional[int] = None
_global_lock = threading.Lock()


def set_max_concurrent(limit: Optional[int]) -> None:
    """Caps concurrent LLM calls across the whole process (``None`` removes the cap)."""
    global _global_sem, _global_limit
    with _global_lock:
        if limit == _global_limit:
            return
        _global_limit = limit
        _global_sem = threading.BoundedSemaphore(max(1, int(limit))) if limit else None


class _Nullctx:
    def __enter__(self) -> None:
        return None

    def __exit__(self, *a: Any) -> None:
        return None


class ChatClient:
    """Provider-neutral chat client. Subclasses implement ``_chat`` and ``_retry_info``.

    * ``max_concurrency`` — at most this many requests in flight from this client (threads).
    * ``max_retries`` — retries on rate limits / overload / 5xx / connection errors, with exponential
      backoff (``retry_base`` seconds, doubled each time, jittered, ``Retry-After`` honoured up to
      ``retry_max`` seconds). SDK-internal retries are switched off so this is the only layer.
    """

    provider: str = "base"
    default_max_tokens: int = 4096

    def __init__(
        self,
        *,
        model: str,
        max_concurrency: int = 4,
        max_retries: int = 3,
        retry_base: float = 1.0,
        retry_max: float = 30.0,
        sleep: Callable[[float], None] = time.sleep,
    ) -> None:
        self.model = model
        self.max_retries = max_retries
        self.retry_base = retry_base
        self.retry_max = retry_max
        self._sleep = sleep
        self._sem = threading.BoundedSemaphore(max(1, max_concurrency))

    # -- public -----------------------------------------------------------------------------
    def chat(
        self,
        messages: Union[MessageLike, Sequence[MessageLike]],
        *,
        system: Optional[str] = None,
        tools: Optional[Sequence[Union[Tool, Mapping[str, Any]]]] = None,
        json_schema: Optional[Mapping[str, Any]] = None,
        max_tokens: Optional[int] = None,
        temperature: Optional[float] = None,
        **options: Any,
    ) -> ChatResult:
        """One model turn.

        ``messages``: a string, a ``{"role", "content"}`` dict, a :class:`Message`, or a list of them.
        ``tools``: :class:`Tool` objects or ``{"name", "description", "parameters"}`` dicts.
        ``json_schema``: a JSON schema the reply must follow; the parsed value is ``result.json``.
        Provider-specific ``options`` (e.g. ``effort`` for Anthropic) are passed through.
        """
        if isinstance(messages, (str, Message)) or isinstance(messages, Mapping):
            messages = [messages]  # type: ignore[list-item]
        msgs = [as_message(m) for m in messages]  # type: ignore[union-attr]
        tool_list = [as_tool(t) for t in tools] if tools else []
        mt = max_tokens or self.default_max_tokens
        glob = _global_sem or _Nullctx()
        with glob, self._sem:
            return self._with_retry(
                lambda: self._chat(
                    msgs,
                    system=system,
                    tools=tool_list,
                    json_schema=dict(json_schema) if json_schema else None,
                    max_tokens=mt,
                    temperature=temperature,
                    **options,
                )
            )

    def __repr__(self) -> str:  # never shows the key
        return f"{type(self).__name__}(provider={self.provider!r}, model={self.model!r})"

    @property
    def name(self) -> str:
        return f"{self.provider}:{self.model}"

    # -- provider hooks -----------------------------------------------------------------------
    def _chat(
        self,
        messages: List[Message],
        *,
        system: Optional[str],
        tools: List[Tool],
        json_schema: Optional[Dict[str, Any]],
        max_tokens: int,
        temperature: Optional[float],
        **options: Any,
    ) -> ChatResult:
        raise NotImplementedError

    def _retry_info(self, exc: BaseException) -> Optional[Tuple[Optional[float]]]:
        """``None`` if ``exc`` is not retryable, else ``(server_suggested_delay_or_None,)``."""
        return None

    # -- retry loop ---------------------------------------------------------------------------
    def _with_retry(self, fn: Callable[[], ChatResult]) -> ChatResult:
        attempt = 0
        while True:
            try:
                return fn()
            except Exception as exc:  # noqa: BLE001 - classified below
                info = self._retry_info(exc)
                if info is None or attempt >= self.max_retries:
                    raise
                hint = info[0]
                delay = self.retry_base * (2**attempt) * (0.5 + random.random() / 2)
                if hint is not None:
                    delay = max(delay, hint)
                delay = min(delay, self.retry_max)
                attempt += 1
                logger.warning(
                    "%s: retryable error (%s), retry %d/%d in %.1fs",
                    self.name, type(exc).__name__, attempt, self.max_retries, delay,
                )
                self._sleep(delay)


_QUOTA_CODES = {"insufficient_quota", "credit_balance_exhausted", "billing_hard_limit_reached"}


def quota_exhausted(exc: BaseException) -> bool:
    """True for "out of credits" errors: they come back as 429 but retrying cannot help."""
    body = getattr(exc, "body", None)
    err = body.get("error", body) if isinstance(body, dict) else {}
    fields = {getattr(exc, "code", None), getattr(exc, "type", None)}
    if isinstance(err, dict):
        fields |= {err.get("code"), err.get("type")}
    return bool(fields & _QUOTA_CODES)


def header_retry_after(exc: BaseException) -> Optional[float]:
    """``Retry-After`` seconds from an SDK exception's HTTP response, if any."""
    resp = getattr(exc, "response", None)
    headers = getattr(resp, "headers", None)
    if not headers:
        return None
    try:
        value = headers.get("retry-after")
        return float(value) if value is not None else None
    except (TypeError, ValueError):
        return None


def list_models(client: "ChatClient") -> List[str]:
    """Model ids the client's key can use (a free ``GET /v1/models`` on OpenAI-style and Anthropic
    endpoints). Bedrock is not supported here (its model list lives in a different AWS API)."""
    sdk_client = getattr(client, "_client", None)
    models = getattr(sdk_client, "models", None)
    if models is None or not hasattr(models, "list"):
        raise NotImplementedError(f"list_models is not available for provider {client.provider!r}")
    page = models.list()
    items = getattr(page, "data", None)
    if items is None:
        items = list(page)
    return sorted(str(getattr(m, "id", m)) for m in items)
