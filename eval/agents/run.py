"""Ask hedgehog-cli and Claude Code the same questions about the same file; score and track both.

    python eval/agents/run.py                         # every case, arms hedgehog + claude
    python eval/agents/run.py --cases pt-contracts --arms hedgehog,claude,claude-hh
    python eval/agents/run.py --grade <run id> covid=1 variants=0 --note "..."
    python eval/agents/run.py --report                # rebuild RESULTS.md from results.jsonl

Arms (same prompt text for all):
- hedgehog:  hedgehog-cli one-shot (`hhc PROMPT -o json`), its own model setting (or --hh-model);
- claude:    Claude Code (`claude -p PROMPT --output-format json`) with shell + file tools only;
- claude-hh: Claude Code with the hedgehog plugin (integrations/claude-code): the `spreadsheets` skill
             and `hedgehog mcp` (tools mcp__hedgehog__*), plus the same shell + file tools.

Each run appends one record to results.jsonl (metrics, answer, automatic checks) and writes the
answer to answers/<case>.<arm>.md; RESULTS.md shows the latest run per case and arm. Checks in
cases.json: `num` (a number in the answer within `tol`, relative), `pct_range` (a percentage in
[lo, hi]; with `abs`, of the absolute value), `text_any` (any of the strings), `judge` (graded by a person
with --grade). A case may name several files (`files`; the prompt then starts `Files: a, b`) and a `root`
folder under $HEDGEHOG_DATA other than irregular/ (e.g. "clean"); check_cases.py validates cases.json.
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
PLUGIN = REPO / "integrations" / "claude-code"  # the Claude Code plugin the claude-hh arm loads
OUT = HERE  # --out-dir: where results.jsonl, grades.json, RESULTS.md, answers/ and runs/ go
RESULTS = OUT / "results.jsonl"
GRADES = OUT / "grades.json"
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


def command(arm: str, prompt: str, args, root: Path | None = None) -> tuple[list[str], dict]:
    root = root or args.root
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
        # The Claude Code plugin (integrations/claude-code): its skill via --plugin-dir, its MCP server
        # config with the binary under test and the case root filled in. --strict-mcp-config keeps the
        # user's other MCP servers out (it also drops the plugin's own server, hence the explicit config).
        server = json.loads((PLUGIN / ".mcp.json").read_text())["mcpServers"]["hedgehog"]
        server = {**server, "command": args.hedgehog,
                  "args": [str(root) if a == "${CLAUDE_PROJECT_DIR}" else a for a in server["args"]]}
        cfg = OUT / "runs" / "mcp.json"
        cfg.parent.mkdir(exist_ok=True)
        cfg.write_text(json.dumps({"mcpServers": {"hedgehog": server}}))
        cmd += ["--plugin-dir", str(PLUGIN), "--mcp-config", str(cfg), "--strict-mcp-config"]
        tools += ["Skill", "mcp__hedgehog__*"]
        # With many user skills installed, the skill listing drops descriptions past its character
        # budget, and a skill without a description is never chosen; raise it so the plugin's skill is
        # listed as it would be for a user with few skills (this also lists every other skill in full).
        if args.skill_budget:
            env["SLASH_COMMAND_TOOL_CHAR_BUDGET"] = str(args.skill_budget)
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

MULT = [(r"bn|b\b|billion|mil milhões|mrd\.?|milliard\w*", 1e9), (r"m\b|mn\b|million\w*|mio\.?|milhões", 1e6),
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
        text = answer
        if c.get("near"):  # the number must follow one of these words within 200 characters (same entity)
            low = answer.lower()
            text = " ".join(answer[i:i + 200] for w in c["near"] for i in [m for m in range(len(low)) if low.startswith(w.lower(), m)])
        return any(abs((abs(x) if c.get("abs") else x) - want) <= tol * abs(want) for x in numbers(text))
    if "pct_range" in c:
        lo, hi = c["pct_range"]
        return any(lo <= (abs(x) if c.get("abs") else x) <= hi for x in numbers(answer, pct_only=True))
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
    batches = {}
    for r in runs:  # runs of one invocation share a batch; the latest batch per case and arm counts
        key = (r["case"], r["arm"])
        b = r.get("batch", r["run"])
        if key not in batches or batches[key][0] != b:
            batches[key] = (b, [])
        batches[key][1].append(r)
    L = ["# Agent comparison: latest results", ""]
    T = ["## All runs", "",
         "Generated by `eval/agents/run.py --report` from `results.jsonl` (every run is kept there); method in",
         "[README.md](README.md), open work in [BACKLOG.md](BACKLOG.md). ✓/✗ = automatic check against the",
         "reference answer, (✓)/(✗) = graded by a person, ? = not graded yet. With repeated runs (`--repeat`),",
         "numbers are means; Checks shows the mean and the worst run, and the check table shows how many runs passed.", "",
         "| Case | Agent | Date | Model | Runs | Time | Input / output tokens | Cost (USD) | Tools (errors) | Checks (worst) |",
         "|---|---|---|---|---:|---:|---:|---:|---:|---:|"]
    detail = []
    summary = {}

    def mean(rs, k):
        v = [r[k] for r in rs if r.get(k) is not None]
        return sum(v) / len(v) if v else None

    def passed(r, c):
        return r["checks"].get(c["id"]) if not c.get("judge") else grades.get(r["run"], {}).get(c["id"])

    for case in cases:
        rows = [(a, batches[(case["id"], a)][1]) for a in ARMS if (case["id"], a) in batches]
        for arm, rs in rows:
            scores = [sum(1 for c in case["checks"] if passed(r, c)) for r in rs]
            n = len(case["checks"])
            tcs, terr, cost, secs = mean(rs, "tool_calls"), mean(rs, "tool_errors"), mean(rs, "cost_usd"), mean(rs, "seconds")
            tc = "–" if tcs is None else f"{tcs:.0f} ({terr or 0:.0f})"
            cost = "–" if cost is None else f"{cost:.4f}" if cost < 0.1 else f"{cost:.2f}"
            secs = "–" if secs is None else f"{secs:.0f} s"
            toks = "–" if mean(rs, "input_tokens") is None else f"{mean(rs, 'input_tokens'):,.0f} / {mean(rs, 'output_tokens'):,.0f}"
            sc = f"{sum(scores) / len(scores):.1f}/{n} ({min(scores)})" if len(rs) > 1 else f"{scores[0]}/{n}"
            summary[(case["id"], arm)] = (sum(scores) / len(scores), min(scores), n, mean(rs, "seconds"), mean(rs, "cost_usd"),
                                          mean(rs, "input_tokens"), rs[-1]["model"])
            T.append(f"| {case['id']} | {arm} | {rs[-1]['date'][:10]} | {rs[-1]['model'] or '–'} | {len(rs)} | {secs} | {toks} | {cost} | {tc} | {sc} |")
        if not rows:
            continue
        detail += ["", f"## {case['id']}: {case['title']}", "", ("Files: " if len(case.get("files") or []) > 1 else "File: ") + ", ".join(f"`{f}`" for f in case.get("files") or [case["file"]]) + (f" (under `{case['root']}/`)" if case.get("root") else ""), "", "> " + case["prompt"].replace("\n", "\n> "), "",
                   "| Check | " + " | ".join(a for a, _ in rows) + " |", "|---|" + "---|" * len(rows)]
        for c in case["checks"]:
            cells = []
            for _, rs in rows:
                vs = [passed(r, c) for r in rs]
                if len(rs) > 1:
                    cells.append("?" if all(v is None for v in vs) else f"{sum(1 for v in vs if v)}/{len(vs)}")
                else:
                    v = vs[0]
                    cells.append("?" if v is None else ("(✓)" if v else "(✗)") if c.get("judge") else ("✓" if v else "✗"))
            detail.append(f"| {c['desc']} | " + " | ".join(cells) + " |")
        notes = [f"- **{a}** (`{r['run']}`): {grades[r['run']]['note']}" for a, rs in rows for r in rs if grades.get(r["run"], {}).get("note")]
        detail += [""] + notes + ([""] if notes else []) + ["Answers (latest run): " + ", ".join(f"[{a}](answers/{case['id']}.{a}.md)" for a, _ in rows)]
        if case.get("backlog"):
            detail.append("Backlog items from this case: " + ", ".join(case["backlog"]))
    # Headline: the simplest harness with hedgehog's tools vs the best harness with its own tools.
    H = ["## Headline: hedgehog-cli vs Claude Code", "",
         "The question this folder answers: how much does the **tool** give? **hedgehog-cli** is a simple chat",
         "loop on a small, cheap model whose only tools are hedgehog's. **Claude Code** is the strongest",
         "general harness, on a frontier model, reading the file with its own Python. Same file, same prompt.", "",
         "| Case | hedgehog-cli checks (worst) | Claude Code checks | Time: hedgehog / Claude | Cost: hedgehog / Claude | Input tokens: hedgehog / Claude |",
         "|---|---:|---:|---:|---:|---:|"]
    tot = {"h": [0.0, 0, 0, 0.0, 0.0, 0.0], "c": [0.0, 0, 0, 0.0, 0.0, 0.0]}
    fmt_t = lambda v: "–" if v is None else f"{v:.0f} s"
    fmt_c = lambda v: "–" if v is None else f"${v:.4f}" if v < 0.1 else f"${v:.2f}"
    fmt_k = lambda v: "–" if v is None else f"{v / 1000:.0f}k"
    models = set()
    for case in cases:
        h, c = summary.get((case["id"], "hedgehog")), summary.get((case["id"], "claude"))
        if not h or not c:
            continue
        models |= {f"hedgehog-cli: {h[6]}", f"Claude Code: {c[6]}"}
        for k, x in (("h", h), ("c", c)):
            t = tot[k]
            t[0] += x[0]; t[1] += x[1]; t[2] += x[2]
            t[3] += x[3] or 0; t[4] += x[4] or 0; t[5] += x[5] or 0
        H.append(f"| {case['id']} | {h[0]:.1f}/{h[2]} ({h[1]}) | {c[0]:.1f}/{c[2]} | {fmt_t(h[3])} / {fmt_t(c[3])} | "
                 f"{fmt_c(h[4])} / {fmt_c(c[4])} | {fmt_k(h[5])} / {fmt_k(c[5])} |")
    if tot["h"][2]:
        h, c = tot["h"], tot["c"]
        H.append(f"| **all cases** | **{h[0]:.1f}/{h[2]} ({h[1]})** | **{c[0]:.1f}/{c[2]}** | {fmt_t(h[3])} / {fmt_t(c[3])} | "
                 f"{fmt_c(h[4])} / {fmt_c(c[4])} | {fmt_k(h[5])} / {fmt_k(c[5])} |")
        ratio = f"{c[4] / h[4]:.0f}× cheaper" if h[4] else ""
        H += ["", f"Models: {', '.join(sorted(models))}. hedgehog-cli is {c[3] / h[3]:.1f}× faster and {ratio}; "
              f"it passes {100 * h[0] / h[2]:.0f} % of the checks vs {100 * c[0] / c[2]:.0f} % for Claude Code.",
              "`claude-hh` (Claude Code plus hedgehog's plugin) is in the table below: the same tool in the best harness."]
    (OUT / "RESULTS.md").write_text("\n".join(L + H + [""] + T + detail) + "\n")


def done_before(case: dict) -> bool:
    """Claude Code's answer doesn't depend on hedgehog: one recorded run per (file, prompt) is enough."""
    if not RESULTS.exists():
        return False
    for line in RESULTS.read_text().splitlines():
        r = json.loads(line)
        if r["case"] == case["id"] and r["arm"] == "claude" and r["file"] == case["file"] and r["prompt"] == case["prompt"]:
            return True
    return False


def case_prompt(case: dict) -> str:
    """What every agent gets: the file path(s) relative to the case root, then the questions."""
    files = case.get("files") or [case["file"]]
    head = f"File: {files[0]}" if len(files) == 1 else "Files: " + ", ".join(files)
    return f"{head}\n\n{case['prompt']}"


def case_root(case: dict, args) -> Path:
    """The folder the agent runs in: --root (default $HEDGEHOG_DATA/irregular), or the case's own `root`
    (a folder under $HEDGEHOG_DATA, e.g. "clean")."""
    return args.root.parent / case["root"] if case.get("root") else args.root


# ---------- main ----------

def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cases", default="")
    ap.add_argument("--arms", default="hedgehog", help="hedgehog, claude, claude-hh (comma-separated)")
    ap.add_argument("--rerun-claude", action="store_true",
                    help="run the claude arm again even if this case already has a recorded answer for the same file and prompt")
    ap.add_argument("--root", type=Path, default=Path(os.environ.get("HEDGEHOG_DATA") or REPO.parent / "testdata") / "irregular")
    ap.add_argument("--hhc", default=str(Path.home() / ".hedgehog-cli/bin/hedgehog-cli"))
    ap.add_argument("--hedgehog", default=str(Path.home() / ".hedgehog-cli/bin/hedgehog"))
    ap.add_argument("--claude", default="claude")
    ap.add_argument("--hh-model", default="")
    ap.add_argument("--claude-model", default="")
    ap.add_argument("--timeout", type=int, default=1800)
    ap.add_argument("--max-usd", type=float, default=3.0, help="spending cap per Claude Code run")
    ap.add_argument("--skill-budget", type=int, default=80000,
                    help="claude-hh: skill-listing character budget, so the plugin's skill is listed with its description (0 = Claude Code's default)")
    ap.add_argument("--repeat", type=int, default=1, help="runs per case and agent (the worst run counts)")
    ap.add_argument("--out-dir", type=Path, default=HERE, help="where results and answers go (default: this folder)")
    ap.add_argument("--report", action="store_true")
    ap.add_argument("--grade", nargs="+", metavar=("RUN", "CHECK=0|1"))
    ap.add_argument("--note", default="")
    args = ap.parse_args()
    global OUT, RESULTS, GRADES
    OUT = args.out_dir.resolve()
    OUT.mkdir(parents=True, exist_ok=True)
    RESULTS, GRADES = OUT / "results.jsonl", OUT / "grades.json"
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
    (OUT / "runs").mkdir(exist_ok=True)
    batch = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    for case in (c for c in cases if not want or c["id"] in want):
        prompt = case_prompt(case)
        root = case_root(case, args)
        for arm in [a for a in args.arms.split(",") for _ in range(args.repeat)]:
            if arm == "claude" and not args.rerun_claude and done_before(case):
                print(f"{case['id']}: claude answer already recorded for this file and prompt; reusing it (--rerun-claude to run again)")
                break
            cmd, env = command(arm, prompt, args, root)
            t0 = time.time()
            p = subprocess.run(cmd, cwd=root, env=env, capture_output=True, text=True, timeout=args.timeout)
            stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
            run = f"{stamp}-{case['id']}-{arm}"
            (OUT / "runs" / f"{run}.out").write_text(p.stdout + "\n--- stderr ---\n" + p.stderr[-20000:])
            try:
                m = metrics(arm, first_json(p.stdout))
            except Exception as e:  # keep going; the raw output is in runs/
                print(f"{run}: could not read the output ({e.__class__.__name__}); see runs/{run}.out")
                continue
            m["wall_seconds"] = round(time.time() - t0, 1)
            rec = {"run": run, "batch": batch, "date": datetime.now(timezone.utc).isoformat(timespec="seconds"), "case": case["id"], "arm": arm,
                   "file": case["file"], "prompt": case["prompt"], **m,
                   "checks": {c["id"]: check(c, m["answer"]) for c in case["checks"] if not c.get("judge")}}
            with RESULTS.open("a") as f:
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")
            (OUT / "answers").mkdir(exist_ok=True)
            (OUT / "answers" / f"{case['id']}.{arm}.md").write_text(
                f"# {case['id']} · {arm} · {rec['date']} · {m['model']}\n\nRun `{run}`\n\n{m['answer']}\n")
            ok = sum(1 for v in rec["checks"].values() if v)
            print(f"{run}: {m['seconds']} s, {m['input_tokens']:,}/{m['output_tokens']:,} tokens, "
                  f"cost {m['cost_usd']}, checks {ok}/{len(rec['checks'])}")
    report(cases)


if __name__ == "__main__":
    main()
