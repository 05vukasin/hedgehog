"""Run hedgehog over every spreadsheet a dataset's items reference; resumable, one process per call.

    python3 -I eval/public/run_hedgehog.py mimotable [--workers 2] [--limit N]

Per file, under <work>/<dataset>/hh/<key>/ (key = sha1 of the file's path in the dataset):
  cells.tsv      `hedgehog cells`: the raw source cells (the ground truth for "is this value in the file")
  norm/          `hedgehog normalize --csv --keep-totals`: manifest + one CSV per table (with _src_row, _row_type)
  catalog.txt    `hedgehog mcp` -> open_file: the catalog an agent sees first
  meta.json      exit codes, wall time, peak RSS (wait4) and error text of each call; written last (= done)
The disk cache is off (HEDGEHOG_CACHE=0), so open times are cold. Files over 200 MB are skipped.
Run it under scripts/run_capped.sh.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import data_dir, hedgehog_bin, read_jsonl, work_dir  # noqa: E402

MAX_BYTES = 200 << 20
LAUNCHER = ""
TIMEOUT = 300


def dataset_files(name: str) -> list[str]:
    root = data_dir() / name
    files: set[str] = set()
    for it in read_jsonl(root / "items.jsonl"):
        files.update(it.get("files") or [it["file"]])
    for t in read_jsonl(root / "tables_meta.jsonl"):
        files.add(t["file"])
    return sorted(f for f in files if (root / f).exists())


def key_of(rel: str) -> str:
    return hashlib.sha1(rel.encode()).hexdigest()[:16]


def maxrss_launcher() -> str:
    """eval/public/maxrss.c, compiled once into the work folder."""
    exe = work_dir() / "bin" / "maxrss"
    src = Path(__file__).resolve().parent / "maxrss.c"
    if not exe.exists() or exe.stat().st_mtime < src.stat().st_mtime:
        exe.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["cc", "-O2", "-o", str(exe), str(src)], check=True)
    return str(exe)


def timed(cmd: list[str], stdin: str | None = None) -> dict:
    """Run with wall time and the peak RSS of exactly this process (kB, via maxrss.c); kill after TIMEOUT."""
    env = dict(os.environ, HEDGEHOG_CACHE="0")
    tmp = work_dir() / "tmp"
    tmp.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryFile("w+", dir=tmp) as fo, tempfile.TemporaryFile("w+", dir=tmp) as fe, \
            tempfile.NamedTemporaryFile("r", suffix=".rss", dir=tmp) as fr:
        cmd = [LAUNCHER, fr.name, *cmd]
        t = time.perf_counter()
        p = subprocess.Popen(cmd, stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
                             stdout=fo, stderr=fe, env=env, text=True, start_new_session=True)
        if stdin is not None:
            try:
                p.stdin.write(stdin)  # type: ignore[union-attr]
                p.stdin.close()  # type: ignore[union-attr]
            except BrokenPipeError:
                pass
        killer = threading.Timer(TIMEOUT, lambda: os.killpg(p.pid, 9))
        killer.start()
        p.wait()
        killer.cancel()
        secs = time.perf_counter() - t
        rss = fr.read().split()
        fo.seek(0)
        fe.seek(0)
        out, err = fo.read(), fe.read()
    return {"rc": p.returncode, "secs": round(secs, 4), "timeout": secs >= TIMEOUT, "maxrss_kb": int(rss[0]) if rss else None,
            "stdout": out, "stderr": err.strip()[-2000:]}


def mcp_open(path: Path) -> dict:
    msgs = [{"jsonrpc": "2.0", "id": 0, "method": "initialize",
             "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "public-bench", "version": "0"}}},
            {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "open_file", "arguments": {"path": path.name}}}]
    r = timed([hedgehog_bin(), "mcp", "--root", str(path.parent)], "".join(json.dumps(m) + "\n" for m in msgs))
    text, is_error = "", None
    for line in (r.pop("stdout") or "").splitlines():
        if line.startswith("{"):
            m = json.loads(line)
            if m.get("id") == 1:
                if "result" in m:
                    text = "\n".join(c.get("text", "") for c in m["result"].get("content", []))
                    is_error = bool(m["result"].get("isError"))
                else:
                    text, is_error = json.dumps(m.get("error")), True
    r["text"], r["is_error"] = text, is_error
    return r


def one(name: str, rel: str) -> str:
    root = data_dir() / name
    src = root / rel
    out = work_dir() / name / "hh" / key_of(rel)
    if (out / "meta.json").exists():
        return "cached"
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    meta = {"file": rel, "bytes": src.stat().st_size}
    if meta["bytes"] > MAX_BYTES:
        meta["skipped"] = "over 200 MB"
    else:
        c = timed([hedgehog_bin(), "cells", str(src)])
        (out / "cells.tsv").write_text(c.pop("stdout") or "")
        meta["cells"] = c
        n = timed([hedgehog_bin(), "normalize", str(src), "-o", str(out / "norm"), "--csv", "--keep-totals"])
        n.pop("stdout")
        for x in (out / "norm").glob("*.normalized.xlsx"):  # the CSVs and the manifest hold the same
            x.unlink()
        meta["normalize"] = n
        o = mcp_open(src)
        (out / "catalog.txt").write_text(o.pop("text"))
        meta["open"] = o
    (out / "meta.json").write_text(json.dumps(meta, ensure_ascii=False, indent=1))
    return "done"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("dataset")
    ap.add_argument("--workers", type=int, default=2)
    ap.add_argument("--limit", type=int)
    a = ap.parse_args()
    global LAUNCHER
    LAUNCHER = maxrss_launcher()
    files = dataset_files(a.dataset)[: a.limit]
    t = time.time()
    print(f"{a.dataset}: {len(files)} files, binary {hedgehog_bin()}", flush=True)
    done = 0
    with ThreadPoolExecutor(a.workers) as ex:
        for i, st in enumerate(ex.map(lambda f: one(a.dataset, f), files), 1):
            done += st == "done"
            if i % 100 == 0 or i == len(files):
                print(f"  {i}/{len(files)} ({done} new) {time.time() - t:.0f} s", flush=True)


if __name__ == "__main__":
    main()
