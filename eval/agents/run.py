"""Ask hedgehog-cli and Claude Code the same questions about the same file; score and track both.

    python eval/agents/run.py                         # every case, arms hedgehog + claude
    python eval/agents/run.py --cases pt-contracts --arms hedgehog,claude,claude-hh
    python eval/agents/run.py --grade <run id> covid=1 variants=0 --note "..."
    python eval/agents/run.py --report                # rebuild RESULTS.md from results.jsonl

Arms (same prompt text for all):
- hedgehog:  hedgehog-cli one-shot (`hhc PROMPT -o json`), its own model setting (or --hh-model);
- claude:    Claude Code (`claude -p PROMPT --output-format json`) with shell + file tools only;
- claude-hh: Claude Code with hedgehog's tools added over MCP (`hedgehog mcp`).

Each run appends one record to results.jsonl (metrics, answer, automatic checks) and writes the
answer to answers/<case>.<arm>.md; RESULTS.md shows the latest run per case and arm. Checks in
cases.json: `num` (a number in the answer within `tol`, relative), `pct_range` (a percentage in
[lo, hi]), `text_any` (any of the strings), `judge` (graded by a person with --grade).
Run the whole script under scripts/run_capped.sh: Claude's pandas on a 4.5M-cell sheet is heavy.
"""

from __future__ import annotations

import argparse
import ast
import json
import os
import re
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
RESULTS = HERE / "results.jsonl"
GRADES = HERE / "grades.json"
ARMS = ("hedgehog", "claude", "claude-hh")
# USD per 1M tokens: input, cached input, output (list prices; an estimate).
PRICES = {"gpt-4o-mini": (0.15, 0.075, 0.60), "gpt-4o": (2.5, 1.25, 10.0), "gpt-4.1-mini": (0.4, 0.1, 1.6),
          "gpt-4.1": (2.0, 0.5, 8.0)}


# ---------- running ----------

def dotenv() -> dict:
    env = {}
    p = REPO / ".env"
    if p.exists():
        for line in p.read_text().splitlines():
            k, sep, v = line.partition("=")
            if sep and k.strip() and not k.lstrip().startswith("#"):
                env[k.strip()] = v.strip().strip('"').strip("'")
    return env


def first_json(text: str):
    """The JSON document in a CLI's stdout (skipping any log lines before it)."""
    dec = json.JSONDecoder()
    for m in re.finditer(r"^[\[{]", text, re.M):
        try:
            return dec.raw_decode(text[m.start():])[0]
        except json.JSONDecodeError:
            continue
    raise ValueError("no JSON in output:\n" + text[-2000:])


def command(arm: str, prompt: str, args) -> tuple[list[str], dict]:
    env = dict(os.environ)
    if arm == "hedgehog":
        env.update({k: v for k, v in dotenv().items() if k.endswith("_API_KEY")})
        cmd = [args.hhc, prompt, "-o", "json"] + (["-m", args.hh_model] if args.hh_model else [])
        return cmd, env
    tools = ["Bash", "Read", "Glob", "Grep"]
    cmd = [args.claude, "-p", prompt, "--output-format", "json", "--max-turns", "80", "--max-budget-usd", str(args.max_usd)]
    if args.claude_model:
        cmd += ["--model", args.claude_model]
    if arm == "claude-hh":
        cfg = HERE / "runs" / "mcp.json"
        cfg.parent.mkdir(exist_ok=True)
        cfg.write_text(json.dumps({"mcpServers": {"hedgehog": {"command": args.hedgehog, "args": ["mcp", "--root", str(args.root)]}}}))
        cmd += ["--mcp-config", str(cfg), "--strict-mcp-config"]
        tools.append("mcp__hedgehog")
    return cmd + ["--allowedTools", *tools], env


def metrics(arm: str, out) -> dict:
    if arm == "hedgehog":
        res = next(x for x in reversed(out) if x.get("type") == "result")
        stats = res.get("stats") or {}
        if isinstance(stats, str):
            stats = ast.literal_eval(stats)
        model, mstats = next(iter(stats.get("models", {}).items()), (None, {}))
        tok = mstats.get("tokens", {})
        tools = stats.get("tools", {})
        inp, cached, outp = tok.get("prompt", 0), tok.get("cached", 0), tok.get("candidates", 0)
        p = PRICES.get(model or "")
        cost = round(((inp - cached) * p[0] + cached * p[1] + outp * p[2]) / 1e6, 4) if p else None
        return {"model": model, "seconds": round(res.get("duration_ms", 0) / 1000, 1), "turns": res.get("num_turns"),
                "input_tokens": inp, "cached_tokens": cached, "output_tokens": outp, "cost_usd": cost,
                "cost_note": "estimate (list prices)", "tool_calls": tools.get("totalCalls"),
                "tool_errors": tools.get("totalFail"), "answer": res.get("result", ""), "error": res.get("is_error")}
    u = out.get("usage", {})
    inp = u.get("input_tokens", 0) + u.get("cache_creation_input_tokens", 0) + u.get("cache_read_input_tokens", 0)
    return {"model": ",".join(out.get("modelUsage", {}) or []) or None, "seconds": round(out.get("duration_ms", 0) / 1000, 1),
            "turns": out.get("num_turns"), "input_tokens": inp, "cached_tokens": u.get("cache_read_input_tokens", 0),
            "output_tokens": u.get("output_tokens", 0), "cost_usd": out.get("total_cost_usd"), "cost_note": "reported by Claude Code",
            "tool_calls": None, "tool_errors": None, "answer": out.get("result", ""), "error": out.get("is_error")}


# ---------- scoring ----------

MULT = [(r"bn|billion|mil milhões|mrd\.?|milliard\w*", 1e9), (r"m\b|mn\b|million\w*|mio\.?|milhões", 1e6),
        (r"k\b|thousand|tsd\.?", 1e3)]
NUM = re.compile(r"(?<![\w.])[-−]?\d(?:[\d.,  ]*\d)?")


def numbers(text: str, pct_only: bool = False) -> list[float]:
    """Every reading of every number in the text (thousands/decimal separators are ambiguous)."""
    out = []
    for m in NUM.finditer(text):
        s = m.group().replace("−", "-").replace(" ", "").replace(" ", "")
        rest = text[m.end():m.end() + 16].lstrip().lower()
        if pct_only and not rest.startswith("%"):
            continue
        cands = set()
        if "," in s and "." in s:
            dec = "," if s.rfind(",") > s.rfind(".") else "."
            cands.add(float(s.replace("." if dec == "," else ",", "").replace(",", ".")))
        elif "," in s:
            if all(len(g) == 3 for g in s.split(",")[1:]):
                cands.add(float(s.replace(",", "")))
            if s.count(",") == 1:
                cands.add(float(s.replace(",", ".")))
        elif s.count(".") > 1:
            cands.add(float(s.replace(".", "")))
        else:
            cands.add(float(s))
            if "." in s and len(s.split(".")[1]) == 3:
                cands.add(float(s.replace(".", "")))
        mult = next((f for pat, f in MULT if re.match(pat, rest)), 1)
        out += [c * mult for c in cands] + ([c for c in cands] if mult != 1 else [])
    return out


def check(c: dict, answer: str):
    if c.get("judge"):
        return None
    if "num" in c:
        tol = c.get("tol", 0.005)
        want = c["num"]
        return any(abs((abs(x) if c.get("abs") else x) - want) <= tol * abs(want) for x in numbers(answer))
    if "pct_range" in c:
        lo, hi = c["pct_range"]
        return any(lo <= x <= hi for x in numbers(answer, pct_only=True))
    if "text_any" in c:
        a = answer.lower()
        return any(t.lower() in a for t in c["text_any"])
    raise ValueError(f"unknown check {c}")


# ---------- report ----------

def load(path: Path, default):
    return json.loads(path.read_text()) if path.exists() else default


def report(cases: list[dict]) -> None:
    runs = [json.loads(l) for l in RESULTS.read_text().splitlines()] if RESULTS.exists() else []
    grades = load(GRADES, {})
    latest = {}
    for r in runs:
        latest[(r["case"], r["arm"])] = r
    L = ["# Agent comparison: latest results", "",
         "Same file, same prompt, different agents. Generated by `eval/agents/run.py --report` from",
         "`results.jsonl` (every run is kept there); method in [README.md](README.md), open work in",
         "[BACKLOG.md](BACKLOG.md). ✓/✗ = automatic check against the reference answer, (✓)/(✗) = graded by a",
         "person, ? = not graded yet.", "",
         "| Case | Agent | Date | Model | Time | Input / output tokens | Cost (USD) | Tools (errors) | Checks |",
         "|---|---|---|---|---:|---:|---:|---:|---:|"]
    detail = []
    for case in cases:
        rows = [(a, latest[(case["id"], a)]) for a in ARMS if (case["id"], a) in latest]
        for arm, r in rows:
            g = grades.get(r["run"], {})
            res = [r["checks"].get(c["id"]) if not c.get("judge") else g.get(c["id"]) for c in case["checks"]]
            ok = sum(1 for x in res if x)
            tc = "–" if r["tool_calls"] is None else f"{r['tool_calls']} ({r['tool_errors'] or 0})"
            cost = "–" if r["cost_usd"] is None else f"{r['cost_usd']:.4f}" if r["cost_usd"] < 0.1 else f"{r['cost_usd']:.2f}"
            secs = "–" if r["seconds"] is None else f"{r['seconds']:.0f} s"
            toks = "–" if r["input_tokens"] is None else f"{r['input_tokens']:,} / {r['output_tokens']:,}"
            L.append(f"| {case['id']} | {arm} | {r['date'][:10]} | {r['model'] or '–'} | {secs} | {toks} | {cost} | {tc} | {ok}/{len(res)} |")
        if not rows:
            continue
        detail += ["", f"## {case['id']}: {case['title']}", "", f"File: `{case['file']}`", "", "> " + case["prompt"].replace("\n", "\n> "), "",
                   "| Check | " + " | ".join(a for a, _ in rows) + " |", "|---|" + "---|" * len(rows)]
        for c in case["checks"]:
            cells = []
            for _, r in rows:
                v = r["checks"].get(c["id"]) if not c.get("judge") else grades.get(r["run"], {}).get(c["id"])
                cells.append("?" if v is None else ("(✓)" if v else "(✗)") if c.get("judge") else ("✓" if v else "✗"))
            detail.append(f"| {c['desc']} | " + " | ".join(cells) + " |")
        notes = [f"- **{a}**: {grades[r['run']]['note']}" for a, r in rows if grades.get(r["run"], {}).get("note")]
        detail += [""] + notes + ([""] if notes else []) + ["Answers: " + ", ".join(f"[{a}](answers/{case['id']}.{a}.md)" for a, _ in rows)]
        if case.get("backlog"):
            detail.append("Backlog items from this case: " + ", ".join(case["backlog"]))
    (HERE / "RESULTS.md").write_text("\n".join(L + detail) + "\n")


# ---------- main ----------

def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cases", default="")
    ap.add_argument("--arms", default="hedgehog,claude")
    ap.add_argument("--root", type=Path, default=Path(os.environ.get("HEDGEHOG_DATA") or REPO.parent / "testdata") / "irregular")
    ap.add_argument("--hhc", default=str(Path.home() / ".hedgehog-cli/bin/hedgehog-cli"))
    ap.add_argument("--hedgehog", default=str(Path.home() / ".hedgehog-cli/bin/hedgehog"))
    ap.add_argument("--claude", default="claude")
    ap.add_argument("--hh-model", default="")
    ap.add_argument("--claude-model", default="")
    ap.add_argument("--timeout", type=int, default=1800)
    ap.add_argument("--max-usd", type=float, default=3.0, help="spending cap per Claude Code run")
    ap.add_argument("--report", action="store_true")
    ap.add_argument("--grade", nargs="+", metavar=("RUN", "CHECK=0|1"))
    ap.add_argument("--note", default="")
    args = ap.parse_args()
    cases = json.loads((HERE / "cases.json").read_text())
    if args.grade:
        grades = load(GRADES, {})
        g = grades.setdefault(args.grade[0], {})
        for kv in args.grade[1:]:
            k, _, v = kv.partition("=")
            g[k] = v in ("1", "true", "yes")
        if args.note:
            g["note"] = args.note
        GRADES.write_text(json.dumps(grades, indent=1, ensure_ascii=False) + "\n")
        return report(cases)
    if args.report:
        return report(cases)
    want = set(filter(None, args.cases.split(",")))
    (HERE / "runs").mkdir(exist_ok=True)
    for case in (c for c in cases if not want or c["id"] in want):
        prompt = f"File: {case['file']}\n\n{case['prompt']}"
        for arm in args.arms.split(","):
            cmd, env = command(arm, prompt, args)
            t0 = time.time()
            p = subprocess.run(cmd, cwd=args.root, env=env, capture_output=True, text=True, timeout=args.timeout)
            stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
            run = f"{stamp}-{case['id']}-{arm}"
            (HERE / "runs" / f"{run}.out").write_text(p.stdout + "\n--- stderr ---\n" + p.stderr[-20000:])
            try:
                m = metrics(arm, first_json(p.stdout))
            except Exception as e:  # keep going; the raw output is in runs/
                print(f"{run}: could not read the output ({e.__class__.__name__}); see runs/{run}.out")
                continue
            m["wall_seconds"] = round(time.time() - t0, 1)
            rec = {"run": run, "date": datetime.now(timezone.utc).isoformat(timespec="seconds"), "case": case["id"], "arm": arm,
                   "file": case["file"], "prompt": case["prompt"], **m,
                   "checks": {c["id"]: check(c, m["answer"]) for c in case["checks"] if not c.get("judge")}}
            with RESULTS.open("a") as f:
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")
            (HERE / "answers").mkdir(exist_ok=True)
            (HERE / "answers" / f"{case['id']}.{arm}.md").write_text(
                f"# {case['id']} · {arm} · {rec['date']} · {m['model']}\n\nRun `{run}`\n\n{m['answer']}\n")
            ok = sum(1 for v in rec["checks"].values() if v)
            print(f"{run}: {m['seconds']} s, {m['input_tokens']:,}/{m['output_tokens']:,} tokens, "
                  f"cost {m['cost_usd']}, checks {ok}/{len(rec['checks'])}")
    report(cases)


if __name__ == "__main__":
    main()
