"""Fetch RealHiTBench (Wu et al., ACL 2025 Findings, arXiv 2506.13405) from the authors' Hugging Face dataset.

    python3 -I eval/public/fetch_realhitbench.py [--record]

Source: https://huggingface.co/datasets/spzy/RealHiTBench (linked from github.com/cspzyy/RealHiTBench),
pinned revision, license CC BY-NC 4.0 (kept local, never committed). Downloads only the original
spreadsheets (tables/*.xlsx, including the authors' `_swap` variants) and the QA files; the html,
latex, markdown, csv and image renderings and the .py files in tables/ are not downloaded.
The file list comes from checksums/realhitbench.sha256 once recorded, else from the HF API.
"""
import json
import sys
import urllib.request
from pathlib import Path
from urllib.parse import quote

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from _fetch import UA, fetch  # noqa: E402

REV = "68ca8e50950dc0704c53968ff9a4a0456b585e96"
BASE = f"https://huggingface.co/datasets/spzy/RealHiTBench/resolve/{REV}"


def file_list() -> list[str]:
    sums = HERE / "checksums" / "realhitbench.sha256"
    if sums.exists():
        return [line.split("  ", 1)[1] for line in sums.read_text().splitlines() if line.strip()]
    req = urllib.request.Request(f"https://huggingface.co/api/datasets/spzy/RealHiTBench/tree/{REV}/tables",
                                 headers={"User-Agent": UA})
    tree = json.load(urllib.request.urlopen(req, timeout=60))
    xlsx = sorted(x["path"] for x in tree if x["type"] == "file" and x["path"].endswith(".xlsx"))
    return ["QA_final.json", "QA_long.json", "README.md", *xlsx]


if __name__ == "__main__":
    fetch("realhitbench", [(f"{BASE}/{quote(f)}", f) for f in file_list()], "--record" in sys.argv)
