# LLM connections

Optional: hedgehog computes everything deterministically, and the LLM only phrases summaries or answers
test questions through tools. The Rust core makes no network or LLM calls. Install a provider with
`pip install 'hedgehog[openai]'`, `[anthropic]`, `[bedrock]` or `[llm]` (all three).

```python
import hedgehog
from hedgehog import llm

client = llm.connect("anthropic", api_key="sk-ant-...")       # the key is a normal argument
client = llm.connect(env_file=".env")                         # or from .env (only when asked to)

hedgehog.summarize("book.xlsx")                               # deterministic markdown, free
s = hedgehog.summarize("book.xlsx", llm=client)               # one call, no tools, ≤ ~3k input tokens
s.text, s.usage, s.warnings, s.llm_error                      # bad citations land in warnings

a = llm.ask("book.xlsx", "What is the 2024 total?", client)   # tool loop: describe_workbook, get_cell,
a.final, a.cited_cells, a.tool_calls_made, a.usage            # get_range, find_text, check_totals
print(llm.bench(cases, llm.connect_available()))              # cases: {file, question, expected, tolerance}
```

## Providers

| `connect(...)` | SDK | chat | tools | `json_schema` | usage | notes |
|---|---|---|---|---|---|---|
| `openai` | `openai` | yes | function calling | `response_format` json_schema (`strict_json=`) | in/out/cached | default model `gpt-4o-mini` (`OPENAI_DEFAULT_MODEL`) |
| `openai_compatible`, `vllm`, `ollama`, `openrouter`, `groq` | `openai` | yes | function calling | json_schema, `strict=False` | in/out | thinking switched off via `extra_body` (vLLM `chat_template_kwargs`, OpenRouter `reasoning`), dropped if the gateway rejects it; `<think>` blocks stripped |
| `anthropic` | `anthropic` ≥ 1.15 | yes | tool use, `tool_choice` auto | `output_config.format` | in/out/cache read/write | default `claude-opus-5-5`; optional `effort=`; top-level `cache_control`; refusal `stop_details`; server-side refusal fallbacks on opus/sonnet-5-5 and fable-5-1 (`fallbacks=False` to opt out) |
| `bedrock` | `boto3` | Converse | toolConfig | via a `structured_output` tool (auto choice) | in/out/cache | bearer token exported as `AWS_BEARER_TOKEN_BEDROCK` |

All providers share `ChatClient.chat(messages, system=, tools=, json_schema=, max_tokens=, temperature=)`
→ `ChatResult(text, tool_calls, stop_reason, usage, raw, json, refusal)`, a per-client concurrency limit
(`max_concurrency=4`), a process-wide one (`AI_MAX_CONCURRENT_LLM`), and retries with backoff on 429 / 5xx
/ connection errors (`max_retries=3`, `Retry-After` honoured). `llm.list_models(client)` lists the models
a key can use (OpenAI-style and Anthropic).

## Keys and settings

Order: argument → `os.environ` → `.env` file (only with `env_file=` or `llm.load_env()`) → default → error.
Names follow `.env.example`: `AI_PROVIDER`, `AI_CHAT_MODEL`; `OPENAI_API_KEY/BASE_URL/MODEL`;
`ANTHROPIC_API_KEY/MODEL`; `AI_BEDROCK_BEARER_TOKEN/REGION/MODEL`; `OPENROUTER_*`, `GROQ_*`;
`API_URL/API_TOKEN/API_MODEL` for local servers (`/v1` appended). Without a provider and `AI_PROVIDER`,
the single preset that has credentials is used. `llm.available_providers()` names the configured
presets (never values). Keys never appear in `repr` or logs.

Tests (`py/tests/`) use fake SDK clients and mock HTTP transports only; no test makes a real API call.
