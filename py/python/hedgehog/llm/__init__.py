"""LLM connections for hedgehog: one provider-neutral ChatClient over OpenAI, OpenAI-compatible
servers (vLLM, Ollama, OpenRouter, Groq), Anthropic and AWS Bedrock, plus a tool loop (``ask``) and a
model comparison (``bench``). The Rust core never calls an LLM; everything here is optional and
needs the provider SDK extra (``pip install 'hedgehog-sheets[llm]'``)."""

from .agent import Answer, BenchTable, ask, bench, final_answer, load_cases, score
from .base import ChatClient, list_models, set_max_concurrent
from .connect import OPENAI_DEFAULT_MODEL, PRESETS, PROVIDERS, available_providers, connect, connect_available
from .env import load_env, parse_env, read_env
from .tools import TOOLS, WorkbookTools
from .types import (
    ChatResult,
    ConfigError,
    LLMError,
    Message,
    MissingDependency,
    Tool,
    ToolCall,
    ToolResult,
    Usage,
    strip_think,
    tool_results,
    user,
)

__all__ = [
    "Answer", "BenchTable", "ChatClient", "ChatResult", "ConfigError", "LLMError", "Message",
    "MissingDependency", "OPENAI_DEFAULT_MODEL", "PRESETS", "PROVIDERS", "TOOLS", "Tool", "ToolCall", "ToolResult", "Usage",
    "WorkbookTools", "ask", "list_models", "available_providers", "bench", "connect", "connect_available", "final_answer",
    "load_cases", "load_env", "parse_env", "read_env", "score", "set_max_concurrent", "strip_think",
    "tool_results", "user",
]
