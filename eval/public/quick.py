"""Tier-0 slice of the public benchmark (scripts/quick.sh part `public`): 14 HiTab tables, metric B.

    python3 -I eval/public/quick.py --bin target-quick/quick/hedgehog --data ../testdata --tmp DIR [--update]

The trimmed HiTab files of quick_tables.txt (12 picked by table id with >= 4 lookups, plus statcan/102
with the "French-language" header and statcan/107, a one-column table hedgehog finds no table in) go
through run_hedgehog (cells, normalize, open_file) with the given binary; their lookup questions are
scored like score.py's metric B. quick.tsv holds the counts; a count that drops (found, value equal,
column header exact, row labels exact / leaf) or catalog characters that grow by more than 5 % fail.
Skipped (exit 0) when the HiTab data is not on disk (run fetch_hitab.py and convert_hitab.py once).
Deterministic: same files, same binary, same counts.
"""
import argparse
import os
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
KEYS = ("lookups", "found", "value_equal", "col_exact", "row_exact", "row_leaf", "catalog_chars")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--data", required=True)
    ap.add_argument("--tmp", required=True)
    ap.add_argument("--update", action="store_true")
    a = ap.parse_args()
    os.environ["HEDGEHOG_DATA"] = str(Path(a.data).resolve())
    os.environ["HEDGEHOG_BIN"] = str(Path(a.bin).resolve())
    work = Path(a.tmp).resolve() / "public"
    os.environ["HH_PUBLIC_WORK"] = str(work)
    sys.path.insert(0, str(HERE))
    import run_hedgehog as R  # noqa: E402  (after the environment: paths are read at call time)
    import score as S  # noqa: E402
    from common import data_dir, read_jsonl  # noqa: E402

    items = read_jsonl(data_dir() / "hitab" / "items.jsonl")
    if not items:
        print("public: skipped (no HiTab data; run eval/public/fetch_hitab.py and convert_hitab.py)")
        return 0
    tables = [t for t in (HERE / "quick_tables.txt").read_text().split() if t]
    import shutil
    shutil.rmtree(work, ignore_errors=True)
    R.LAUNCHER = R.maxrss_launcher()
    by_table = {}
    for it in items:
        by_table.setdefault(it["table"], []).append(it)
    rows = {}
    for t in tables:
        its = by_table.get(t, [])
        if not its:
            continue
        f = its[0]["file"]
        R.one("hitab", f)
        meta, raw, catalog, hh_tables = S.hh_load("hitab", f)
        c = dict.fromkeys(KEYS, 0)
        c["catalog_chars"] = len(catalog)
        for it in its:
            if it["kind"] != "lookup" or it["cells"][0]["in_header_rows"]:
                continue
            c["lookups"] += 1
            r = S.link_one(hh_tables, it, raw)
            if not r["found"]:
                continue
            c["found"] += 1
            c["value_equal"] += r["value"] in ("equal", "equal (percent as fraction)")
            c["col_exact"] += bool(r["col_exact"])
            c["row_exact"] += bool(r["row_exact"])
            c["row_leaf"] += bool(r["row_leaf"])
        rows[t] = c
    tsv = HERE / "quick.tsv"
    text = "table\t" + "\t".join(KEYS) + "\n" + "".join(f"{t}\t" + "\t".join(str(c[k]) for k in KEYS) + "\n" for t, c in rows.items())
    tot = {k: sum(c[k] for c in rows.values()) for k in KEYS}
    line = (f"{len(rows)} tables, {tot['lookups']} lookups: found {tot['found']}, value {tot['value_equal']}, "
            f"column header {tot['col_exact']}, row labels {tot['row_exact']} (leaf {tot['row_leaf']}), catalogs {tot['catalog_chars']:,} chars")
    if a.update or not tsv.exists():
        tsv.write_text(text)
        print(f"public: {line} (stored in eval/public/quick.tsv)")
        return 0
    old = {}
    for ln in tsv.read_text().splitlines()[1:]:
        f = ln.split("\t")
        old[f[0]] = dict(zip(KEYS, map(int, f[1:])))
    bad = []
    for t, c in rows.items():
        o = old.get(t)
        if not o:
            continue
        for k in KEYS[1:-1]:
            if c[k] < o[k]:
                bad.append(f"{t}: {k} {o[k]} -> {c[k]}")
        if c["catalog_chars"] > o["catalog_chars"] * 1.05:
            bad.append(f"{t}: catalog {o['catalog_chars']} -> {c['catalog_chars']} chars (> +5 %)")
    if bad:
        print(f"public FAILED: {line}")
        for b in bad:
            print(f"  {b}")
        return 1
    changed = text != tsv.read_text()
    print(f"public OK: {line}" + (" (better than stored; make quick-update to keep it)" if changed else ""))
    return 0


if __name__ == "__main__":
    sys.exit(main())
