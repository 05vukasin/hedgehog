"""Run the naive baselines (pandas read_excel header=0; MarkItDown) over a dataset's files, in bubblewrap.

    python3 -I eval/public/run_baselines.py mimotable [--only pandas|markitdown] [--limit N]

Uses the pinned venvs of eval/agents/swap/setup.sh under $SWAP_DIR (default <work>/swap). The sandbox
sees the dataset folder read-only and <work>/<dataset>/<mode>/ as /work; no network, empty environment,
address space capped at 3 GB. Resumable (files with a result are skipped). Run under run_capped.sh.
"""
import argparse
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from common import data_dir, sandbox_cmd, swap_dir, work_dir  # noqa: E402
from run_hedgehog import dataset_files, key_of  # noqa: E402

VENV = {"pandas": "venv-pandas", "markitdown": "venv-markitdown"}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("dataset")
    ap.add_argument("--only")
    ap.add_argument("--limit", type=int)
    a = ap.parse_args()
    root = data_dir() / a.dataset
    files = dataset_files(a.dataset)[: a.limit]
    for mode in ([a.only] if a.only else ["pandas", "markitdown"]):
        wd = work_dir() / a.dataset / mode
        wd.mkdir(parents=True, exist_ok=True)
        jobs = [{"key": key_of(f), "path": str(root / f)} for f in files
                if (root / f).stat().st_size <= 200 << 20]
        (wd / "jobs.json").write_text(json.dumps(jobs))
        py = swap_dir() / VENV[mode] / "bin" / "python"
        cmd = sandbox_cmd(wd, [root, HERE / "sandboxed"], py, mem_gb=3, secs=6 * 3600)
        cmd += [str(py), "-I", str(HERE / "sandboxed" / "baseline_dump.py"), mode, "/work/jobs.json"]
        print(f"{a.dataset} {mode}: {len(jobs)} files", flush=True)
        subprocess.run(cmd, check=True)


if __name__ == "__main__":
    main()
