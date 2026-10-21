"""Tool-swap arms: the same minimal loop (agent.py) and the same small model; only the tools change.

    swap-hedgehog  hedgehog's six MCP tools (`hedgehog mcp --root <root>`)
    swap-excelmcp  a generic Excel MCP server (excel-csv-mcp-server 1.0.2 = ishayoyo/excel-mcp), read tools only
    swap-pandas    one tool, python(code), in a sandbox with pandas
    swap-markdown  no tools; the file as markdown (MarkItDown) in the prompt, cut at the context budget
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path

from . import agent, tools

ARMS = ("swap-hedgehog", "swap-excelmcp", "swap-pandas", "swap-markdown")
INTRO = {
    "swap-hedgehog": "You have the tools of the hedgehog spreadsheet server; file paths are relative to the working folder.",
    "swap-excelmcp": "You have the tools of an Excel/CSV MCP server; file paths are relative to the working folder.",
    "swap-pandas": "You have a python tool that runs code in the working folder, which holds the file; paths are relative to it.",
    "swap-markdown": "You have no tools: the file's contents, converted to markdown, follow the questions.",
}


def case_files(case: dict) -> list[str]:
    return list(case.get("files") or [case["file"]])


def run_arm(arm: str, case: dict, prompt: str, root: Path, out: Path, run_id: str, key: str, model: str,
            hedgehog_exe: str, md_tokens: int) -> dict:
    """One run of a swap arm on a case; `prompt` is the same text every arm gets, `root` the case's folder."""
    files = case_files(case)
    work = out / "work" / run_id
    work.mkdir(parents=True, exist_ok=True)
    trace = (out / "runs" / f"{run_id}.jsonl").open("a")
    log = lambda ev: trace.write(json.dumps(ev, ensure_ascii=False) + "\n")
    user, ts, extra = prompt, None, {}
    try:
        if arm == "swap-hedgehog":
            ts = tools.hedgehog(hedgehog_exe, root, work)
        elif arm == "swap-excelmcp":
            tools.copy_files(root, files, work)
            ts = tools.excelmcp(work)
        elif arm == "swap-pandas":
            tools.copy_files(root, files, work)
            ts = tools.Pandas(work)
        elif arm == "swap-markdown":
            budget = (md_tokens or 0) // len(files)
            infos = []
            for f in files:
                md, info = tools.markdown(root, f, model, budget, out / "md-cache")
                user += f"\n\n--- {f} as markdown ---\n\n{md}"
                infos.append({"file": f, **info})
            extra["markdown"] = infos
            log({"markdown": infos})
        else:
            raise ValueError(arm)
        m = agent.run(key, model, INTRO[arm], user, ts, log)
    finally:
        if hasattr(ts, "close"):
            ts.close()
        trace.close()
        shutil.rmtree(work, ignore_errors=True)
    return m | extra
