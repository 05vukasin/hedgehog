"""The swapped tool sets: hedgehog's MCP server, a generic Excel MCP server, a pandas code tool, and
markdown of the sheet in the prompt.

Untrusted code and parsers run in a bubblewrap sandbox (`sandbox()`): no network, no view of the home
folder, read-only system and interpreter, a working folder that holds only a copy of the case's files,
and limits on memory and time. hedgehog's own binary (the tool under test) runs as in the other arms.
"""

from __future__ import annotations

import json
import os
import queue
import shutil
import subprocess
import threading
from pathlib import Path

HERE = Path(__file__).resolve().parent
SCRATCH = Path(os.environ.get("SWAP_DIR", "/tmp/claude-1000/w1swap"))
PANDAS_PY = SCRATCH / "venv-pandas/bin/python"          # pandas 2.2.3, openpyxl 3.1.5, python-calamine 0.4.0, xlrd 2.0.1
MARKITDOWN_PY = SCRATCH / "venv-markitdown/bin/python"  # markitdown[xlsx,xls] 0.1.3, tiktoken 0.9.0
EXCELMCP_JS = SCRATCH / "excelmcp/inst/node_modules/excel-csv-mcp-server/dist/index.js"  # excel-csv-mcp-server 1.0.2
PANDAS_MAX_OUT = 8000   # characters of a python tool result
MCP_MAX_OUT = 32000     # characters of an MCP tool result (≈ 8k tokens)
# HH_HIDE_KINDS=quality,types: drop these catalog line kinds (eval/info/score.py KINDS / SEGMENTS) from
# open_file results, to measure with a model what they are worth.
HIDE_KINDS = {k for k in os.environ.get("HH_HIDE_KINDS", "").split(",") if k}
# Read and analysis tools of excel-csv-mcp-server (ishayoyo/excel-mcp); its write, export, formatting
# and LLM-calling tools are not offered.
EXCELMCP_READ = {"read_file", "get_cell", "get_range", "get_headers", "search", "filter_rows", "aggregate",
                 "read_file_chunked", "get_file_info", "statistical_analysis", "correlation_analysis", "data_profile",
                 "pivot_table", "trend_analysis", "dcf_analysis", "budget_variance_analysis", "ratio_analysis",
                 "scenario_modeling", "bulk_aggregate_multi_files", "evaluate_formula", "explain_formula"}
CONTEXT = {"gpt-4.1": 1_047_576, "gpt-4o": 128_000}  # model context windows (tokens), by name prefix


def sandbox(workdir: Path, ro: list[Path], mem: str | None = None, secs: int | None = None, env: dict | None = None) -> list[str]:
    """Command prefix that runs a program in a bubblewrap sandbox with `workdir` as /work."""
    pre = ["timeout", "-s", "KILL", str(secs)] if secs else []
    if mem:
        pre += ["prlimit", f"--as={mem}"]
    cmd = pre + ["bwrap", "--ro-bind", "/usr", "/usr", "--symlink", "usr/lib", "/lib", "--symlink", "usr/lib", "/lib64",
                 "--symlink", "usr/bin", "/bin", "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"]
    for p in ro:
        cmd += ["--ro-bind", str(p), str(p)]
    cmd += ["--bind", str(workdir), "/work", "--chdir", "/work", "--unshare-all", "--die-with-parent", "--new-session",
                  "--clearenv", "--setenv", "HOME", "/work", "--setenv", "PATH", "/usr/bin",
                  "--setenv", "OPENBLAS_NUM_THREADS", "1", "--setenv", "OMP_NUM_THREADS", "1"]
    for k, v in (env or {}).items():  # after --clearenv
        cmd += ["--setenv", k, v]
    return cmd


def python_ro(py: Path) -> list[Path]:
    """The venv and the interpreter it points to (both must be visible in the sandbox)."""
    venv = py.parent.parent
    dirs = [venv, Path(os.path.realpath(py)).parent.parent]
    if py.is_symlink():  # uv links through a version-alias folder (cpython-3.12-… -> cpython-3.12.13-…)
        dirs.append(Path(os.readlink(py)).parent.parent)
    return list(dict.fromkeys(dirs))


def copy_files(root: Path, files: list[str], workdir: Path) -> None:
    """The case's files (only those) under their relative paths, so every arm sees the same paths."""
    for f in files:
        dst = workdir / f
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(root / f, dst)


def clip(text: str, n: int) -> str:
    return text if len(text) <= n else text[:n] + f"\n[output truncated: {len(text):,} characters, first {n:,} shown]"


def openai_schema(s: dict) -> dict:
    """MCP input schemas as OpenAI function parameters (arrays need `items`)."""
    if not isinstance(s, dict):
        return s
    s = {k: openai_schema(v) if isinstance(v, dict) else v for k, v in s.items()}
    if "properties" in s:
        s["properties"] = {k: openai_schema(v) for k, v in s["properties"].items()}
    if s.get("type") == "array" and "items" not in s:
        s["items"] = {}
    return s


class Mcp:
    """A stdio MCP client: start the server, list its tools, forward calls (one session per run)."""

    def __init__(self, cmd: list[str], cwd: Path, log: Path, allow: set[str] | None = None, timeout: int = 300):
        self.cmd, self.cwd, self.logp, self.allow, self.timeout = cmd, cwd, log, allow, timeout
        self.n = 0
        self.start()
        tools = self.rpc("tools/list", {})["tools"]
        self.specs = [{"type": "function", "function": {"name": t["name"], "description": t.get("description", ""),
                                                         "parameters": openai_schema(t.get("inputSchema") or {"type": "object", "properties": {}})}}
                      for t in tools if allow is None or t["name"] in allow]

    def start(self) -> None:
        self.err = self.logp.open("a")
        env = {"PATH": "/usr/bin:/bin", "HOME": str(self.cwd)}  # no API keys reach the server
        self.p = subprocess.Popen(self.cmd, cwd=self.cwd, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=self.err, text=True, bufsize=1)
        self.q: queue.Queue = queue.Queue()
        threading.Thread(target=lambda: [self.q.put(l) for l in self.p.stdout] + [self.q.put(None)], daemon=True).start()
        self.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "swap-eval", "version": "0"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, msg: dict) -> None:
        self.p.stdin.write(json.dumps(msg) + "\n")
        self.p.stdin.flush()

    def rpc(self, method: str, params: dict, timeout: int | None = None) -> dict:
        self.n += 1
        self.send({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params})
        while True:
            line = self.q.get(timeout=timeout or self.timeout)  # queue.Empty on timeout
            if line is None:
                raise RuntimeError(f"server exited (code {self.p.poll()})")
            if not line.startswith("{"):
                continue
            m = json.loads(line)
            if m.get("id") == self.n:
                if "error" in m:
                    raise RuntimeError(f"MCP error: {m['error'].get('message', m['error'])}")
                return m["result"]

    def call(self, name: str, args: dict) -> tuple[str, bool]:
        if self.allow is not None and name not in self.allow:
            return f"error: unknown tool {name}", True
        try:
            r = self.rpc("tools/call", {"name": name, "arguments": args})
        except queue.Empty:
            self.close()  # a stuck server is restarted for the next call
            self.start()
            return f"error: the tool did not answer within {self.timeout} s", True
        except RuntimeError as e:
            if self.p.poll() is not None:
                self.close()
                self.start()
            return f"error: {e}", True
        text = "\n".join(c.get("text", "") for c in r.get("content", []) if c.get("type") == "text")
        if HIDE_KINDS and name == "open_file":  # catalog line ablation (eval/info/README.md); test-only
            import sys
            sys.path.insert(0, str(HERE.parents[1] / "info"))
            from score import strip_kinds
            text = strip_kinds(text, HIDE_KINDS)
        return clip(text, MCP_MAX_OUT), bool(r.get("isError"))

    def close(self) -> None:
        try:
            self.p.stdin.close()
            self.p.wait(timeout=10)
        except Exception:
            self.p.kill()
        self.err.close()


def hedgehog(exe: str, root: Path, workdir: Path) -> Mcp:
    return Mcp([exe, "mcp", "--root", str(root)], cwd=workdir, log=workdir.parent / (workdir.name + ".server.log"))


def excelmcp(workdir: Path) -> Mcp:
    pkg = EXCELMCP_JS.parents[3]  # the isolated install (node_modules inside)
    # V8 reserves far more address space than it uses, so memory is capped with its own heap limit.
    cmd = sandbox(workdir, [pkg]) + ["/usr/bin/node", "--max-old-space-size=1536", str(EXCELMCP_JS)]
    return Mcp(cmd, cwd=workdir, log=workdir.parent / (workdir.name + ".server.log"), allow=EXCELMCP_READ)


class Pandas:
    """One tool, python(code): model-written code in a stateful Python process in the sandbox
    (2 GB, 60 s per call, no network, only the file), like a notebook or a code interpreter."""

    LIMIT = 60

    def __init__(self, workdir: Path):
        self.workdir = workdir
        self.specs = [{"type": "function", "function": {
            "name": "python",
            "description": "Run Python 3.12 code in a persistent session (like a notebook cell) in the working folder. "
                           "Variables and imports persist between calls; the value of a final expression is shown, as is "
                           "everything printed (max 8000 characters). pandas, numpy, openpyxl, python-calamine and xlrd are "
                           "installed. Limits: 60 s per call, 2 GB memory, no network.",
            "parameters": {"type": "object", "properties": {"code": {"type": "string", "description": "Python source"}},
                           "required": ["code"]}}}]
        self.start()

    def start(self) -> None:
        cmd = sandbox(self.workdir, python_ro(PANDAS_PY) + [HERE], mem="2147483648") + [
            str(PANDAS_PY), "-I", str(HERE / "py_kernel.py"), str(self.LIMIT)]
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
        self.q: queue.Queue = queue.Queue()
        threading.Thread(target=lambda: [self.q.put(l) for l in self.p.stdout] + [self.q.put(None)], daemon=True).start()

    def call(self, name: str, args: dict) -> tuple[str, bool]:
        if name != "python" or "code" not in args:
            return "error: the only tool is python(code)", True
        try:
            self.p.stdin.write(json.dumps({"code": args["code"]}) + "\n")
            self.p.stdin.flush()
            while True:  # the reply is the next JSON line with "out" (model code may print to the real stdout)
                line = self.q.get(timeout=self.LIMIT + 15)
                if line is None:
                    raise EOFError
                try:
                    r = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if isinstance(r, dict) and "out" in r:
                    return clip(r["out"], PANDAS_MAX_OUT) or "(no output)", r["error"]
        except (queue.Empty, EOFError, BrokenPipeError):
            self.close()
            self.start()
            return "[the Python process was killed (time or memory limit) and restarted; variables are lost]", True

    def close(self) -> None:
        self.p.kill()
        self.p.wait()


def markdown(root: Path, rel: str, model: str, budget: int, cache: Path) -> tuple[str, dict]:
    """The file as markdown (MarkItDown; a streaming openpyxl fallback when it fails), cut at the
    model's context budget. Returns the text and what was done (converter, tokens, truncation)."""
    if not budget:
        budget = next((v for k, v in CONTEXT.items() if model.startswith(k)), 128_000) - 24_000
    key = rel.replace("/", "__") + f".{budget}"
    md_p, info_p = cache / (key + ".md"), cache / (key + ".json")
    if not md_p.exists():
        cache.mkdir(parents=True, exist_ok=True)
        work = cache / (key + ".work")
        shutil.rmtree(work, ignore_errors=True)
        work.mkdir()
        copy_files(root, [rel], work)
        tk = SCRATCH / "tiktoken-cache"  # o200k_base, fetched once outside the sandbox (no network inside)
        if not tk.exists():
            subprocess.run([str(MARKITDOWN_PY), "-I", "-c", "import tiktoken; tiktoken.get_encoding('o200k_base')"],
                           env={"PATH": "/usr/bin", "TIKTOKEN_CACHE_DIR": str(tk)}, check=True)
        cmd = sandbox(work, python_ro(MARKITDOWN_PY) + [HERE, tk], mem="4294967296", secs=1800,
                      env={"TIKTOKEN_CACHE_DIR": str(tk)}) + [
            str(MARKITDOWN_PY), "-I", str(HERE / "md_convert.py"), rel, str(budget)]
        p = subprocess.run(cmd, capture_output=True, text=True)
        if p.returncode:
            raise RuntimeError(f"markdown conversion failed ({p.returncode}): {p.stderr[-1500:]}")
        info = json.loads(p.stdout.strip().splitlines()[-1])
        shutil.move(work / "out.md", md_p)
        info_p.write_text(json.dumps(info))
        shutil.rmtree(work, ignore_errors=True)
    return md_p.read_text(), json.loads(info_p.read_text())
