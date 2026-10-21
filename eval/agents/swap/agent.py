"""A deliberately minimal agent loop: OpenAI chat completions with function calling.

Everything that is not the tool set is fixed here, so an arm differs from another only in its tools:
the same system prompt (plus one line that introduces the tools), temperature 0.2, at most 20 model
calls, and the same answer format. The last call is made with tool_choice "none", so every run ends
with an answer.
"""

from __future__ import annotations

import json
import time
import urllib.error
import urllib.request

SYSTEM = """You answer questions about spreadsheet files for an analyst.
{intro}
Work step by step: look at the file's structure before computing, and check your numbers.
When you are done, write the final answer in plain text: answer every question, give each requested
number with its unit, and say briefly how you got it. Do not ask follow-up questions."""

TEMPERATURE = 0.2
MAX_TURNS = 20
URL = "https://api.openai.com/v1/chat/completions"
# USD per 1M tokens: input, cached input, output (list prices; an estimate).
PRICES = {"gpt-4o-mini": (0.15, 0.075, 0.60), "gpt-4o": (2.5, 1.25, 10.0), "gpt-4.1-mini": (0.4, 0.1, 1.6),
          "gpt-4.1": (2.0, 0.5, 8.0), "gpt-4.1-nano": (0.1, 0.025, 0.4)}


def complete(key: str, body: dict, tries: int = 4) -> dict:
    data = json.dumps(body).encode()
    for i in range(tries):
        req = urllib.request.Request(URL, data=data, headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=600) as r:
                return json.loads(r.read())
        except urllib.error.HTTPError as e:
            msg = e.read().decode(errors="replace")[:500]
            if e.code in (429, 500, 502, 503) and i < tries - 1 and "context_length" not in msg:
                time.sleep(10 * (i + 1))
                continue
            raise RuntimeError(f"OpenAI HTTP {e.code}: {msg}") from None
        except (urllib.error.URLError, TimeoutError) as e:
            if i < tries - 1:
                time.sleep(10 * (i + 1))
                continue
            raise RuntimeError(f"OpenAI: {e}") from None


def cost(model: str, inp: int, cached: int, outp: int):
    p = next((v for k, v in sorted(PRICES.items(), key=lambda kv: -len(kv[0])) if model.startswith(k)), None)
    return round(((inp - cached) * p[0] + cached * p[1] + outp * p[2]) / 1e6, 4) if p else None


def run(key: str, model: str, intro: str, user: str, toolset=None, log=None) -> dict:
    """One agent run. `toolset` has .specs (OpenAI tool list) and .call(name, args) -> (text, is_error)."""
    t0 = time.time()
    msgs = [{"role": "system", "content": SYSTEM.format(intro=intro)}, {"role": "user", "content": user}]
    m = {"model": model, "turns": 0, "input_tokens": 0, "cached_tokens": 0, "output_tokens": 0,
         "tool_calls": 0, "tool_errors": 0, "answer": "", "error": False}
    tools = toolset.specs if toolset else []
    try:
        for turn in range(MAX_TURNS):
            body = {"model": model, "messages": msgs, "temperature": TEMPERATURE}
            if tools:
                body |= {"tools": tools, "tool_choice": "none" if turn == MAX_TURNS - 1 else "auto"}
            r = complete(key, body)
            u = r.get("usage", {})
            m["turns"] += 1
            m["input_tokens"] += u.get("prompt_tokens", 0)
            m["cached_tokens"] += (u.get("prompt_tokens_details") or {}).get("cached_tokens", 0)
            m["output_tokens"] += u.get("completion_tokens", 0)
            m["model"] = r.get("model", model)
            msg = r["choices"][0]["message"]
            calls = msg.get("tool_calls") or []
            msgs.append({k: v for k, v in msg.items() if k in ("role", "content", "tool_calls") and v is not None})
            if log:
                log({"turn": turn, "content": msg.get("content"), "tool_calls": calls, "usage": u})
            if not calls:
                m["answer"] = msg.get("content") or ""
                break
            for c in calls:
                m["tool_calls"] += 1
                try:
                    args = json.loads(c["function"]["arguments"] or "{}")
                    text, err = toolset.call(c["function"]["name"], args)
                except json.JSONDecodeError as e:
                    text, err = f"error: arguments are not valid JSON ({e})", True
                except Exception as e:  # a broken tool call is the model's problem to recover from
                    text, err = f"error: {e.__class__.__name__}: {e}", True
                m["tool_errors"] += bool(err)
                if log:
                    log({"tool": c["function"]["name"], "args": c["function"]["arguments"], "result": text, "is_error": err})
                msgs.append({"role": "tool", "tool_call_id": c["id"], "content": text})
    except Exception as e:  # keep the run: metrics so far plus the error
        m["error"] = True
        m["answer"] = m["answer"] or f"[run failed: {e}]"
    m["seconds"] = round(time.time() - t0, 1)
    m["cost_usd"] = cost(m["model"], m["input_tokens"], m["cached_tokens"], m["output_tokens"])
    m["cost_note"] = "estimate (list prices)"
    return m
