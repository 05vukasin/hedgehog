"""Fetch HiTab (Cheng et al., ACL 2022) from Microsoft's GitHub repo.

    python3 -I eval/public/fetch_hitab.py [--record]

Source: https://github.com/microsoft/HiTab, pinned commit; data license: Computational Use of Data
Agreement v1.0 (C-UDA; redistribution only with attribution and the same terms, so the data stays
local). Downloads the QA samples, tables.zip (the hierarchical table JSON) and the original
annotation archives, which hold the source Excel files (StatCan and NSF), and extracts them under
<data>/public/hitab/{tables,original/statcan,original/nsf}/.
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from _fetch import extract, fetch  # noqa: E402

SHA = "d179602662b490249baf068a76fbe4137029126e"
RAW = f"https://raw.githubusercontent.com/microsoft/HiTab/{SHA}"
FILES = ["data/train_samples.jsonl", "data/dev_samples.jsonl", "data/test_samples.jsonl", "data/tables.zip",
         "Hitab_original_annotations/Table Sentences Rephrasing-StatCan.zip",
         "Hitab_original_annotations/Table Sentences Rephrasing-NSF.zip", "LICENSE"]

if __name__ == "__main__":
    from urllib.parse import quote
    root = fetch("hitab", [(f"{RAW}/{quote(f)}", f) for f in FILES], "--record" in sys.argv)
    for zf, dst in [("data/tables.zip", "tables"),
                    ("Hitab_original_annotations/Table Sentences Rephrasing-StatCan.zip", "original/statcan"),
                    ("Hitab_original_annotations/Table Sentences Rephrasing-NSF.zip", "original/nsf")]:
        if not (root / dst).exists():
            print(f"{dst}: extracted {extract(root / zf, root / dst)} files", file=sys.stderr)
