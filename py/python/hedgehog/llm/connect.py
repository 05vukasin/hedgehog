"""``connect()``: build a ChatClient from explicit arguments, with environment variables as fallback.

Resolution order for every setting: function argument → ``os.environ`` → ``.env`` file (only when
``env_file=`` is passed; nothing is loaded automatically) → preset default → clear error.
Variable names match ``.env.example``:

| preset              | key                       | base URL                          | model                          |
|---------------------|---------------------------|-----------------------------------|--------------------------------|
| ``openai``          | OPENAI_API_KEY            | OPENAI_BASE_URL (optional)        | OPENAI_MODEL / gpt-4o-mini     |
| ``openai_compatible``| OPENAI_API_KEY (optional on localhost) | OPENAI_BASE_URL (required) | OPENAI_MODEL (required) |
| ``openrouter``      | OPENROUTER_API_KEY        | OPENROUTER_BASE_URL / openrouter.ai | OPENROUTER_MODEL (required)  |
| ``groq``            | GROQ_API_KEY              | GROQ_BASE_URL / api.groq.com      | GROQ_MODEL (required)          |
| ``vllm`` (local, ollama, lmstudio) | API_TOKEN (optional) | API_URL (+``/v1``)       | API_MODEL (required)           |
| ``anthropic``       | ANTHROPIC_API_KEY         | –                                 | ANTHROPIC_MODEL / claude-opus-5-5 |
| ``bedrock``         | AI_BEDROCK_BEARER_TOKEN   | AI_BEDROCK_REGION (region)        | AI_BEDROCK_MODEL (required)    |

``AI_PROVIDER`` picks the preset when none is passed; if it is unset too and exactly one preset has
credentials (e.g. only OPENAI_API_KEY), that one is used. ``AI_CHAT_MODEL`` is a last model fallback,
used only when ``AI_PROVIDER`` is unset or names the same preset (so a Bedrock id never leaks into an
Anthropic client). ``AI_MAX_CONCURRENT_LLM`` caps concurrent calls process-wide. Key values are never
logged or shown in ``repr``.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, List, Mapping, Optional, Union
from urllib.parse import urlparse

from .base import ChatClient, set_max_concurrent
from .env import Env, PathLike, read_env
from .types import ConfigError

# Small, cheap default for the plain "openai" preset (summaries only phrase precomputed facts).
# Change here; list_models(client) shows what the key can use.
OPENAI_DEFAULT_MODEL = "gpt-4o-mini"

PROVIDERS = ("openai", "openai_compatible", "anthropic", "bedrock")
PRESETS = ("openai", "openai_compatible", "anthropic", "bedrock", "openrouter", "groq", "vllm", "ollama")

THINKING_OFF_VLLM = {"chat_template_kwargs": {"enable_thinking": False}}
THINKING_OFF_OPENROUTER = {"reasoning": {"enabled": False}}


@dataclass(frozen=True)
class Preset:
    name: str
    kind: str  # openai | openai_compatible | anthropic | bedrock
    key_env: str = ""
    url_env: str = ""
    model_env: str = ""
    default_url: Optional[str] = None
    key_optional: bool = False  # local servers
    thinking_off: Optional[Dict[str, Any]] = None
    append_v1: bool = False


_PRESETS: Dict[str, Preset] = {
    "openai": Preset("openai", "openai", "OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENAI_MODEL"),
    "openai_compatible": Preset(
        "openai_compatible", "openai_compatible", "OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENAI_MODEL",
        thinking_off=THINKING_OFF_VLLM,
    ),
    "openrouter": Preset(
        "openrouter", "openai_compatible", "OPENROUTER_API_KEY", "OPENROUTER_BASE_URL", "OPENROUTER_MODEL",
        default_url="https://openrouter.ai/api/v1", thinking_off=THINKING_OFF_OPENROUTER,
    ),
    "groq": Preset(
        "groq", "openai_compatible", "GROQ_API_KEY", "GROQ_BASE_URL", "GROQ_MODEL",
        default_url="https://api.groq.com/openai/v1",
    ),
    "vllm": Preset(
        "vllm", "openai_compatible", "API_TOKEN", "API_URL", "API_MODEL",
        key_optional=True, thinking_off=THINKING_OFF_VLLM, append_v1=True,
    ),
    "ollama": Preset(
        "ollama", "openai_compatible", "API_TOKEN", "API_URL", "API_MODEL",
        default_url="http://localhost:11434/v1", key_optional=True, thinking_off=THINKING_OFF_VLLM, append_v1=True,
    ),
    "anthropic": Preset("anthropic", "anthropic", "ANTHROPIC_API_KEY", "", "ANTHROPIC_MODEL"),
    "bedrock": Preset("bedrock", "bedrock", "AI_BEDROCK_BEARER_TOKEN", "AI_BEDROCK_REGION", "AI_BEDROCK_MODEL"),
}
_ALIASES = {
    "openai-compatible": "openai_compatible",
    "compatible": "openai_compatible",
    "local": "vllm",
    "localmodels": "vllm",
    "lmstudio": "vllm",
    "lm_studio": "vllm",
    "claude": "anthropic",
    "aws": "bedrock",
    "aws_bedrock": "bedrock",
}


def _preset(name: str) -> Preset:
    key = name.strip().lower().replace(" ", "_")
    key = _ALIASES.get(key, key)
    if key not in _PRESETS:
        raise ConfigError(f"unknown LLM provider {name!r}; use one of {PRESETS}")
    return _PRESETS[key]


def _is_local(url: str) -> bool:
    host = (urlparse(url).hostname or "").lower()
    return host in ("localhost", "127.0.0.1", "0.0.0.0", "::1") or host.endswith(".local")


def _with_v1(url: str) -> str:
    base = url.rstrip("/")
    return base if base.endswith("/v1") else base + "/v1"


def _env_for(env_file: Optional[PathLike], env: Optional[Mapping[str, str]] = None) -> Env:
    if env is not None:
        return Env(env, use_os=False)
    return Env(read_env(env_file) if env_file is not None else {})


def connect(
    provider: Optional[str] = None,
    *,
    api_key: Optional[str] = None,
    model: Optional[str] = None,
    base_url: Optional[str] = None,
    region: Optional[str] = None,
    env_file: Optional[PathLike] = None,
    **opts: Any,
) -> ChatClient:
    """Connect to an LLM. The API key is a normal argument; env vars are only the fallback.

    >>> llm = connect("anthropic", api_key="sk-ant-...")                       # claude-opus-5-5
    >>> llm = connect("openai", api_key="sk-...", model="gpt-...")
    >>> llm = connect("vllm", base_url="http://localhost:8000", model="Qwen/Qwen3-32B")
    >>> llm = connect("openrouter", model="qwen/qwen3-235b-a22b")             # OPENROUTER_API_KEY
    >>> llm = connect("bedrock", api_key="...", region="eu-west-2", model="qwen.qwen3-...")
    >>> llm = connect(env_file=".env")                                          # AI_PROVIDER etc.

    Common ``opts``: ``max_concurrency`` (per client, 4), ``max_retries`` (3), ``timeout``; Anthropic:
    ``effort``, ``fallbacks`` (``"default"``, ``False`` or a list), ``cache``; OpenAI-style:
    ``strict_json``, ``disable_thinking``, ``extra_body``, ``max_tokens_param``; Bedrock:
    ``use_aws_credentials``. ``client=`` injects a prebuilt SDK client (tests, custom transports).
    """
    env = _env_for(env_file, opts.pop("env", None))
    env_provider = env.get("AI_PROVIDER")
    name = provider or env_provider
    if not name:
        configured = [n for n in _available(env) if n != "openai_compatible"]
        if len(configured) == 1:
            name = configured[0]
        else:
            found = f" (credentials found for: {', '.join(configured)})" if configured else ""
            raise ConfigError(
                f"no LLM provider given: pass connect(provider, ...) or set AI_PROVIDER to one of {PRESETS}{found}"
            )
    p = _preset(name)
    same_as_env = env_provider is not None and _preset(env_provider).name == p.name
    model = model or env.get(p.model_env) or (env.get("AI_CHAT_MODEL") if env_provider is None or same_as_env else None)
    injected = opts.get("client") is not None

    limit = env.get("AI_MAX_CONCURRENT_LLM")
    if limit and limit.isdigit():
        set_max_concurrent(int(limit))

    if p.kind == "anthropic":
        from .anthropic_provider import DEFAULT_MODEL, AnthropicClient  # noqa: PLC0415

        key = api_key or env.get(p.key_env)
        if not key and not injected:
            raise ConfigError("Anthropic needs an API key: connect('anthropic', api_key=...) or set ANTHROPIC_API_KEY")
        return AnthropicClient(api_key=key or "", model=model or DEFAULT_MODEL, base_url=base_url, **opts)

    if p.kind in ("openai", "openai_compatible"):
        from .openai_provider import OpenAIClient  # noqa: PLC0415

        url = base_url or env.get(p.url_env) or p.default_url
        if url and p.append_v1:
            url = _with_v1(url)
        compatible = p.kind == "openai_compatible"
        if compatible and not url:
            raise ConfigError(
                f"{p.name} needs a base URL: connect({p.name!r}, base_url='http://host:8000/v1', ...) or set {p.url_env}"
            )
        key = api_key or env.get(p.key_env)
        if not key and compatible and url and (p.key_optional or _is_local(url)):
            key = "not-needed"  # local servers accept any key; the SDK insists on one
        if not key and not injected:
            raise ConfigError(f"{p.name} needs an API key: connect({p.name!r}, api_key=...) or set {p.key_env}")
        if not model and p.name == "openai":
            model = OPENAI_DEFAULT_MODEL
        if not model:
            raise ConfigError(f"{p.name} needs a model: connect({p.name!r}, model=...) or set {p.model_env}")
        if compatible and "disable_thinking" not in opts:
            opts["disable_thinking"] = p.thinking_off is not None
        if p.thinking_off is not None and "thinking_extra" not in opts:
            opts["thinking_extra"] = p.thinking_off
        client = OpenAIClient(api_key=key or "", model=model, base_url=url, compatible=compatible, **opts)
        client.preset = p.name
        return client

    # bedrock
    from .bedrock_provider import BedrockClient  # noqa: PLC0415

    use_aws = bool(opts.pop("use_aws_credentials", False))
    key = api_key or env.get(p.key_env) or env.get("AWS_BEARER_TOKEN_BEDROCK")
    if not key and not use_aws and not injected:
        raise ConfigError(
            "Bedrock needs an API key (bearer token): connect('bedrock', api_key=...) or set AI_BEDROCK_BEARER_TOKEN; "
            "pass use_aws_credentials=True to use the normal AWS credential chain instead"
        )
    reg = region or env.get(p.url_env) or env.get("AWS_REGION") or env.get("AWS_DEFAULT_REGION")
    if not reg:
        raise ConfigError("Bedrock needs a region: connect('bedrock', region=...) or set AI_BEDROCK_REGION")
    if not model:
        raise ConfigError("Bedrock needs a model id: connect('bedrock', model=...) or set AI_BEDROCK_MODEL")
    return BedrockClient(api_key=key, model=model, region=reg, **opts)


def available_providers(env: Union[None, PathLike, Mapping[str, str]] = None) -> List[str]:
    """Presets that have credentials configured (names only, never values).

    ``env``: ``None`` → ``os.environ`` plus the default ``.env`` file if one is found; a path → that
    file plus ``os.environ``; a mapping → only that mapping. A preset counts when its key is set (for
    ``vllm``: its URL); whether a model is configured is not checked — see :func:`connect_available`.
    """
    if env is None:
        e = Env(read_env())
    elif isinstance(env, Mapping):
        e = Env(env, use_os=False)
    else:
        e = Env(read_env(env))
    return _available(e)


def _available(e: Env) -> List[str]:
    out = []
    for name in ("openai", "anthropic", "bedrock", "openrouter", "groq", "vllm"):
        p = _PRESETS[name]
        if name == "vllm":
            ok = e.get(p.url_env) is not None
        elif name == "bedrock":
            ok = e.get(p.key_env) is not None or e.get("AWS_BEARER_TOKEN_BEDROCK") is not None
        else:
            ok = e.get(p.key_env) is not None
        if ok:
            out.append(name)
    if e.get("OPENAI_BASE_URL") and "openai" in out:
        out.append("openai_compatible")
    return out


def connect_available(
    env: Union[None, PathLike, Mapping[str, str]] = None, **opts: Any
) -> Dict[str, ChatClient]:
    """One client per configured preset (``{"anthropic:claude-opus-5-5": client, ...}``), skipping
    presets whose configuration is incomplete (e.g. no model) — for ``bench`` over every model."""
    if env is None:
        mapping: Optional[Mapping[str, str]] = None
        env_file: Optional[PathLike] = None
    elif isinstance(env, Mapping):
        mapping, env_file = env, None
    else:
        mapping, env_file = None, env
    clients: Dict[str, ChatClient] = {}
    for name in available_providers(env):
        try:
            kwargs = dict(opts)
            if mapping is not None:
                kwargs["env"] = mapping
            if name == "openai_compatible":
                continue  # same key/model variables as "openai"; connect it explicitly if wanted
            c = connect(name, env_file=env_file if env_file is not None else _default_file(), **kwargs)
        except ConfigError:
            continue
        clients[c.name] = c
    return clients


def _default_file() -> Optional[PathLike]:
    from .env import find_env_file  # noqa: PLC0415

    return find_env_file()
