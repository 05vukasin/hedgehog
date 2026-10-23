"""Metric A: structure scores on the public corpora with `hedgehog benchmark` (no new scorer).

    python3 -I eval/public/run_structure.py [--bin target-public/release/hedgehog] [--corpora hitab,tablesense]

Runs `hedgehog benchmark --corpora public:<name>,... --analyzer A --out <work>/structure_A.tsv` for the
analyzers `model` (rules + the learned row model: what normalize and the MCP server use), `layout`
(rules only) and `baseline` (8-connected flood fill: the number to beat). The corpora are the
labels.tsv / tables.tsv that convert_hitab.py and convert_tablesense.py write. Needs a hedgehog
built from this branch or later (the `public:` corpora and the header-depth TSV columns).
score.py renders the TSVs into RESULTS.md. Run it under scripts/run_capped.sh.
"""
import argparse
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import REPO, data_dir, hedgehog_bin, work_dir  # noqa: E402

ANALYZERS = ("model", "layout", "baseline")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", default=hedgehog_bin())
    ap.add_argument("--corpora", default="hitab,tablesense")
    ap.add_argument("--threads", default="2")
    a = ap.parse_args()
    corpora = ",".join(f"public:{c}" for c in a.corpora.split(",") if (data_dir() / c / "labels.tsv").exists())
    for an in ANALYZERS:
        out = work_dir() / f"structure_{an}.tsv"
        subprocess.run([a.bin, "benchmark", "--corpora", corpora, "--analyzer", an, "--threads", a.threads,
                        "--out", str(out)], check=True, cwd=REPO, env={"HEDGEHOG_DATA": str(data_dir().parent),
                                                                      "PATH": "/usr/bin:/bin"})


if __name__ == "__main__":
    main()
