"""Run SQL against a spreadsheet through `hedgehog mcp`, for building reference answers.

    python eval/agents/sql.py FILE "SELECT ..." ["SELECT ..." ...]
    python eval/agents/sql.py FILE --catalog

FILE is relative to --root (default: $HEDGEHOG_DATA/irregular). Every query runs in one server
session, so the file is normalized once. Uses `hedgehog` from PATH or $HEDGEHOG_BIN.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
from pathlib import Path


def call(root: Path, calls: list[tuple[str, dict]]) -> list[str]:
    """Run tool calls in one `hedgehog mcp` session; returns each call's text."""
    exe = os.environ.get("HEDGEHOG_BIN") or shutil.which("hedgehog") or str(Path.home() / ".hedgehog-cli/bin/hedgehog")
    msgs = [{"jsonrpc": "2.0", "id": 0, "method": "initialize",
             "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "eval", "version": "0"}}}]
    for i, (name, args) in enumerate(calls, 1):
        msgs.append({"jsonrpc": "2.0", "id": i, "method": "tools/call", "params": {"name": name, "arguments": args}})
    stdin = "".join(json.dumps(m) + "\n" for m in msgs)
    out = subprocess.run([exe, "mcp", "--root", str(root)], input=stdin, capture_output=True, text=True, check=True).stdout
    texts = {}
    for line in out.splitlines():
        if line.startswith("{"):
            m = json.loads(line)
            if m.get("id"):
                texts[m["id"]] = "\n".join(c["text"] for c in m["result"]["content"]) if "result" in m else str(m.get("error"))
    return [texts.get(i, "") for i in range(1, len(calls) + 1)]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("file")
    ap.add_argument("sql", nargs="*")
    ap.add_argument("--catalog", action="store_true")
    ap.add_argument("--root", type=Path, default=Path(os.environ.get("HEDGEHOG_DATA", "testdata")) / "irregular")
    ap.add_argument("--limit", type=int, default=50)
    args = ap.parse_args()
    calls = [("open_file", {"path": args.file})] if args.catalog else []
    calls += [("query", {"path": args.file, "sql": q, "limit": args.limit}) for q in args.sql]
    for q, text in zip(["catalog"] * args.catalog + args.sql, call(args.root, calls)):
        print(f"== {q}\n{text}\n")


if __name__ == "__main__":
    main()
