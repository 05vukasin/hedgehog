"""Clean source tables: read CSV/XLSX/ODS (parse only, never execute), infer simple semantic
column types, and cache a bounded sample per dataset as JSON.

The cache keeps up to ``CHUNKS`` contiguous row chunks per dataset (contiguous so groups such as
"all rows of one country" survive, which totals/crosstab operators need) with their original row
numbers, so provenance can point back into the original file.
"""
from __future__ import annotations

import csv
import datetime as dt
import json
import random
import re
from dataclasses import dataclass, field
from functools import lru_cache
from pathlib import Path

CHUNKS = 4
CHUNK_ROWS = 300
MAX_COLS = 30
MAX_CELL_LEN = 120
csv.field_size_limit(1 << 20)

# Semantic types the generator understands. Numeric ones are rendered with number operators,
# date ones with date operators, the rest are text.
NUMERIC = {"int", "float", "money", "percent", "count", "lat", "lon"}
DATE = {"date", "datetime", "month"}
TEXTUAL = {"category", "city", "country", "state", "name", "text", "id", "code", "url"}
ALL_TYPES = sorted(NUMERIC | DATE | TEXTUAL | {"year", "bool"})


@dataclass
class Column:
    name: str
    stype: str  # generator type (drives which operators apply)
    semantic: str | None = None  # semantic type from testdata/clean/MANIFEST.csv (ground truth)


# MANIFEST.csv semantic type → generator types it may map to (first compatible wins)
MANIFEST_MAP = {
    "quantity": ("count", "int", "float"), "percent": ("percent",), "id": ("id",), "category": ("category",),
    "currency_amount": ("money",), "region": ("state",), "date": ("date",), "boolean": ("bool",),
    "country": ("country",), "company": ("name",), "city": ("city",), "status": ("category",), "address": ("text",),
    "person_name": ("name",), "year": ("year",), "price": ("money",), "text": ("text",), "latitude": ("lat",),
    "longitude": ("lon",), "datetime": ("datetime", "date"), "phone": ("code",), "product": ("category", "name"),
    "month": ("month", "category"), "tax_id": ("id",), "email": ("text",), "quarter": ("category",),
}


def _compatible(st: str, kinds) -> bool:
    vals = [k for k, _ in kinds if k != "empty"]
    if not vals:
        return False
    num = sum(1 for k in vals if k in ("int", "float")) / len(vals)
    dat = sum(1 for k in vals if k in ("date", "datetime", "month")) / len(vals)
    if st in NUMERIC or st == "year":
        return num >= 0.9 and (st not in ("count", "int", "year") or all(k == "int" for k in vals if k in ("int", "float")))
    if st in DATE:
        return dat >= 0.9
    if st == "bool":
        return sum(1 for k in vals if k == "bool") / len(vals) >= 0.9
    return num < 0.9 and dat < 0.9 or st in ("id", "code")


def manifest_stype(sem: str, name: str, kinds) -> str:
    for st in MANIFEST_MAP.get(sem, ()):
        if _compatible(st, kinds):
            return st
    return infer_type(name, kinds)


@dataclass
class Source:
    name: str  # path relative to the clean root
    title: str
    columns: list[Column]
    rows: list[list]  # typed values (None = empty)
    row_idx: list[int]  # original 0-based data-row number of each row
    n_rows_total: int
    chunks: list[tuple[int, int]] = field(default_factory=list)  # [start, end) into rows


# ---------------------------------------------------------------- parsing of raw strings

_INT = re.compile(r"^[+-]?\d+$")
_FLOAT = re.compile(r"^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$")
_ISO_DATE = re.compile(r"^(\d{4})-(\d{2})-(\d{2})$")
_ISO_DT = re.compile(r"^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2})(:(\d{2}))?")
_ISO_MONTH = re.compile(r"^(\d{4})-(\d{2})$")
_US_DATE = re.compile(r"^(\d{1,2})/(\d{1,2})/(\d{2}|\d{4})$")
_MON_DATE = re.compile(r"^([A-Za-z]{3})\w* (\d{1,2}),? (\d{4})$")
_MONTHS = {m: i + 1 for i, m in enumerate(["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"])}
_BOOL = {"true": True, "false": False, "yes": True, "no": False}


def _year2(y: int) -> int:
    return y + (2000 if y < 30 else 1900) if y < 100 else y


def parse_scalar(s):
    """Raw cell → (kind, value). Kinds: empty, int, float, date, datetime, month, bool, text."""
    if s is None:
        return "empty", None
    if isinstance(s, bool):
        return "bool", s
    if isinstance(s, int):
        return "int", s
    if isinstance(s, float):
        if s != s or s in (float("inf"), float("-inf")):
            return "empty", None
        return ("int", int(s)) if s.is_integer() and abs(s) < 1e15 else ("float", s)
    if isinstance(s, dt.datetime):
        return ("date", s.date()) if s.time() == dt.time() else ("datetime", s.replace(microsecond=0))
    if isinstance(s, dt.date):
        return "date", s
    if isinstance(s, dt.time):
        return "text", s.isoformat()
    t = str(s).strip()
    if not t or t.lower() in ("nan", "null", "none", "na", "n/a", "#n/a"):
        return "empty", None
    if len(t) > MAX_CELL_LEN:
        t = t[:MAX_CELL_LEN].rstrip()
    if _INT.match(t):
        if len(t) > 1 and t.lstrip("+-").startswith("0"):
            return "text", t  # leading zeros: an identifier, not a number
        v = int(t)
        return ("int", v) if abs(v) < 1e15 else ("text", t)
    if _FLOAT.match(t):
        x = float(t)
        return ("float", x) if x not in (float("inf"), float("-inf")) else ("text", t)
    try:
        if m := _ISO_DATE.match(t):
            return "date", dt.date(int(m[1]), int(m[2]), int(m[3]))
        if m := _ISO_DT.match(t):
            v = dt.datetime(int(m[1]), int(m[2]), int(m[3]), int(m[4]), int(m[5]), int(m[7] or 0))
            return ("date", v.date()) if v.time() == dt.time() else ("datetime", v)
        if m := _ISO_MONTH.match(t):
            return "month", dt.date(int(m[1]), int(m[2]), 1)
        if m := _US_DATE.match(t):
            return "date", dt.date(_year2(int(m[3])), int(m[1]), int(m[2]))
        if (m := _MON_DATE.match(t)) and m[1].lower() in _MONTHS:
            return "date", dt.date(int(m[3]), _MONTHS[m[1].lower()], int(m[2]))
    except ValueError:
        pass
    if t.lower() in _BOOL:
        return "bool", _BOOL[t.lower()]
    return "text", t


# ---------------------------------------------------------------- semantic type inference

_HDR = [
    ("lat", {"lat", "latitude"}),
    ("lon", {"lon", "lng", "long", "longitude"}),
    ("year", {"year", "yr", "god", "godina", "jahr", "annee", "ano", "anno", "jaar", "season"}),
    ("percent", {"pct", "percent", "percentage", "share", "prct", "%"}),
    ("money", {"price", "gdp", "income", "cost", "amount", "revenue", "budget", "salary", "wage", "usd", "eur",
               "rsd", "iznos", "dinar", "open", "close", "high", "low", "adjusted", "bill", "tip", "sales",
               "profit", "premiums", "losses", "gdppercap", "rashod", "prihod", "sredstva"}),
    ("city", {"city", "town", "municipality", "strcity", "grad", "opstina", "mesto", "nter", "opstinanaziv1"}),
    ("country", {"country", "nation", "drzava"}),
    ("state", {"state", "st", "strstate", "province", "region", "okrug"}),
    ("id", {"id", "code", "zip", "iata", "icao", "ident", "symbol", "cik", "bioguide", "url", "pib", "mbr"}),
    ("name", {"name", "naziv", "firstname", "lastname", "ime", "prezime", "film", "title", "airline", "major",
              "security", "airport"}),
    ("count", {"count", "number", "n", "total", "population", "pop", "births", "deaths", "cnt", "size",
               "appearances", "employed", "men", "women", "fatalities", "broj", "vrednost", "quantity", "units"}),
]


def header_tokens(name: str) -> set[str]:
    s = re.sub(r"([a-z])([A-Z])", r"\1 \2", name)
    toks = {t for t in re.split(r"[^0-9A-Za-zÀ-ž%]+", s.lower()) if t}
    return toks | ({"%"} if "%" in name else set())


def infer_type(name: str, kinds: list[tuple[str, object]]) -> str:
    vals = [(k, v) for k, v in kinds if k != "empty"]
    if not vals:
        return "text"
    n = len(vals)
    cnt: dict[str, int] = {}
    for k, _ in vals:
        cnt[k] = cnt.get(k, 0) + 1
    top, topn = max(cnt.items(), key=lambda kv: kv[1])
    toks = header_tokens(name)
    hint = next((t for t, words in _HDR if toks & words), None)
    if topn < 0.9 * n:
        if cnt.get("int", 0) + cnt.get("float", 0) >= 0.9 * n:
            top = "float"
        else:
            return "text"
    if top == "bool":
        return "bool"
    if top in ("date", "datetime", "month"):
        return top
    if top in ("int", "float"):
        nums = [float(v) for k, v in vals if k in ("int", "float")]
        lo, hi = min(nums), max(nums)
        if hint == "year":
            return "year" if top == "int" and 1500 <= lo and hi <= 2200 else "int"
        if hint in ("lat", "lon"):
            return hint
        if hint == "id":
            return "id"
        if hint == "percent" and lo >= -100 and hi <= 100:
            return "percent"
        if hint == "money":
            return "money"
        if top == "int" and hint == "count" and lo >= 0:
            return "count"
        return top
    # text
    distinct = len({v for _, v in vals})
    if hint in ("city", "country", "state"):
        return hint
    if hint == "id":
        return "id"
    if any(isinstance(v, str) and v.startswith("http") for _, v in vals[:20]):
        return "url"
    avg = sum(len(str(v)) for _, v in vals) / n
    if distinct <= max(12, 0.1 * n) and avg < 40:
        return "category"
    if hint == "name":
        return "name"
    if all(isinstance(v, str) and len(v) <= 8 and " " not in v for _, v in vals[:200]) and distinct > 0.5 * n:
        return "code"
    return "text"


def coerce(stype: str, kind: str, v):
    """Typed value for a column of `stype`, or the text if it does not fit."""
    if kind == "empty":
        return None
    if stype in NUMERIC or stype == "year":
        if kind in ("int", "float"):
            if stype == "year":
                return int(v)
            return v
        return None
    if stype in DATE:
        if kind in ("date", "datetime", "month"):
            return v
        return None
    if stype == "bool":
        return v if kind == "bool" else None
    if kind in ("int", "float") and stype in ("id", "code"):
        return v  # numeric identifiers stay numbers (as in real sheets); leading-zero ones are text already
    if kind in ("date", "datetime", "month"):
        return v.isoformat()
    return v if isinstance(v, str) else str(v)


# ---------------------------------------------------------------- readers


_COMMA_NUM = re.compile(r"^-?\d{1,3}(\.\d{3})*,\d+$|^-?\d+,\d+$")


def _read_csv_rows(path: Path):
    with open(path, newline="", encoding="utf-8-sig", errors="replace") as f:
        sample = f.read(4096)
        f.seek(0)
        delim = ";" if sample.count(";") > sample.count(",") else ","
        for row in csv.reader(f, delimiter=delim):
            if delim == ";":  # semicolon CSVs use decimal commas: parse them as numbers
                row = [x.replace(".", "").replace(",", ".") if _COMMA_NUM.match(x.strip()) else x for x in row]
            yield row


def _read_book_rows(path: Path):
    from python_calamine import CalamineWorkbook

    wb = CalamineWorkbook.from_path(str(path))
    sh = wb.get_sheet_by_index(0)
    for row in sh.iter_rows():
        yield [None if (isinstance(x, str) and not x.strip()) else x for x in row]


def _detect_header(head: list[list]) -> int:
    """Index of the header row among the first rows of a spreadsheet: the first row whose
    filled count is ≥ 70% of the widest row and is all text."""
    widths = [sum(1 for x in r if x not in (None, "")) for r in head]
    if not widths:
        return 0
    w = max(widths)
    for i, r in enumerate(head):
        filled = [x for x in r if x not in (None, "")]
        if len(filled) >= 0.7 * w and all(isinstance(x, str) for x in filled):
            return i
    return 0


def load_raw(path: Path, rng: random.Random, header_row: int | None = None):
    """(header, chunked rows, row numbers, total rows). Two streaming passes, bounded memory."""
    reader = _read_csv_rows if path.suffix.lower() == ".csv" else _read_book_rows
    head = []
    for r in reader(path):
        head.append(r)
        if len(head) >= 40:
            break
    h = header_row if header_row is not None else (_detect_header(head) if path.suffix.lower() != ".csv" else 0)
    header = [str(x).strip() if x not in (None, "") else "" for x in head[h]] if head else []
    total = -h - 1
    for _ in reader(path):
        total += 1
    total = max(total, 0)
    if total <= CHUNKS * CHUNK_ROWS:
        starts = [0]
        span = total
    else:
        span = CHUNK_ROWS
        cands = sorted(rng.sample(range(0, total - CHUNK_ROWS, CHUNK_ROWS), min(CHUNKS, (total - CHUNK_ROWS) // CHUNK_ROWS)))
        starts = cands or [0]
    want = set()
    for s in starts:
        want.update(range(s, min(s + span, total)))
    rows, idx = [], []
    for i, r in enumerate(reader(path)):
        d = i - h - 1
        if d in want:
            rows.append(r)
            idx.append(d)
    chunks = []
    for s in starts:
        a = next((k for k, d in enumerate(idx) if d >= s), len(idx))
        b = next((k for k, d in enumerate(idx) if d >= s + span), len(idx))
        if b > a:
            chunks.append((a, b))
    return header, rows, idx, total, chunks


def build_source(path: Path, root: Path, manifest: dict | None, seed: int = 7) -> Source | None:
    rng = random.Random(f"{seed}:{path.name}")
    manifest = manifest or {}
    hr = 1 if "real header in row2" in manifest.get("domain", "") else None
    header, rows, idx, total, chunks = load_raw(path, rng, hr)
    sems = manifest.get("types") or []
    if sems and len(sems) != len(header):
        sems = []  # manifest does not describe this sheet layout; infer instead
    if not header or len(rows) < 5:
        return None
    width = len(header)
    parsed = [[parse_scalar(r[c] if c < len(r) else None) for c in range(width)] for r in rows]
    keep, cols = [], []
    for c in range(width):
        name = header[c] or f"col{c + 1}"
        kinds = [p[c] for p in parsed]
        filled = sum(1 for k, _ in kinds if k != "empty")
        if filled < 0.5 * len(kinds):
            continue
        if name.upper() == "WKT" or any(isinstance(v, str) and v.startswith(("POINT", "POLYGON", "MULTI")) for _, v in kinds[:5]):
            continue
        sem = sems[c] if sems else None
        st = manifest_stype(sem, name, kinds) if sem else infer_type(name, kinds)
        keep.append(c)
        cols.append(Column(name, st, sem))
        if len(keep) >= MAX_COLS:
            break
    if len(keep) < 2:
        return None
    typed = [[coerce(cols[j].stype, *p[c]) for j, c in enumerate(keep)] for p in parsed]
    rel = str(path.relative_to(root))
    title = re.sub(r"[_\-]+", " ", path.stem).strip().title()
    return Source(rel, title, cols, typed, idx, total, chunks or [(0, len(typed))])


# ---------------------------------------------------------------- JSON cache


def _enc(v):
    if isinstance(v, dt.datetime):
        return {"dt": v.isoformat()}
    if isinstance(v, dt.date):
        return {"d": v.isoformat()}
    return v


def _dec(v):
    if isinstance(v, dict):
        if "dt" in v:
            return dt.datetime.fromisoformat(v["dt"])
        if "d" in v:
            return dt.date.fromisoformat(v["d"])
    return v


def save_source(src: Source, path: Path):
    path.write_text(json.dumps({
        "name": src.name, "title": src.title, "n_rows_total": src.n_rows_total,
        "columns": [[c.name, c.stype, c.semantic] for c in src.columns],
        "row_idx": src.row_idx, "chunks": src.chunks,
        "rows": [[_enc(v) for v in r] for r in src.rows],
    }, ensure_ascii=False))


def load_source(path: Path) -> Source:
    d = json.loads(path.read_text())
    return Source(d["name"], d["title"], [Column(*c) for c in d["columns"]],
                  [[_dec(v) for v in r] for r in d["rows"]], d["row_idx"], d["n_rows_total"],
                  [tuple(c) for c in d["chunks"]])


@lru_cache(maxsize=12)
def cached_source(path: str) -> Source:
    return load_source(Path(path))


def read_manifest(root: Path) -> dict[str, dict]:
    """MANIFEST.csv → {relative path: {"types": [semantic type per column, in order], "domain": str}}."""
    m = root / "MANIFEST.csv"
    out: dict[str, dict] = {}
    if not m.exists():
        return out
    with open(m, newline="", encoding="utf-8-sig") as f:
        for rec in csv.DictReader(f):
            p = (rec.get("path") or "").strip()
            raw = (rec.get("column_semantic_types") or "").strip()
            if p:
                out[p] = {"types": [t.strip() for t in raw.split(";")] if raw else [], "domain": rec.get("domain") or ""}
    return out


def build_cache(clean: Path, out: Path, log=print) -> list[Path]:
    out.mkdir(parents=True, exist_ok=True)
    manifest = read_manifest(clean)
    done = []
    files = sorted(p for p in clean.rglob("*") if p.suffix.lower() in (".csv", ".xlsx", ".xls", ".ods") and p.name != "MANIFEST.csv")
    for p in files:
        target = out / (re.sub(r"[^A-Za-z0-9]+", "_", str(p.relative_to(clean))) + ".json")
        if target.exists():
            done.append(target)
            continue
        try:
            src = build_source(p, clean, manifest.get(str(p.relative_to(clean))))
        except Exception as e:  # untrusted input: a bad file is skipped, not fatal
            log(f"skip {p}: {type(e).__name__}: {e}")
            continue
        if src is None:
            log(f"skip {p}: no usable table")
            continue
        save_source(src, target)
        log(f"{src.name}: {len(src.rows)} rows cached of {src.n_rows_total}, {len(src.columns)} cols: "
            + ", ".join(f"{c.name}:{c.stype}/{c.semantic}" for c in src.columns[:12]))
        done.append(target)
    return done
