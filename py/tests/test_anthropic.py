"""Anthropic provider against fake SDK objects and a mock httpx2 transport (no network)."""

import json
from types import SimpleNamespace

import pytest

anthropic = pytest.importorskip("anthropic")
httpx2 = pytest.importorskip("httpx2")

from hedgehog.llm import Message, ToolResult, connect, tool_results, user  # noqa: E402
from hedgehog.llm.anthropic_provider import FALLBACK_BETA  # noqa: E402

from conftest import Recorder, no_sleep  # noqa: E402


def msg(content, stop="end_turn", **extra):
    data = {
        "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "content": content, "stop_reason": stop, "stop_sequence": None,
        "usage": {"input_tokens": 100, "output_tokens": 20, "cache_read_input_tokens": 80},
    }
    data.update(extra)
    return anthropic.types.beta.BetaMessage.model_validate(data)


def fake_sdk(create=None, beta_create=None):
    return SimpleNamespace(
        messages=SimpleNamespace(create=create or Recorder([])),
        beta=SimpleNamespace(messages=SimpleNamespace(create=beta_create or Recorder([]))),
    )


def test_request_shape_with_fallbacks():
    beta = Recorder([msg([{"type": "text", "text": "hi"}])])
    c = connect("anthropic", api_key="k", client=fake_sdk(beta_create=beta), effort="medium")
    r = c.chat("hello", system="sys", tools=[{"name": "t", "description": "d", "parameters": {"type": "object"}}])
    kw = beta.calls[0]
    assert kw["betas"] == [FALLBACK_BETA] and kw["fallbacks"] == "default"
    assert kw["model"] == "claude-opus-5-5" and kw["max_tokens"] == 16000
    assert kw["output_config"] == {"effort": "medium"}
    assert kw["cache_control"] == {"type": "ephemeral"}
    assert "thinking" not in kw and "tool_choice" not in kw and "temperature" not in kw
    assert kw["tools"] == [{"name": "t", "description": "d", "input_schema": {"type": "object"}}]
    assert kw["messages"] == [{"role": "user", "content": "hello"}]
    assert r.text == "hi" and r.usage.input_tokens == 100 and r.usage.cache_read_tokens == 80


def test_fallbacks_opt_out_and_haiku():
    plain = Recorder([msg([{"type": "text", "text": "a"}]), msg([{"type": "text", "text": "b"}])])
    c = connect("anthropic", api_key="k", client=fake_sdk(create=plain), fallbacks=False)
    c.chat("x")
    h = connect("anthropic", api_key="k", model="claude-haiku-5-5", client=fake_sdk(create=plain))
    h.chat("y")
    assert len(plain.calls) == 2 and all("fallbacks" not in k and "betas" not in k for k in plain.calls)


def test_json_schema_output_config():
    schema = {"type": "object", "properties": {"total": {"type": "number"}}, "required": ["total"]}
    beta = Recorder([msg([{"type": "text", "text": '{"total": 29}'}])])
    c = connect("anthropic", api_key="k", client=fake_sdk(beta_create=beta))
    r = c.chat("sum?", json_schema=schema)
    assert beta.calls[0]["output_config"] == {"format": {"type": "json_schema", "schema": schema}}
    assert r.json == {"total": 29}


def test_tool_round_trip_replays_raw_content():
    first = msg(
        [
            {"type": "text", "text": "Looking."},
            {"type": "tool_use", "id": "tu_1", "name": "get_cell", "input": {"sheet": "S", "address": "B5"}},
            {"type": "tool_use", "id": "tu_2", "name": "get_cell", "input": {"sheet": "S", "address": "C5"}},
        ],
        stop="tool_use",
    )
    second = msg([{"type": "text", "text": "16 (S!B5)"}])
    beta = Recorder([first, second])
    c = connect("anthropic", api_key="k", client=fake_sdk(beta_create=beta))
    r1 = c.chat("q")
    assert r1.stop_reason == "tool_use" and [t.arguments["address"] for t in r1.tool_calls] == ["B5", "C5"]
    history = [user("q"), r1.to_message(), tool_results([ToolResult("tu_1", "16"), ToolResult("tu_2", "30", True)])]
    c.chat(history)
    sent = beta.calls[1]["messages"]
    assert sent[1]["role"] == "assistant" and sent[1]["content"][1]["type"] == "tool_use"
    # both results in ONE user message
    assert sent[2] == {
        "role": "user",
        "content": [
            {"type": "tool_result", "tool_use_id": "tu_1", "content": "16", "is_error": False},
            {"type": "tool_result", "tool_use_id": "tu_2", "content": "30", "is_error": True},
        ],
    }


def test_refusal_stop_reason():
    refused = msg(
        [{"type": "text", "text": "partial"}], stop="refusal",
        stop_details={"type": "refusal", "category": "cyber", "explanation": "nope"},
    )
    c = connect("anthropic", api_key="k", client=fake_sdk(beta_create=Recorder([refused])))
    r = c.chat("x")
    assert r.stop_reason == "refusal" and r.refusal == {"category": "cyber", "explanation": "nope"}
    assert r.tool_calls == []


def test_no_prefill():
    c = connect("anthropic", api_key="k", client=fake_sdk())
    with pytest.raises(ValueError, match="prefill"):
        c.chat([user("q"), Message(role="assistant", text="The answer is")])


def _rate_limit():
    req = httpx2.Request("POST", "https://api.anthropic.com/v1/messages")
    resp = httpx2.Response(429, request=req, headers={"retry-after": "2"})
    return anthropic.RateLimitError("rate limited", response=resp, body=None)


def test_retry_on_429():
    sleeps = []
    beta = Recorder([_rate_limit(), _rate_limit(), msg([{"type": "text", "text": "ok"}])])
    c = connect("anthropic", api_key="k", client=fake_sdk(beta_create=beta), sleep=sleeps.append)
    assert c.chat("x").text == "ok"
    assert len(beta.calls) == 3 and len(sleeps) == 2 and all(s >= 2 for s in sleeps)


def test_retry_gives_up():
    beta = Recorder([_rate_limit()] * 3)
    c = connect("anthropic", api_key="k", client=fake_sdk(beta_create=beta), max_retries=2, sleep=no_sleep)
    with pytest.raises(anthropic.RateLimitError):
        c.chat("x")


def test_real_sdk_over_mock_transport():
    """The real SDK serializes our request: beta header, fallbacks and output_config reach the wire."""
    seen = {}

    def handler(request):
        seen["headers"] = dict(request.headers)
        seen["body"] = json.loads(request.content)
        seen["url"] = str(request.url)
        return httpx2.Response(200, json={
            "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
            "content": [{"type": "text", "text": "fine"}], "stop_reason": "end_turn", "stop_sequence": None,
            "usage": {"input_tokens": 3, "output_tokens": 1},
        })

    sdk = anthropic.Anthropic(api_key="test-key", http_client=httpx2.Client(transport=httpx2.MockTransport(handler)), max_retries=0)
    c = connect("anthropic", api_key="test-key", client=sdk, effort="low")
    r = c.chat("hi")
    assert r.text == "fine" and r.usage.output_tokens == 1
    assert FALLBACK_BETA in seen["headers"].get("anthropic-beta", "")
    assert seen["body"]["fallbacks"] == "default"
    assert seen["body"]["output_config"] == {"effort": "low"}
    assert seen["body"]["cache_control"] == {"type": "ephemeral"}
    assert "thinking" not in seen["body"]
