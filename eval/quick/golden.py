"""Tier 0 mini golden: what hedgehog makes of ~20 small real files, compared with golden.tsv.

    python3 eval/quick/golden.py --bin target-quick/debug/hedgehog [--update]

For every file in files.txt (relative to $HEDGEHOG_DATA): the `open_file` catalog text through one
`hedgehog mcp` session, and the manifest of `hedgehog normalize` (its source path and generator
version left out). golden.tsv keeps per file: table count, catalog sha256, manifest sha256 (first
16 hex digits). A changed catalog is shown as a line diff against the text kept locally in
catalogs/ (gitignored: it is derived from the private test data). `--update` rewrites both.
Deterministic: the disk cache is off, the normalize time and the session's open-file lines are
dropped from the catalog. Exit code 1 on any difference.
"""

from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
SESSION_LINES = ("FILES open in this session", "JOIN KEYS ")


def sha(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()[:16]


def clean_catalog(text: str) -> str:
    text = re.sub(r"\(normalized in [0-9.]+ s\)", "(normalized in <t> s)", text)
    return "\n".join(l for l in text.splitlines() if not l.startswith(SESSION_LINES)) + "\n"


def catalogs(exe: str, root: Path, files: list[str], env: dict) -> list[str]:
    """open_file for every file in one `hedgehog mcp` session."""
    msgs = [{"jsonrpc": "2.0", "id": 0, "method": "initialize",
             "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "quick", "version": "0"}}}]
    for i, f in enumerate(files, 1):
        msgs.append({"jsonrpc": "2.0", "id": i, "method": "tools/call", "params": {"name": "open_file", "arguments": {"path": f}}})
    stdin = "".join(json.dumps(m) + "\n" for m in msgs)
    p = subprocess.run([exe, "mcp", "--root", str(root)], input=stdin, capture_output=True, text=True, env=env)
    texts: dict[int, str] = {}
    for line in p.stdout.splitlines():
        if line.startswith("{"):
            m = json.loads(line)
            if m.get("id"):
                if "result" in m:
                    r = m["result"]
                    body = "\n".join(c.get("text", "") for c in r.get("content", []))
                    texts[m["id"]] = ("ERROR: " if r.get("isError") else "") + body
                else:
                    texts[m["id"]] = f"ERROR: {m.get('error')}"
    if p.returncode != 0:
        print(f"hedgehog mcp exited with {p.returncode}: {p.stderr[-2000:]}", file=sys.stderr)
    return [clean_catalog(texts.get(i, f"ERROR: no response (server exit {p.returncode})")) for i in range(1, len(files) + 1)]


def manifest(exe: str, path: Path, out: Path, env: dict) -> tuple[str, int]:
    """(manifest sha, table count) of `hedgehog normalize`, or ('error: …', -1)."""
    out.mkdir(parents=True, exist_ok=True)
    p = subprocess.run([exe, "normalize", str(path), "-o", str(out)], capture_output=True, text=True, env=env)
    found = sorted(out.glob("*.manifest.json"))
    if p.returncode != 0 or not found:
        msg = (p.stderr or p.stdout).strip().replace("\t", " ").splitlines()
        return "error: " + (msg[-1] if msg else f"exit {p.returncode}"), -1
    m = json.loads(found[0].read_text())
    m.get("source", {}).pop("path", None)
    m.get("generator", {}).pop("version", None)
    return sha(json.dumps(m, sort_keys=True, ensure_ascii=False)), len(m.get("tables", []))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True, help="hedgehog binary (release or debug)")
    ap.add_argument("--data", type=Path, default=Path(os.environ.get("HEDGEHOG_DATA", "../testdata")))
    ap.add_argument("--files", type=Path, default=HERE / "files.txt")
    ap.add_argument("--golden", type=Path, default=HERE / "golden.tsv")
    ap.add_argument("--catalogs", type=Path, default=HERE / "catalogs")
    ap.add_argument("--update", action="store_true")
    ap.add_argument("--jobs", type=int, default=2)
    ap.add_argument("--tmp", help="folder for the normalize outputs (default: the system temp folder)")
    a = ap.parse_args()

    files = [l.strip() for l in a.files.read_text().splitlines() if l.strip() and not l.lstrip().startswith("#")]
    missing = [f for f in files if not (a.data / f).is_file()]
    if missing:
        print(f"mini golden: {len(missing)} files missing under {a.data} (HEDGEHOG_DATA), e.g. {missing[0]}")
        return 1
    env = dict(os.environ, HEDGEHOG_CACHE="off")
    with tempfile.TemporaryDirectory(prefix="hh_quick_", dir=a.tmp) as tmp, ThreadPoolExecutor(a.jobs + 1) as pool:
        cats = pool.submit(catalogs, a.bin, a.data, files, env)
        mans = list(pool.map(lambda kv: manifest(a.bin, a.data / kv[1], Path(tmp) / str(kv[0]), env), enumerate(files)))
        cats = cats.result()

    rows = [(f, n, sha(c), m) for f, c, (m, n) in zip(files, cats, mans)]
    lines = ["# file\ttables\tcatalog_sha\tmanifest_sha"] + [f"{f}\t{n}\t{c}\t{m}" for f, n, c, m in rows]
    if a.update:
        a.golden.write_text("\n".join(lines) + "\n")
        a.catalogs.mkdir(exist_ok=True)
        for old in a.catalogs.glob("*.txt"):
            old.unlink()
        for f, c in zip(files, cats):
            (a.catalogs / (f.replace("/", "__") + ".txt")).write_text(c)
        print(f"mini golden: wrote {a.golden.name} ({len(rows)} files) and catalogs/")
        return 0

    want = {}
    if a.golden.exists():
        for l in a.golden.read_text().splitlines():
            if l and not l.startswith("#"):
                p = l.split("\t")
                want[p[0]] = p[1:]
    bad = 0
    for (f, n, c, m), cat in zip(rows, cats):
        w = want.pop(f, None)
        got = [str(n), c, m]
        if w == got:
            continue
        bad += 1
        if w is None:
            print(f"NEW   {f}: {n} tables (not in golden.tsv; run with --update)")
            continue
        what = [k for k, x, y in zip(("tables", "catalog", "manifest"), w, got) if x != y]
        print(f"DIFF  {f}: {', '.join(what)} changed (tables {w[0]} -> {n}, manifest {w[2]} -> {m})")
        if w[1] != c:
            kept = a.catalogs / (f.replace("/", "__") + ".txt")
            old = kept.read_text() if kept.exists() else None
            if old is None or sha(old) != w[1]:
                print("      (no stored catalog text for this golden; run --update on the base commit to get diffs)")
            else:
                d = difflib.unified_diff(old.splitlines(), cat.splitlines(), "golden", "now", n=1, lineterm="")
                for l in list(d)[2:]:
                    print("      " + l[:240])
    for f in want:
        bad += 1
        print(f"GONE  {f} (in golden.tsv, not in files.txt)")
    if bad:
        print(f"mini golden: {bad} of {len(rows)} files differ — accept with `make quick-update` (scripts/quick.sh --update)")
        return 1
    print(f"mini golden OK: {len(rows)} files identical")
    return 0


if __name__ == "__main__":
    sys.exit(main())
