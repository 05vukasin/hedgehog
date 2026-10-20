# LLMs (optional)

How to let a language model phrase summaries or answer questions about a workbook, which providers
work, where API keys come from, and what it costs.

hedgehog does not need an LLM. Table detection, value parsing, totals verification, normalization
and the deterministic summary are all computed in Rust without network access. The optional
`hedgehog.llm` package adds:

- [`summarize(..., llm=client)`](#summaries): one call that turns precomputed facts into a short
  summary, with every cited cell checked afterwards;
- [`llm.ask`](#asking-questions-with-tools): a tool loop in which a model answers a question by
  reading cells through five workbook tools;
- [`llm.bench`](#comparing-models-with-bench): the same questions on several models, scored.

Contents: [Install](#install) · [Connecting](#connecting) · [API keys and settings](#api-keys-and-settings) ·
[Providers](#providers) · [Summaries](#summaries) · [Asking questions with tools](#asking-questions-with-tools) ·
[Comparing models with bench](#comparing-models-with-bench) · [The ChatClient interface](#the-chatclient-interface) ·
[Retries, rate limits and quotas](#retries-rate-limits-and-quotas) · [Cost](#cost) · [Errors](#errors)

## Install

Each provider needs its SDK, available as an extra:

```sh
pip install "hedgehog-sheets[openai]"      # OpenAI and every OpenAI-compatible server
pip install "hedgehog-sheets[anthropic]"   # Anthropic
pip install "hedgehog-sheets[bedrock]"     # AWS Bedrock (boto3)
pip install "hedgehog-sheets[llm]"         # all three
```

Until the package is on PyPI, add the `--find-links` option from [installation.md](installation.md).

## Connecting

`llm.connect(provider, ...)` returns a `ChatClient`. The API key is an ordinary argument:

```python
from hedgehog import llm

client = llm.connect("openai", api_key="sk-...")                        # model gpt-4o-mini by default
client = llm.connect("anthropic", api_key="sk-ant-...")                 # model claude-opus-5-5 by default
client = llm.connect("openrouter", api_key="sk-or-...", model="qwen/qwen3-235b-a22b")
client = llm.connect("vllm", base_url="http://localhost:8000", model="Qwen/Qwen3-32B")
client = llm.connect("ollama", model="llama3.1")                        # http://localhost:11434/v1
client = llm.connect("bedrock", api_key="...", region="eu-west-2", model="qwen.qwen3-235b-a22b-2507-v1:0")
```

Signature:

```
connect(provider=None, *, api_key=None, model=None, base_url=None, region=None, env_file=None, **opts) -> ChatClient
```

Common `opts`: `max_concurrency` (per client, default 4), `max_retries` (default 3), `timeout`.
Anthropic: `effort` (`"low"` to `"max"`), `fallbacks` (`"default"`, `False` or a list), `cache`.
OpenAI-style: `strict_json`, `disable_thinking`, `extra_body`, `max_tokens_param`. Bedrock:
`use_aws_credentials=True` to use the normal AWS credential chain instead of a bearer token.
`client=` injects a prebuilt SDK client (tests, custom transports).

## API keys and settings

Every setting is resolved in this order:

1. the argument to `connect` (`api_key=`, `model=`, `base_url=`, `region=`);
2. the environment variable (`os.environ`);
3. a `.env` file, only when you pass `env_file=".env"` or call `llm.load_env()` first;
4. the preset's default (default model, default URL);
5. otherwise a `ConfigError` that names the argument and the variable to set.

```python
from hedgehog import llm

client = llm.connect("openai")                     # OPENAI_API_KEY from the environment
client = llm.connect("openai", env_file=".env")    # or from a .env file
client = llm.connect(env_file=".env")              # provider from AI_PROVIDER in the file
```

Without a provider argument, `connect` uses `AI_PROVIDER`; if that is unset too and exactly one
preset has credentials, that one is used. Variable names (see `.env.example` in the repository):

| Preset | API key | Base URL | Model |
|---|---|---|---|
| `openai` | `OPENAI_API_KEY` | `OPENAI_BASE_URL` (optional) | `OPENAI_MODEL`, default `gpt-4o-mini` |
| `openai_compatible` | `OPENAI_API_KEY` (optional for localhost) | `OPENAI_BASE_URL` (required) | `OPENAI_MODEL` (required) |
| `openrouter` | `OPENROUTER_API_KEY` | `OPENROUTER_BASE_URL`, default `https://openrouter.ai/api/v1` | `OPENROUTER_MODEL` (required) |
| `groq` | `GROQ_API_KEY` | `GROQ_BASE_URL`, default `https://api.groq.com/openai/v1` | `GROQ_MODEL` (required) |
| `vllm` (also `local`, `lmstudio`) | `API_TOKEN` (optional) | `API_URL`, `/v1` appended | `API_MODEL` (required) |
| `ollama` | `API_TOKEN` (optional) | `API_URL`, default `http://localhost:11434/v1` | `API_MODEL` (required) |
| `anthropic` (also `claude`) | `ANTHROPIC_API_KEY` | | `ANTHROPIC_MODEL`, default `claude-opus-5-5` |
| `bedrock` (also `aws`) | `AI_BEDROCK_BEARER_TOKEN` (or `AWS_BEARER_TOKEN_BEDROCK`) | region: `AI_BEDROCK_REGION`, `AWS_REGION` | `AI_BEDROCK_MODEL` (required) |

Also: `AI_PROVIDER` (default preset), `AI_CHAT_MODEL` (last model fallback, used only when
`AI_PROVIDER` is unset or names the same preset) and `AI_MAX_CONCURRENT_LLM` (process-wide cap on
concurrent calls).

Key values never appear in `repr(client)` or in logs. `llm.available_providers()` lists the presets
that have credentials (names only); unlike `connect`, it and `llm.connect_available()` also look for
a `.env` file in the current directory (or the repository root in a development checkout).

```python
from hedgehog import llm

print(llm.PRESETS)
```

```text
('openai', 'openai_compatible', 'anthropic', 'bedrock', 'openrouter', 'groq', 'vllm')
```

(`ollama` is a preset like `vllm`, pointed at the local Ollama server.)

## Providers

| Provider | SDK | Tools | Structured output | Usage reported | Notes |
|---|---|---|---|---|---|
| `openai` | `openai` | function calling | `response_format` JSON schema (`strict_json=`) | input, output, cached | |
| `openai_compatible`, `vllm`, `ollama`, `openrouter`, `groq` | `openai` | function calling | JSON schema, non-strict | input, output | "Thinking" is switched off where the server supports it (vLLM `chat_template_kwargs`, OpenRouter `reasoning`) and dropped if the server rejects it; `<think>` blocks are stripped from replies. |
| `anthropic` | `anthropic` 1.15 or newer | tool use, automatic choice | `output_config.format` | input, output, cache read/write | Models `claude-opus-5-5` (default), `claude-sonnet-5-5`, `claude-haiku-5-5`, `claude-fable-5-1`. Optional `effort`; prompt caching of the stable prefix; refusals reported in `ChatResult.refusal`. |
| `bedrock` | `boto3` | Converse `toolConfig` | through a `structured_output` tool | input, output, cache | The bearer token is set on the client's own botocore session; `os.environ` is not modified. |

`llm.list_models(client)` lists the model ids a key can use (OpenAI-style and Anthropic endpoints).

## Summaries

```python
import hedgehog
from hedgehog import llm

client = llm.connect("openai", env_file=".env")
s = hedgehog.summarize("sales.xlsx", llm=client)
print(s.source, s.model, s.usage)
print(s.warnings)
print(s.text)
```

Output of one real run (gpt-4o-mini, sample from the [quickstart](quickstart.md#a-sample-messy-workbook);
the wording differs from run to run):

```text
llm gpt-4o-mini-2024-07-18 Usage(input_tokens=801, output_tokens=172, cache_read_tokens=0, cache_write_tokens=0, calls=1)
['numbers without a cell citation: 4600, 4525.5']
The workbook contains one sheet titled 'Sales' with a range of A1:E15, featuring a sales report with headers for Product, Revenue, Units, and Margin.

There is one mismatched total: the Grand total in cell Sales!B13 shows a value of 4600, but it should be 4525.5 based on the range B6:B12, indicating a discrepancy in the data.

Key figures include:
- Subtotal for Region North: 1500.5 (Sales!B8)
- Subtotal for Region South: 3025 (Sales!B12)
- Grand total for Revenue: 4600 (Sales!B13)
- Total Revenue for Q1: 4525.5 (B6:B12)
- Total Revenue for Q2: 4630 (Sales!C13)
```

The warning shows the citation check at work: the model wrote 4600 and 4525.5 in a sentence without
a cell reference next to them.

How it works:

1. hedgehog computes the [facts](python-api.md#hedgehogfacts) (sheets, previews, verified totals,
   every value with its cell address) and trims them to `budget_tokens` (default 3000, estimated at
   3 characters per token). Mismatched totals are kept first; anything cut is listed under
   `"omitted"` so the model knows the payload is partial.
2. One `chat` call with no tools: a short system prompt (problems first, at most 5 key figures, cite
   every number as `24,517.83 (Sheet1!G54)`, under `words` words, in `language`), `max_tokens=800`,
   `temperature=0.2`. A model that rejects the temperature is retried once without it.
3. Every citation in the reply is checked against the workbook with
   [`check_citations`](python-api.md#hedgehogcheck_citations). Wrong values, unknown sheets, cells the
   model never saw and numbers without a citation become `warnings`.
4. If the call fails or the reply is empty, `summarize` returns the deterministic summary with
   `llm_error` set instead of raising.

`s.payload` holds exactly what was sent, and `s.deterministic` the free summary, so you can compare
the two.

## Asking questions with tools

`llm.ask` runs a tool loop: the model calls workbook tools until it can answer, then ends with a line
`ANSWER: <value>`.

```python
from hedgehog import llm

client = llm.connect("openai", env_file=".env")
a = llm.ask("sales.xlsx", "What is the Q2 revenue of the South region?", client)
print(a.final, a.cited_cells)
print([c["name"] for c in a.tool_calls_made], a.usage.input_tokens, a.usage.output_tokens)
```

One real run (gpt-4o-mini):

```text
0 ['Sales!C9']
['find_text', 'find_text', 'get_cell'] 1522 100
```

The model found the `Region: South` label, read the empty cell next to it (`Sales!C9`) and answered
0; the right answer is the South subtotal, 2870 in `Sales!C12`. Tool access does not make a small
model read a messy layout correctly. For questions like this, give the agent the
[normalized output](agent-recipes.md) instead, where the answer is
`df[df["Region"] == "South"]["Revenue / Q2"].sum()`.

```
ask(wb, question, client, *, max_turns=8, max_tokens=None, temperature=None, system=None, **chat_options) -> Answer
```

`Answer` fields: `text`, `final` (the value after `ANSWER:`), `tool_calls_made` (name, arguments,
error), `usage`, `cited_cells`, `citations` (a `CitationReport`), `stop_reason` (`end_turn`,
`max_turns`, `refusal`, ...), `turns`, `seconds`, `model`, `refusal`, `messages`.

The tools (`llm.TOOLS`, executed by `llm.WorkbookTools`) are the same for every provider. Results are
compact JSON with cell addresses:

| Tool | Arguments | Returns |
|---|---|---|
| `describe_workbook` | none | The facts: sheets, sizes, small sheets' values, large sheets' first rows, verified totals. |
| `get_cell` | `sheet`, `address` | Value, type and formula of one cell; for a merged cell also the merge range and the value it shows. |
| `get_range` | `sheet`, `range` | Values row by row; at most 400 cells per call. |
| `find_text` | `query`, optional `sheet`, `limit` (default 20) | Cells containing the text (case-, accent- and Cyrillic/Latin-insensitive). |
| `check_totals` | optional `sheet` | Counts per status and the totals (mismatches first) with source ranges, recomputed sum and status: the same `ok` / `mismatch` / `unknown` as the normalize manifest. |

These five tools are the current agent interface; an MCP server exposing them (and more) is
[planned](../README.md#status-and-roadmap).

## Comparing models with bench

```python
from hedgehog import llm

cases = [{"file": "sales.xlsx", "question": "What is the Q1 revenue of Gizmos?", "expected": 2045}]
table = llm.bench(cases, llm.connect("openai", env_file=".env"))
print(table)
```

One real run:

```text
| model | accuracy | correct/cases | avg tool calls | input tok | output tok | bad citations | errors | s |
|---|---|---|---|---|---|---|---|---|
| openai:gpt-4o-mini | 0.0% | 0/1 | 7.0 | 5070 | 318 | 1 | 0 | 9.28 |
```

(An earlier run of the same case on a slightly different sample answered correctly with 4 tool
calls. Run several cases and models before drawing conclusions.)

A case is `{file, question, expected, tolerance}` (optional `id`). Numeric `expected` is compared
with the number on the `ANSWER:` line within `tolerance`; text is matched case-insensitively.
`clients` may be one client, a list, or a dict such as `llm.connect_available()` (one client per
configured preset). `llm.load_cases(path)` reads cases from a JSON list or JSON-lines file. Errors are
recorded per row, not raised. `table.rows` has every result; `table.by_model()` the aggregates.

## The ChatClient interface

All providers share one call:

```
client.chat(messages, *, system=None, tools=None, json_schema=None, max_tokens=None, temperature=None, **options) -> ChatResult
```

`messages` is a string, a `{"role", "content"}` dict, a `llm.Message` or a list of them. `tools` are
`llm.Tool` objects or `{"name", "description", "parameters"}` dicts. With `json_schema`, the parsed
reply is in `result.json`.

`ChatResult` fields: `text`, `tool_calls` (`ToolCall(id, name, arguments)`), `stop_reason`
(`end_turn`, `tool_use`, `max_tokens`, `refusal`, `stop_sequence`), `usage`, `model`, `provider`,
`json`, `refusal`, `raw`. Helpers: `llm.user(text)`, `llm.tool_results(results)`,
`result.to_message()`, `llm.strip_think(text)`.

## Retries, rate limits and quotas

- Rate limits (429), overload and 5xx errors and connection errors are retried up to `max_retries`
  times (default 3) with jittered exponential backoff, honouring `Retry-After`, capped at 30 s. The
  SDKs' own retries are switched off so there is only one retry layer.
- An exhausted quota or credit balance (`insufficient_quota`, `credit_balance_exhausted`,
  `billing_hard_limit_reached`) also arrives as a 429 but is **not** retried, because waiting does
  not help. `summarize` then falls back to the deterministic summary with `llm_error` set; `ask`
  raises, and `bench` records the error in the row.
- Concurrency: at most `max_concurrency` calls in flight per client (default 4) and
  `AI_MAX_CONCURRENT_LLM` (or `llm.set_max_concurrent(n)`) across the process.

## Cost

- `summarize` without `llm` is free.
- `summarize` with `llm` is **one** call per workbook: about `budget_tokens` input tokens at most
  (801 for the sample above) and at most `max_tokens` (800) output tokens. `s.usage` reports the
  actual numbers.
- `ask` makes one call per turn (up to `max_turns`, default 8), and every turn resends the
  conversation, so it usually costs several times a summary.
- `bench` runs `ask` for every case on every client.

No test in the repository calls a real API; they use scripted clients and mocked transports.

## Errors

| Exception | Meaning |
|---|---|
| `llm.ConfigError` (a `ValueError`) | Missing or invalid provider, API key, model, base URL or region. The message says which argument or variable to set. |
| `llm.MissingDependency` (an `ImportError`) | The provider SDK is not installed. Install the matching extra (`hedgehog-sheets[openai]`, ...). |
| `llm.LLMError` | Base class of both. Provider SDK errors (authentication, bad request, exhausted quota) are raised as the SDK's own exception types. |

## See also

- [python-api.md](python-api.md#hedgehogsummarize): `summarize`, `Summary`, `facts`, `check_citations`
- [troubleshooting.md](troubleshooting.md#llm-calls-fail): quota, rate limits and missing extras
- [agent-recipes.md](agent-recipes.md): the normalized output, which needs no LLM at all
