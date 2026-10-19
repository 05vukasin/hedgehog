"""OpenAI / OpenAI-compatible provider against fake SDK objects and a mock httpx2 transport."""

import json
from types import SimpleNamespace

import pytest

openai = pytest.importorskip("openai")
httpx2 = pytest.importorskip("httpx2")

from hedgehog.llm import LLMError, ToolResult, connect, list_models, strip_think, tool_results, user  # noqa: E402

from conftest import Recorder, no_sleep  # noqa: E402


def completion(content=None, tool_calls=None, finish="stop", refusal=None):
    message = {"role": "assistant", "content": content, "refusal": refusal}
    if tool_calls:
        message["tool_calls"] = tool_calls
    return openai.types.chat.ChatCompletion.model_validate({
        "id": "c1", "object": "chat.completion", "created": 0, "model": "m",
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
        "usage": {"prompt_tokens": 50, "completion_tokens": 7, "total_tokens": 57,
                  "prompt_tokens_details": {"cached_tokens": 10}},
    })


def fake_sdk(rec):
    return SimpleNamespace(chat=SimpleNamespace(completions=SimpleNamespace(create=rec)))


def _err(cls, code):
    req = httpx2.Request("POST", "http://x/v1/chat/completions")
    return cls("err", response=httpx2.Response(code, request=req), body=None)


def test_tool_round_trip():
    rec = Recorder([
        completion(tool_calls=[{"id": "call_1", "type": "function",
                                "function": {"name": "get_cell", "arguments": '{"sheet":"S","address":"B5"}'}}],
                   finish="tool_calls"),
        completion("It is 16 (S!B5)."),
    ])
    c = connect("openai", api_key="k", model="gpt-x", client=fake_sdk(rec))
    tools = [{"name": "get_cell", "description": "d", "parameters": {"type": "object"}}]
    r = c.chat("q", system="sys", tools=tools)
    assert r.stop_reason == "tool_use" and r.tool_calls[0].arguments == {"sheet": "S", "address": "B5"}
    kw = rec.calls[0]
    assert kw["messages"][0] == {"role": "system", "content": "sys"}
    assert kw["tools"][0]["function"]["name"] == "get_cell" and kw["tool_choice"] == "auto"
    assert kw["max_completion_tokens"] == 4096 and "extra_body" not in kw
    c.chat([user("q"), r.to_message(), tool_results([ToolResult("call_1", "16")])], tools=tools)
    sent = rec.calls[1]["messages"]
    assert sent[1]["tool_calls"][0]["id"] == "call_1"
    assert sent[2] == {"role": "tool", "tool_call_id": "call_1", "content": "16"}
    assert r.usage.input_tokens == 50 and r.usage.cache_read_tokens == 10


def test_json_schema_and_strict_flag():
    schema = {"type": "object", "properties": {"a": {"type": "number"}}}
    rec = Recorder([completion('```json\n{"a": 1}\n```')])
    c = connect("openai_compatible", base_url="http://gpu:8000/v1", api_key="k", model="qwen", client=fake_sdk(rec))
    r = c.chat("x", json_schema=schema)
    rf = rec.calls[0]["response_format"]
    assert rf["type"] == "json_schema" and rf["json_schema"]["schema"] == schema and rf["json_schema"]["strict"] is False
    assert r.json == {"a": 1}
    rec2 = Recorder([completion('{"a": 2}')])
    c2 = connect("openai", api_key="k", model="m", client=fake_sdk(rec2), strict_json=True)
    c2.chat("x", json_schema=schema)
    assert rec2.calls[0]["response_format"]["json_schema"]["strict"] is True


def test_think_tags_stripped_and_thinking_switch_sent():
    rec = Recorder([completion("<think>let me think\nabout it</think>\nAnswer: 3")])
    c = connect("vllm", base_url="http://localhost:8000", model="qwen", client=fake_sdk(rec))
    assert c.chat("x").text == "Answer: 3"
    assert rec.calls[0]["extra_body"] == {"chat_template_kwargs": {"enable_thinking": False}}
    assert rec.calls[0]["max_tokens"] == 4096
    assert strip_think("reasoning only</think> final") == "final"
    assert strip_think("<think>never closed") == ""


def test_thinking_switch_dropped_when_rejected():
    rec = Recorder([_err(openai.BadRequestError, 400), completion("ok"), completion("again")])
    c = connect("openrouter", api_key="k", model="m", client=fake_sdk(rec))
    assert c.chat("x").text == "ok"
    assert rec.calls[0]["extra_body"] == {"reasoning": {"enabled": False}}
    assert "extra_body" not in rec.calls[1]
    c.chat("y")
    assert "extra_body" not in rec.calls[2]  # remembered


def test_refusal():
    rec = Recorder([completion(None, refusal="I can't help with that")])
    r = connect("openai", api_key="k", model="m", client=fake_sdk(rec)).chat("x")
    assert r.stop_reason == "refusal" and r.refusal["explanation"] == "I can't help with that"


def test_retry_on_429_and_5xx():
    rec = Recorder([_err(openai.RateLimitError, 429), _err(openai.InternalServerError, 503), completion("ok")])
    c = connect("openai", api_key="k", model="m", client=fake_sdk(rec), sleep=no_sleep)
    assert c.chat("x").text == "ok" and len(rec.calls) == 3


def test_no_retry_on_400():
    rec = Recorder([_err(openai.BadRequestError, 400)])
    c = connect("openai", api_key="k", model="m", client=fake_sdk(rec), sleep=no_sleep)
    with pytest.raises(openai.BadRequestError):
        c.chat("x")


def test_unknown_model_message():
    rec = Recorder([_err(openai.NotFoundError, 404)])
    c = connect("openai", api_key="k", client=fake_sdk(rec))
    with pytest.raises(LLMError, match="list_models"):
        c.chat("x")


def test_real_sdk_over_mock_transport():
    seen = []

    def handler(request):
        if request.url.path.endswith("/models"):
            return httpx2.Response(200, json={"object": "list", "data": [
                {"id": "gpt-b", "object": "model", "created": 0, "owned_by": "x"},
                {"id": "gpt-a", "object": "model", "created": 0, "owned_by": "x"}]})
        seen.append((dict(request.headers), json.loads(request.content)))
        return httpx2.Response(200, json=completion("hello").model_dump())

    sdk = openai.OpenAI(api_key="sk-test", base_url="http://gw/v1",
                        http_client=httpx2.Client(transport=httpx2.MockTransport(handler)), max_retries=0)
    c = connect("openai", api_key="sk-test", client=sdk)
    assert c.chat("hi", temperature=0.2).text == "hello"
    headers, body = seen[0]
    assert headers["authorization"] == "Bearer sk-test"
    assert body["model"] == "gpt-4o-mini" and body["temperature"] == 0.2
    assert list_models(c) == ["gpt-a", "gpt-b"]
