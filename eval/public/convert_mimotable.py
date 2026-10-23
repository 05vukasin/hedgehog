"""MiMoTable -> items.jsonl (one line per question).

    python3 -I eval/public/convert_mimotable.py        # after fetch_mimotable.py

MiMoTable's answers are free text written by annotators ("Based on the data, the applicant is
"Xia Zhu"."), not values. The items keep the benchmark's own labels (language, table_type,
meta_operation_list, difficulty) and the answer's *atoms*: quoted phrases and numbers that the
question does not already contain (`common.atoms_from_narrative`). score.py keeps only the atoms
that are a cell of the question's spreadsheet(s) (raw cells, `hedgehog cells`), so narrative numbers
that are not in the file never count.

Task classes (from meta_operation_list): `lookup` = only Lookup; `needs_query` = Calculate and/or
Compare (with or without Lookup) and nothing else; `not_measured` = any Reasoning, Visualize or Edit.
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import atoms_from_narrative, data_dir  # noqa: E402

ROOT = data_dir() / "mimotable"


def task_class(ops: list[str]) -> str:
    s = set(ops)
    if s & {"Reasoning", "Visualize", "Edit"}:
        return "not_measured"
    if s == {"Lookup"}:
        return "lookup"
    return "needs_query" if s & {"Calculate", "Compare"} else "not_measured"


def main() -> None:
    items = []
    for lang in ("english", "chinese"):
        base = ROOT / "repo" / "data" / lang
        for n, line in enumerate((base / "problems.json").read_text().splitlines()):
            if not line.strip():
                continue
            q = json.loads(line)
            files = [f"repo/data/{lang}/{f.removeprefix('./')}" for f in q["spreadsheet_list"]]
            items.append({"id": f"{lang[:2]}-{n:04d}", "lang": lang, "files": files, "question": q["question"],
                          "answer": q.get("answer", ""), "table_type": q["table_type"], "table_difficulty": q["table_difficulty"],
                          "ops": q["meta_operation_list"], "task": task_class(q["meta_operation_list"]),
                          "missing": [f for f in files if not (ROOT / f).exists()],
                          "atoms": atoms_from_narrative(q.get("answer", ""), q["question"])})
    (ROOT / "items.jsonl").write_text("".join(json.dumps(x, ensure_ascii=False) + "\n" for x in items))
    from collections import Counter
    print(len(items), Counter(x["task"] for x in items), sum(bool(x["missing"]) for x in items), "with missing files")


if __name__ == "__main__":
    main()
