"""AWS Bedrock provider via boto3 ``bedrock-runtime`` Converse, with bearer-token (API key) auth."""

from __future__ import annotations

import os
import threading
from typing import Any, Dict, List, Optional

from .base import ChatClient
from .types import ChatResult, Message, MissingDependency, Tool, ToolCall, Usage, extract_json, strip_think

_STOP = {
    "end_turn": "end_turn",
    "tool_use": "tool_use",
    "max_tokens": "max_tokens",
    "stop_sequence": "stop_sequence",
    "guardrail_intervened": "refusal",
    "content_filtered": "refusal",
}
_RETRY_CODES = {
    "ThrottlingException",
    "TooManyRequestsException",
    "ServiceUnavailableException",
    "InternalServerException",
    "ModelNotReadyException",
    "ServiceQuotaExceededException",
}
JSON_TOOL = "structured_output"
_env_lock = threading.Lock()


def _import_sdk():
    try:
        import boto3  # noqa: PLC0415
        from botocore.config import Config  # noqa: PLC0415
    except ImportError as e:  # pragma: no cover - depends on environment
        raise MissingDependency("The Bedrock provider needs `boto3`: pip install 'hedgehog-sheets[bedrock]'") from e
    return boto3, Config


class BedrockClient(ChatClient):
    """Bedrock Converse. The API key is exported as ``AWS_BEARER_TOKEN_BEDROCK`` (process-wide, which
    is how botocore discovers bearer tokens) before the boto3 client is created; with
    ``use_aws_credentials=True`` and no key, the normal AWS credential chain is used instead.

    Structured output (``json_schema``) uses a ``structured_output`` tool with automatic tool choice;
    if the model answers in text instead, the JSON is extracted from the text.
    """

    provider = "bedrock"
    default_max_tokens = 4096

    def __init__(
        self,
        *,
        api_key: Optional[str],
        model: str,
        region: str,
        timeout: float = 600.0,
        client: Any = None,
        **base_opts: Any,
    ) -> None:
        super().__init__(model=model, **base_opts)
        self.region = region
        if client is None:
            boto3, Config = _import_sdk()
            with _env_lock:
                if api_key:
                    os.environ["AWS_BEARER_TOKEN_BEDROCK"] = api_key
                client = boto3.client(
                    "bedrock-runtime",
                    region_name=region,
                    config=Config(retries={"max_attempts": 1, "mode": "standard"}, read_timeout=timeout),
                )
        self._client = client

    def __repr__(self) -> str:
        return f"BedrockClient(model={self.model!r}, region={self.region!r})"

    # -- request building -------------------------------------------------------------------
    def _messages(self, messages: List[Message]) -> List[Dict[str, Any]]:
        out: List[Dict[str, Any]] = []
        for m in messages:
            if m.role == "assistant":
                if m.raw is not None and m.raw_provider == self.provider:
                    content = list(m.raw)
                else:
                    content = []
                    if m.text:
                        content.append({"text": m.text})
                    for tc in m.tool_calls:
                        content.append({"toolUse": {"toolUseId": tc.id, "name": tc.name, "input": tc.arguments}})
                out.append({"role": "assistant", "content": content})
                continue
            content = []
            for r in m.tool_results:
                block: Dict[str, Any] = {"toolUseId": r.tool_call_id, "content": [{"text": r.content}]}
                if r.is_error:
                    block["status"] = "error"
                content.append({"toolResult": block})
            if m.text:
                content.append({"text": m.text})
            out.append({"role": "user", "content": content})
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
        **extra: Any,
    ) -> ChatResult:
        kwargs: Dict[str, Any] = {"modelId": self.model, "messages": self._messages(messages)}
        sys_text = system or ""
        specs = [
            {"toolSpec": {"name": t.name, "description": t.description or t.name, "inputSchema": {"json": t.parameters}}}
            for t in tools
        ]
        if json_schema:
            specs.append(
                {
                    "toolSpec": {
                        "name": JSON_TOOL,
                        "description": "Return the final answer as structured data.",
                        "inputSchema": {"json": json_schema},
                    }
                }
            )
            sys_text = (sys_text + "\n\n" if sys_text else "") + (
                f"When you have the final answer, return it by calling the `{JSON_TOOL}` tool."
            )
        if sys_text:
            kwargs["system"] = [{"text": sys_text}]
        if specs:
            kwargs["toolConfig"] = {"tools": specs}
        inference: Dict[str, Any] = {"maxTokens": max_tokens}
        if temperature is not None:
            inference["temperature"] = temperature
        kwargs["inferenceConfig"] = inference
        kwargs.update(extra)
        resp = self._client.converse(**kwargs)
        return self._parse(resp, json_schema is not None)

    # -- response parsing -------------------------------------------------------------------
    def _parse(self, resp: Dict[str, Any], want_json: bool) -> ChatResult:
        content = (resp.get("output") or {}).get("message", {}).get("content", []) or []
        texts: List[str] = []
        calls: List[ToolCall] = []
        parsed: Any = None
        got_json_tool = False
        for block in content:
            if "text" in block:
                texts.append(str(block["text"]))
            elif "toolUse" in block:
                tu = block["toolUse"]
                args = tu.get("input")
                args = args if isinstance(args, dict) else {}
                if want_json and tu.get("name") == JSON_TOOL:
                    parsed, got_json_tool = args, True
                    continue
                calls.append(ToolCall(id=str(tu.get("toolUseId", "")), name=str(tu.get("name", "")), arguments=args))
        raw_stop = str(resp.get("stopReason", ""))
        stop = _STOP.get(raw_stop, raw_stop)
        text = strip_think("".join(texts))
        if want_json and not got_json_tool and text:
            parsed = extract_json(text)
        if got_json_tool and not calls:
            stop = "end_turn"
        refusal = {"category": raw_stop, "explanation": text or None} if stop == "refusal" else None
        u = resp.get("usage") or {}
        usage = Usage(
            input_tokens=int(u.get("inputTokens", 0) or 0),
            output_tokens=int(u.get("outputTokens", 0) or 0),
            cache_read_tokens=int(u.get("cacheReadInputTokens", 0) or 0),
            cache_write_tokens=int(u.get("cacheWriteInputTokens", 0) or 0),
            calls=1,
        )
        return ChatResult(
            text=text,
            tool_calls=calls,
            stop_reason=stop,
            usage=usage,
            raw=resp,
            model=self.model,
            provider=self.provider,
            json=parsed,
            refusal=refusal,
            # A structured_output toolUse must not be replayed without its result; rebuild instead.
            raw_content=None if got_json_tool else (content or None),
        )

    def _retry_info(self, exc: BaseException):
        err = getattr(exc, "response", None)
        if isinstance(err, dict):  # botocore ClientError
            code = (err.get("Error") or {}).get("Code", "")
            status = (err.get("ResponseMetadata") or {}).get("HTTPStatusCode", 0) or 0
            if code in _RETRY_CODES or status == 429 or status >= 500:
                return (None,)
            return None
        name = type(exc).__name__
        if name in ("EndpointConnectionError", "ConnectTimeoutError", "ReadTimeoutError", "ConnectionClosedError"):
            return (None,)
        return None
