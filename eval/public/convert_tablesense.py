"""TableSense table ranges -> <data>/public/tablesense/{labels.tsv,tables.tsv} (table detection only).

    python3 -I eval/public/convert_tablesense.py

Not a new download: the annotations (github.com/microsoft/TableSense, `dataset/Table range
annotations.txt`, O-UDA-1.0) and the VEnron2 / VEUSES / VFUSE files they label (SpreadCluster,
figshare, CC0) are already in <data>/irregular/{tablesense_annotations,venron_spreadcluster}/ (see
its MANIFEST.csv). The annotators converted .xls to .xlsx; the local copies are the .xls originals,
matched by file stem in the annotation's cluster folder. Sheets whose file is not on disk are
dropped (counted). No cell roles: `hedgehog benchmark --corpora public:tablesense` scores table
detection (P/R/F1 at IoU 0.5 / 0.8 / 1.0) only.
"""
import os
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import data_dir, parse_a1  # noqa: E402

IRR = data_dir().parent / "irregular"
OUT = data_dir() / "tablesense"


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    stats = Counter()
    rows = ["corpus\tfile\tsheet\ttop\tleft\tbottom\tright"]
    ann = (IRR / "tablesense_annotations" / "Table range annotations.txt").read_text(encoding="utf-8", errors="replace")
    for line in ann.splitlines():
        f = line.split("\t")
        if len(f) < 5:
            continue
        name, sheet, _split, folder = f[:4]
        parts = folder.replace("\\", "/").split("/")
        d = IRR / "venron_spreadcluster" / parts[0] / parts[-1]
        stem = os.path.splitext(name)[0]
        hits = sorted(x for x in os.listdir(d) if os.path.splitext(x)[0] == stem) if d.is_dir() else []
        if not hits:
            stats["sheets_file_missing"] += 1
            continue
        rel = os.path.relpath(d / hits[0], OUT)
        n = 0
        for reg in (x.strip() for x in f[4:] if x.strip()):
            try:
                a, b = reg.split(":")
                (r0, c0), (r1, c1) = parse_a1(a), parse_a1(b)
            except ValueError:
                stats["bad_range"] += 1
                continue
            rows.append(f"tablesense-{parts[0]}\t{rel}\t{sheet}\t{r0}\t{c0}\t{r1}\t{c1}")
            n += 1
        stats["sheets"] += 1
        stats["tables"] += n
    (OUT / "tables.tsv").write_text("\n".join(rows) + "\n")
    (OUT / "labels.tsv").write_text("corpus\tfile\tsheet\trow\tcol\trole\n")
    print(dict(stats))


if __name__ == "__main__":
    main()
