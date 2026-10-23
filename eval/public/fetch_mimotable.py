"""Fetch MiMoTable (Li et al., COLING 2025, arXiv 2412.11711) from the authors' GitHub repo.

    python3 -I eval/public/fetch_mimotable.py [--record]

Source: https://github.com/jasonNLP/MiMoTable (linked from the paper), MIT license, pinned commit.
Writes <data>/public/mimotable/MiMoTable-<sha>.tar.gz and extracts it to .../mimotable/repo/
(data/{english,chinese}/spreadsheet/*.xlsx and problems.json). Idempotent; sha256 pinned in
checksums/mimotable.sha256.
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from _fetch import extract, fetch  # noqa: E402

SHA = "96ff0bc7b38f4d79e709875296ea4919940b5fe3"
URL = f"https://codeload.github.com/jasonNLP/MiMoTable/tar.gz/{SHA}"

if __name__ == "__main__":
    root = fetch("mimotable", [(URL, f"MiMoTable-{SHA}.tar.gz")], "--record" in sys.argv)
    if not (root / "repo" / "data").exists():
        print(f"extracted {extract(root / f'MiMoTable-{SHA}.tar.gz', root / 'repo', strip=1)} files", file=sys.stderr)
