import os

import pytest

from hedgehog.llm import ConfigError, available_providers, connect, connect_available, load_env, parse_env
from hedgehog.llm.anthropic_provider import AnthropicClient
from hedgehog.llm.bedrock_provider import BedrockClient
from hedgehog.llm.openai_provider import OpenAIClient

FAKE = object()  # injected SDK client: no network, no SDK construction


def test_parse_env():
    text = """
# comment
export A=1
B = "two words"
C='single # not comment'
D=plain # trailing comment
EMPTY=
QUOTED_EMPTY=""
bad line
E="line\\nbreak"
"""
    env = parse_env(text)
    assert env == {"A": "1", "B": "two words", "C": "single # not comment", "D": "plain", "E": "line\nbreak"}


def test_load_env_does_not_override(tmp_path, monkeypatch):
    (tmp_path / ".env").write_text("OPENAI_API_KEY=from-file\nANTHROPIC_API_KEY=file-ant\n")
    monkeypatch.setenv("OPENAI_API_KEY", "from-os")
    names = load_env()
    assert names == ["ANTHROPIC_API_KEY"]
    assert os.environ["OPENAI_API_KEY"] == "from-os"
    assert load_env(override=True) == ["OPENAI_API_KEY", "ANTHROPIC_API_KEY"]
    assert os.environ["OPENAI_API_KEY"] == "from-file"


def test_resolution_order(tmp_path, monkeypatch):
    env_file = tmp_path / "x.env"
    env_file.write_text("ANTHROPIC_API_KEY=file-key\nANTHROPIC_MODEL=claude-haiku-5-5\n")
    # .env only
    c = connect("anthropic", env_file=env_file, client=FAKE)
    assert c.model == "claude-haiku-5-5"
    # os.environ beats .env
    monkeypatch.setenv("ANTHROPIC_MODEL", "claude-sonnet-5-5")
    assert connect("anthropic", env_file=env_file, client=FAKE).model == "claude-sonnet-5-5"
    # argument beats both
    assert connect("anthropic", model="claude-fable-5-1", env_file=env_file, client=FAKE).model == "claude-fable-5-1"
    # no auto-loading without env_file
    with pytest.raises(ConfigError, match="ANTHROPIC_API_KEY"):
        connect("anthropic")


def test_key_from_argument_vs_env(monkeypatch):
    import anthropic

    seen = {}

    class FakeAnthropic:
        def __init__(self, **kw):
            seen.update(kw)

    monkeypatch.setattr(anthropic, "Anthropic", FakeAnthropic)
    monkeypatch.setenv("ANTHROPIC_API_KEY", "env-key")
    connect("anthropic")
    assert seen["api_key"] == "env-key" and seen["max_retries"] == 0
    c = connect("anthropic", api_key="arg-key")
    assert seen["api_key"] == "arg-key"
    assert "arg-key" not in repr(c) and c.model == "claude-opus-5-5"


def test_missing_key_and_model_errors(monkeypatch):
    with pytest.raises(ConfigError, match="OPENAI_API_KEY"):
        connect("openai", model="m")
    with pytest.raises(ConfigError, match="OPENROUTER_MODEL"):
        connect("openrouter", api_key="k")
    with pytest.raises(ConfigError, match="base URL"):
        connect("openai_compatible", api_key="k", model="m")
    with pytest.raises(ConfigError, match="AI_BEDROCK_BEARER_TOKEN"):
        connect("bedrock", model="m", region="eu-west-2")
    with pytest.raises(ConfigError, match="region"):
        connect("bedrock", api_key="t", model="m")
    with pytest.raises(ConfigError, match="AI_PROVIDER"):
        connect()
    with pytest.raises(ConfigError, match="unknown"):
        connect("gemini")


def test_presets():
    env = {
        "OPENROUTER_API_KEY": "or", "OPENROUTER_MODEL": "qwen/qwen3",
        "GROQ_API_KEY": "gq", "GROQ_MODEL": "llama",
        "API_URL": "http://localhost:8000", "API_MODEL": "Qwen/Qwen3-32B",
    }
    orc = connect("openrouter", env=env, client=FAKE)
    assert isinstance(orc, OpenAIClient) and orc.provider == "openai_compatible"
    assert orc.base_url == "https://openrouter.ai/api/v1"
    assert orc.thinking_extra == {"reasoning": {"enabled": False}}
    assert orc.name == "openrouter:qwen/qwen3"
    gq = connect("groq", env=env, client=FAKE)
    assert gq.base_url == "https://api.groq.com/openai/v1" and gq.thinking_extra is None
    vl = connect("vllm", env=env, client=FAKE)
    assert vl.base_url == "http://localhost:8000/v1"  # /v1 appended
    assert vl.thinking_extra == {"chat_template_kwargs": {"enable_thinking": False}}
    assert vl.max_tokens_param == "max_tokens"
    # LocalModels (sada naming) maps to vllm; no key needed for a local server
    assert connect("LocalModels", env=env, client=FAKE).preset == "vllm"
    oa = connect("openai", api_key="k", model="gpt-x", client=FAKE)
    assert oa.thinking_extra is None and oa.max_tokens_param == "max_completion_tokens"


def test_ai_provider_and_chat_model(monkeypatch):
    monkeypatch.setenv("AI_PROVIDER", "bedrock")
    monkeypatch.setenv("AI_CHAT_MODEL", "qwen.qwen3-235b")
    monkeypatch.setenv("AI_BEDROCK_BEARER_TOKEN", "tok")
    monkeypatch.setenv("AI_BEDROCK_REGION", "eu-west-2")
    c = connect(client=FAKE)
    assert isinstance(c, BedrockClient) and c.model == "qwen.qwen3-235b" and c.region == "eu-west-2"
    # AI_CHAT_MODEL belongs to bedrock; it must not leak into an Anthropic client
    a = connect("anthropic", api_key="k", client=FAKE)
    assert isinstance(a, AnthropicClient) and a.model == "claude-opus-5-5"


def test_bedrock_key_exported_for_boto3(monkeypatch):
    import boto3

    seen = {}

    def fake_client(service, **kw):
        seen["service"] = service
        seen["token"] = os.environ.get("AWS_BEARER_TOKEN_BEDROCK")
        seen.update(kw)
        return FAKE

    monkeypatch.setattr(boto3, "client", fake_client)
    connect("bedrock", api_key="bedrock-key", region="eu-west-2", model="m")
    assert seen["service"] == "bedrock-runtime" and seen["token"] == "bedrock-key"
    assert seen["region_name"] == "eu-west-2"


def test_available_providers(tmp_path, monkeypatch):
    env = {"ANTHROPIC_API_KEY": "a", "API_URL": "http://localhost:8000", "GROQ_API_KEY": ""}
    assert available_providers(env) == ["anthropic", "vllm"]
    (tmp_path / ".env").write_text("OPENAI_API_KEY=x\nAI_BEDROCK_BEARER_TOKEN=y\n")
    monkeypatch.setenv("OPENROUTER_API_KEY", "z")
    assert available_providers() == ["openai", "bedrock", "openrouter"]


def test_connect_available_skips_incomplete(monkeypatch):
    import anthropic

    monkeypatch.setattr(anthropic, "Anthropic", lambda **kw: FAKE)
    env = {"ANTHROPIC_API_KEY": "a", "GROQ_API_KEY": "g"}  # groq has no model → skipped
    clients = connect_available(env)
    assert list(clients) == ["anthropic:claude-opus-5-5"]


def test_single_configured_provider_is_used(tmp_path, monkeypatch):
    import openai

    seen = {}
    monkeypatch.setattr(openai, "OpenAI", lambda **kw: seen.update(kw) or FAKE)
    # only OPENAI_API_KEY, no AI_PROVIDER, no model → openai with the default small model
    (tmp_path / ".env").write_text("OPENAI_API_KEY=sk-test\nAI_PROVIDER=\nOPENAI_MODEL=\n")
    c = connect(env_file=".env")
    assert c.name == "openai:gpt-4o-mini" and seen["api_key"] == "sk-test"
    monkeypatch.setenv("AI_CHAT_MODEL", "gpt-other")
    assert connect(env_file=".env").model == "gpt-other"
    monkeypatch.setenv("OPENAI_MODEL", "gpt-specific")
    assert connect(env_file=".env").model == "gpt-specific"
    # two configured providers and no AI_PROVIDER → ambiguous, names (not values) in the error
    monkeypatch.setenv("ANTHROPIC_API_KEY", "secret-value")
    with pytest.raises(ConfigError) as e:
        connect(env_file=".env")
    assert "openai, anthropic" in str(e.value) and "secret-value" not in str(e.value)


def test_global_concurrency_from_env(monkeypatch):
    from hedgehog.llm import base

    monkeypatch.setenv("AI_MAX_CONCURRENT_LLM", "2")
    connect("anthropic", api_key="k", client=FAKE)
    assert base._global_limit == 2
