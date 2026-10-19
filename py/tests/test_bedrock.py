"""Bedrock Converse provider against a fake boto3 client."""

from types import SimpleNamespace

import pytest

botocore = pytest.importorskip("botocore")
from botocore.exceptions import ClientError  # noqa: E402

from hedgehog.llm import ToolResult, connect, tool_results, user  # noqa: E402

from conftest import Recorder, no_sleep  # noqa: E402


def resp(content, stop="end_turn"):
    return {
        "output": {"message": {"role": "assistant", "content": content}},
        "stopReason": stop,
        "usage": {"inputTokens": 40, "outputTokens": 9, "totalTokens": 49},
    }


def client(rec, **kw):
    return connect("bedrock", api_key="t", region="eu-west-2", model="qwen.qwen3", client=SimpleNamespace(converse=rec), **kw)


def test_converse_shape_and_tool_round_trip():
    rec = Recorder([
        resp([{"toolUse": {"toolUseId": "tu1", "name": "get_cell", "input": {"sheet": "S", "address": "B5"}}}], "tool_use"),
        resp([{"text": "<think>hm</think>16 (S!B5)"}]),
    ])
    c = client(rec)
    tools = [{"name": "get_cell", "description": "d", "parameters": {"type": "object"}}]
    r = c.chat("q", system="sys", tools=tools, temperature=0.1, max_tokens=500)
    kw = rec.calls[0]
    assert kw["modelId"] == "qwen.qwen3" and kw["system"] == [{"text": "sys"}]
    assert kw["messages"] == [{"role": "user", "content": [{"text": "q"}]}]
    assert kw["toolConfig"] == {"tools": [{"toolSpec": {"name": "get_cell", "description": "d", "inputSchema": {"json": {"type": "object"}}}}]}
    assert kw["inferenceConfig"] == {"maxTokens": 500, "temperature": 0.1}
    assert r.stop_reason == "tool_use" and r.tool_calls[0].id == "tu1"
    r2 = c.chat([user("q"), r.to_message(), tool_results([ToolResult("tu1", "16", is_error=True)])], tools=tools)
    sent = rec.calls[1]["messages"]
    assert sent[1]["content"][0]["toolUse"]["toolUseId"] == "tu1"
    assert sent[2]["content"] == [{"toolResult": {"toolUseId": "tu1", "content": [{"text": "16"}], "status": "error"}}]
    assert r2.text == "16 (S!B5)" and r2.usage.input_tokens == 40


def test_json_schema_via_tool():
    schema = {"type": "object", "properties": {"a": {"type": "number"}}}
    rec = Recorder([resp([{"toolUse": {"toolUseId": "x", "name": "structured_output", "input": {"a": 5}}}], "tool_use")])
    r = client(rec).chat("x", json_schema=schema)
    assert r.json == {"a": 5} and r.tool_calls == [] and r.stop_reason == "end_turn"
    assert rec.calls[0]["toolConfig"]["tools"][0]["toolSpec"]["name"] == "structured_output"
    assert "toolChoice" not in rec.calls[0]["toolConfig"]


def test_guardrail_is_refusal():
    r = client(Recorder([resp([{"text": "blocked"}], "guardrail_intervened")])).chat("x")
    assert r.stop_reason == "refusal" and r.refusal["category"] == "guardrail_intervened"


def test_retry_on_throttling():
    throttled = ClientError({"Error": {"Code": "ThrottlingException", "Message": "slow down"},
                             "ResponseMetadata": {"HTTPStatusCode": 429}}, "Converse")
    denied = ClientError({"Error": {"Code": "AccessDeniedException", "Message": "no"},
                          "ResponseMetadata": {"HTTPStatusCode": 403}}, "Converse")
    rec = Recorder([throttled, resp([{"text": "ok"}])])
    assert client(rec, sleep=no_sleep).chat("x").text == "ok"
    with pytest.raises(ClientError):
        client(Recorder([denied]), sleep=no_sleep).chat("x")
