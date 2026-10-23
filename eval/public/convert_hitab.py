"""HiTab -> trimmed workbooks, structure labels and value-linking items.

    python3 -I eval/public/convert_hitab.py            # after fetch_hitab.py; writes into <data>/public/hitab/

1. Trimmed copies (`clean/<source>/<table_id>.xlsx`): HiTab's original files are the annotators'
   workbooks; below each table they hold the sentence, the question, the answer and its formula.
   `sandboxed/hitab_trim.py` (inside bubblewrap) keeps only the table's sheet and cuts that block, so
   the files are the published StatCan / NSF tables again (title, header, body, footnotes).
   ToTTo tables (Wikipedia, no spreadsheet) and the 60 legacy .xls originals are skipped.
2. Alignment: HiTab's table matrix (tables/raw/<id>.json) is placed on the sheet with the offset its
   samples' `reference_cells_map` gives (B9 <-> (6, 1)), and kept only when >= 80 % of its non-empty
   texts equal the sheet's cells there (raw cells read with `hedgehog cells`).
3. `labels.tsv` + `tables.tsv` (coarse cell roles + table rectangles, for `hedgehog benchmark --corpora
   public:hitab`): rows above the matrix = title, the header rows (`header_depth`) = header, left
   header cells = data (group_header when the rest of the row is empty), the body = data. Cells below
   the matrix (footnotes) are not labelled.
4. `items.jsonl`: one line per QA sample on a kept table, with the answer cells' sheet coordinates,
   the gold column-header path (top tree) and row-header path (left tree, header-row nodes dropped).
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import (HERE, col_letter, data_dir, hedgehog_bin, norm_text, parse_a1, parse_number,  # noqa: E402
                    sandbox_cmd, swap_dir, work_dir)

ROOT = data_dir() / "hitab"
CLEAN = ROOT / "clean"


def raw_cells(path: Path) -> dict[tuple[int, int], str]:
    out = subprocess.run([hedgehog_bin(), "cells", str(path)], capture_output=True, text=True, timeout=120)
    cells = {}
    for line in out.stdout.splitlines():
        f = line.split("\t", 4)
        if len(f) == 5:
            cells[(int(f[1]), int(f[2]))] = f[4]
    return cells


def same(a: str, b: str) -> bool:
    if norm_text(a) == norm_text(b):
        return True
    x, y = parse_number(a), parse_number(b)
    return x is not None and y is not None and abs(x[0] - y[0]) <= 1e-9 * max(1.0, abs(x[0]))


def tree_paths(root: dict, key: str) -> dict[int, list[tuple[int, int]]]:
    """index (column for the top tree, row for the left tree) -> deepest node's path [(row, col), ...]."""
    best: dict[int, list[tuple[int, int]]] = {}

    def walk(n, path):
        here = path + [(n["row_index"], n["column_index"])] if n["row_index"] >= 0 else path
        if n["row_index"] >= 0:
            i = n[key]
            if i not in best or len(here) >= len(best[i]):
                best[i] = here
        for c in n["children"]:
            walk(c, here)

    walk(root, [])
    return best


def header_depth(tj: dict, L: int) -> int:
    """Header rows of a HiTab table. `top_header_rows_num` is one more than the top tree's depth in
    almost every table (NSF tables with one header row say 2), so: the top tree's depth, plus the rows
    right below it whose label columns are empty and whose other cells hold no number (unit rows such
    as 'percent'; empty spacer rows have no labels and do not matter)."""
    texts = tj["texts"]
    depth = 0

    def walk(n):
        nonlocal depth
        if n["row_index"] >= 0:
            depth = max(depth, n["row_index"] + 1)
        for c in n["children"]:
            walk(c)

    walk(tj["top_root"])
    h = depth
    while h < len(texts):
        row = [str(x).strip() for x in texts[h]]
        if any(row[:L]) or any(parse_number(x) for x in row[L:]):
            break
        h += 1
    return h


def original_path(source: str, table_id: str) -> Path | None:
    o = ROOT / "original"
    if source == "statcan":
        p = o / "statcan" / "Table Sentences Rephrasing-StatCan" / f"{table_id}.xlsx"
        return p if p.exists() else None
    if source == "nsf":
        _, folder, stem = table_id.split("_", 2)
        hits = sorted((o / "nsf" / "Table Sentences Rephrasing-NSF" / folder).glob(f"{stem}.xlsx"))
        return hits[0] if hits else None
    return None


def main() -> None:
    samples = []
    for split in ("train", "dev", "test"):
        for line in (ROOT / "data" / f"{split}_samples.jsonl").read_text().splitlines():
            s = json.loads(line)
            s["split"] = split
            samples.append(s)
    by_table: dict[tuple[str, str], list[dict]] = defaultdict(list)
    for s in samples:
        by_table[(s["table_source"], s["table_id"])].append(s)
    stats = Counter()
    jobs = []
    for (src, tid) in sorted(by_table):
        if src == "totto":
            stats["tables_totto_no_spreadsheet"] += 1
            continue
        p = original_path(src, tid)
        if p is None:
            stats["tables_no_xlsx_original"] += 1
            continue
        jobs.append({"id": f"{src}/{tid}", "src": str(p), "dst": f"{src}/{tid}.xlsx"})
    # 1. trim in the sandbox
    wd = work_dir() / "hitab_trim"
    wd.mkdir(parents=True, exist_ok=True)
    CLEAN.mkdir(parents=True, exist_ok=True)
    (wd / "jobs.json").write_text(json.dumps(jobs))
    py = swap_dir() / "venv-pandas" / "bin" / "python"
    script = HERE / "sandboxed" / "hitab_trim.py"
    cmd = sandbox_cmd(CLEAN, [ROOT / "original", wd, HERE / "sandboxed"], py, mem_gb=3, secs=3600)
    cmd += [str(py), "-I", str(script), str(wd / "jobs.json"), str(wd / "trim.jsonl")]
    # results.jsonl must be writable: write it in /work (the clean folder) and move it after
    cmd[-1] = "/work/.trim.jsonl"
    subprocess.run(cmd, check=True)
    trim = {}
    for line in (CLEAN / ".trim.jsonl").read_text().splitlines():
        r = json.loads(line)
        trim[r["id"]] = r
    # 2-4. align, label, items
    labels = ["corpus\tfile\tsheet\trow\tcol\trole"]
    tables = ["corpus\tfile\tsheet\ttop\tleft\tbottom\tright"]
    items = []
    tmeta = []
    for j in jobs:
        tr = trim.get(j["id"], {})
        if not tr.get("ok"):
            stats["tables_trim_failed"] += 1
            continue
        src, tid = j["id"].split("/", 1)
        tj = json.loads((ROOT / "tables" / "tables" / "raw" / f"{tid}.json").read_text())
        texts = tj["texts"]
        L = tj["left_header_columns_num"]
        R, C = len(texts), max(len(r) for r in texts)
        H = header_depth(tj, L)
        path = CLEAN / j["dst"]
        cells = raw_cells(path)
        # offset candidates from the samples' reference maps
        cands = Counter()
        for s in by_table[(src, tid)]:
            for a1, ij in (s.get("reference_cells_map") or {}).items():
                try:
                    r0, c0 = parse_a1(a1)
                except ValueError:
                    continue
                i, k = (int(x) for x in re.findall(r"-?\d+", ij)[:2])
                cands[(r0 - i, c0 - k)] += 1
        nonempty = [(i, k) for i in range(R) for k in range(len(texts[i])) if str(texts[i][k]).strip()]

        def score(dr, dc):
            return sum(same(texts[i][k], cells.get((i + dr, k + dc), "")) for i, k in nonempty) / max(1, len(nonempty))

        best = max(((score(dr, dc), dr, dc) for (dr, dc), _ in cands.most_common(3)), default=(0.0, 0, 0))
        if best[0] < 0.8:  # search near the top-left corner
            best = max([best] + [(score(dr, dc), dr, dc) for dr in range(0, 15) for dc in range(0, 4)])
        if best[0] < 0.8:
            stats["tables_not_aligned"] += 1
            continue
        _, dr, dc = best
        stats["tables_kept"] += 1
        corpus = f"hitab-{src}"
        rel = f"clean/{j['dst']}"
        sheet = tr["sheet"]
        tables.append(f"{corpus}\t{rel}\t{sheet}\t{dr}\t{dc}\t{dr + R - 1}\t{dc + C - 1}")
        for (r, c), v in sorted(cells.items()):
            if r < dr:
                role = "title"
            elif r < dr + H and dc <= c < dc + C:
                role = "header"
            elif r < dr + R and dc <= c < dc + C:
                if c < dc + L:
                    rest = any(cells.get((r, cc), "").strip() for cc in range(dc + L, dc + C))
                    role = "data" if rest else "group_header"
                else:
                    role = "data"
            else:
                continue
            labels.append(f"{corpus}\t{rel}\t{sheet}\t{r}\t{c}\t{role}")
        tops = tree_paths(tj["top_root"], "column_index")
        lefts = tree_paths(tj["left_root"], "row_index")
        txt = lambda rc: str(texts[rc[0]][rc[1]]) if rc[0] < R and rc[1] < len(texts[rc[0]]) else ""
        tmeta.append({"table": j["id"], "file": rel, "sheet": sheet, "offset": [dr, dc], "rows": R, "cols": C,
                      "header_rows": H, "left_cols": L, "align": round(best[0], 3), "title": tj.get("title", "")})
        for s in by_table[(src, tid)]:
            ql = s["linked_cells"]["quantity_link"].get("[ANSWER]", {})
            el = [v.get("[ANSWER]") for v in s["linked_cells"]["entity_link"].values() if isinstance(v, dict)]
            if any(el) or any("[ANSWER]" in v for v in s["linked_cells"]["entity_link"].values()):
                kind = "header_answer"
            elif s["aggregation"] == ["none"] and len(ql) == 1:
                kind = "lookup"
            elif ql:
                kind = "computed"
            else:
                kind = "unlinked"
            acells = []
            for ij in ql:
                i, k = (int(x) for x in re.findall(r"-?\d+", ij)[:2])
                r, c = i + dr, k + dc
                acells.append({"row": r, "col": c, "a1": f"{col_letter(c)}{r + 1}", "matrix": [i, k],
                               "text": txt((i, k)), "raw": cells.get((r, c), ""),
                               "top_path": [txt(n) for n in tops.get(k, []) if n[0] < H],
                               "left_path": [txt(n) for n in lefts.get(i, []) if n[0] >= H],
                               "in_header_rows": i < H, "in_left_cols": k < L})
            items.append({"id": s["id"], "split": s["split"], "table": j["id"], "file": rel, "sheet": sheet,
                          "question": s["question"], "answer": s["answer"], "aggregation": s["aggregation"],
                          "kind": kind, "cells": acells})
    (ROOT / "labels.tsv").write_text("\n".join(labels) + "\n")
    (ROOT / "tables.tsv").write_text("\n".join(tables) + "\n")
    (ROOT / "items.jsonl").write_text("".join(json.dumps(x, ensure_ascii=False) + "\n" for x in items))
    (ROOT / "tables_meta.jsonl").write_text("".join(json.dumps(x, ensure_ascii=False) + "\n" for x in tmeta))
    stats["items"] = len(items)
    stats.update(f"items_{x['kind']}" for x in items)
    (ROOT / "convert_stats.json").write_text(json.dumps(stats, indent=1))
    print(json.dumps(stats, indent=1))


if __name__ == "__main__":
    main()
