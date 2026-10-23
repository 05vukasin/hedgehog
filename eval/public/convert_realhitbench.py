"""RealHiTBench -> items.jsonl (one line per QA pair of QA_final.json and QA_long.json).

    python3 -I eval/public/convert_realhitbench.py     # after fetch_realhitbench.py

The spreadsheet is tables/<FileName>.xlsx (the authors' original-format file; the `_swap` variants
are not referenced by the QA files). `ProcessedAnswer` is short: it is split into atoms on ', ', ';'
and line breaks (`common.atoms_from_short`). Task classes: the benchmark's QuestionType; Fact
Checking, Numerical Reasoning and Structure Comprehending are measured, Data Analysis and
Visualization (free-text analyses, chart code) are `not_measured`. Answers over 200 characters are
`not_measured` too (not a value or a short list). `long` marks QA_long.json (tables the authors set
aside as too large for an LLM prompt).
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import atoms_from_short, data_dir  # noqa: E402

ROOT = data_dir() / "realhitbench"
MEASURED = {"Fact Checking", "Numerical Reasoning", "Structure Comprehending"}


def main() -> None:
    items = []
    for fname, long in (("QA_final.json", False), ("QA_long.json", True)):
        for q in json.loads((ROOT / fname).read_text())["queries"]:
            ans = str(q.get("ProcessedAnswer") or q.get("FinalAnswer") or "").strip()
            f = f"tables/{q['FileName']}.xlsx"
            task = q["QuestionType"] if q["QuestionType"] in MEASURED and 0 < len(ans) <= 200 else "not_measured"
            items.append({"id": f"{'L' if long else 'F'}{q['id']}", "files": [f], "question": q["Question"], "answer": ans,
                          "qtype": q["QuestionType"], "subtype": q["SubQType"], "structure": q["CompStrucCata"],
                          "long": long, "task": task, "missing": [] if (ROOT / f).exists() else [f],
                          "atoms": atoms_from_short(ans) if task != "not_measured" else []})
    (ROOT / "items.jsonl").write_text("".join(json.dumps(x, ensure_ascii=False) + "\n" for x in items))
    from collections import Counter
    print(len(items), Counter(x["task"] for x in items), sum(bool(x["missing"]) for x in items), "with missing files")


if __name__ == "__main__":
    main()
