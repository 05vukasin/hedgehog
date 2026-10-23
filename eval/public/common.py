"""Shared pieces of the public benchmark: paths, the bubblewrap sandbox, value/label matching, tokens.

Everything here is deterministic and model-free. Matching rules (used by every metric):

- text: Unicode NFKC, casefold, accents removed, every run of non-alphanumerics -> one space,
  stripped (`norm_text`). "French-language workers" == "french language workers", "Côte" == "cote".
- numbers: `parse_number` reads 1,234.5 / 1.234,5 / (500) / 12% / 2.5e3 / '−' minus; two numbers
  match when they are equal after rounding to the decimals the answer shows (an answer "52.1" matches
  52.1 and 52.13; "2000" matches 2000.0), and a percent answer "52.1%" also matches the fraction 0.521.
"""

from __future__ import annotations

import importlib.util
import json
import math
import os
import re
import unicodedata
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
PROJECT = REPO.parent


def data_dir() -> Path:
    return Path(os.environ.get("HEDGEHOG_DATA") or PROJECT / "testdata") / "public"


def work_dir() -> Path:
    """Scratch for tool outputs (on disk, outside the repo and the data folder)."""
    return Path(os.environ.get("HH_PUBLIC_WORK") or PROJECT / "scratch-archive" / "public-bench")


def swap_dir() -> Path:
    return Path(os.environ.get("SWAP_DIR") or work_dir() / "swap")


def hedgehog_bin() -> str:
    return os.environ.get("HEDGEHOG_BIN") or str(REPO / "target-master" / "release" / "hedgehog")


def _swap_tools():
    spec = importlib.util.spec_from_file_location("swap_tools", REPO / "eval" / "agents" / "swap" / "tools.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)  # type: ignore[union-attr]
    return mod


def sandbox_cmd(workdir: Path, ro: list[Path], py: Path, mem_gb: float = 3, secs: int | None = None) -> list[str]:
    """bwrap prefix (eval/agents/swap/tools.py `sandbox`): no network, no home, empty env, `workdir` as /work,
    address space capped at `mem_gb` (prlimit)."""
    t = _swap_tools()
    return t.sandbox(workdir, ro + t.python_ro(py), mem=str(int(mem_gb * (1 << 30))), secs=secs)


def read_jsonl(p: Path) -> list[dict]:
    if not p.exists():
        return []
    return [json.loads(line) for line in p.read_text().splitlines() if line.strip()]


# ------------------------------------------------------------------------------------------------
# text and numbers

_NON_ALNUM = re.compile(r"[\W_]+", re.UNICODE)


def norm_text(s) -> str:
    if s is None:
        return ""
    s = unicodedata.normalize("NFKD", unicodedata.normalize("NFKC", str(s)).casefold())
    s = "".join(c for c in s if not unicodedata.combining(c))
    return _NON_ALNUM.sub(" ", unicodedata.normalize("NFC", s)).strip()


_NUM = re.compile(r"^[+\-−–]?\(?[+\-−–]?(\d{1,3}([,.   ']\d{3})+|\d+)([.,]\d+)?\)?(e[+\-]?\d+)?%?$", re.I)


def parse_number(s) -> tuple[float, int, bool] | None:
    """(value, decimals shown, is_percent) or None. Accepts US and European grouping."""
    if s is None or isinstance(s, bool):
        return None
    if isinstance(s, (int, float)):
        if isinstance(s, float) and not math.isfinite(s):
            return None
        return float(s), _decimals_of(repr(s)) if isinstance(s, float) else 0, False
    t = str(s).strip().replace("−", "-").replace("–", "-").replace("$", "").replace("€", "").replace("£", "")
    t = t.replace("¥", "").replace("元", "").strip()
    if not t or len(t) > 40 or not _NUM.match(t):
        return None
    pct = t.endswith("%")
    t = t.rstrip("%")
    neg = t.startswith("-") or (t.startswith("(") and t.endswith(")"))
    t = t.strip("()+-")
    exp = ""
    m = re.search(r"e[+\-]?\d+$", t, re.I)
    if m:
        exp, t = t[m.start():], t[:m.start()]
    t = t.replace(" ", "").replace(" ", "").replace(" ", "").replace("'", "")
    if "," in t and "." in t:
        dec = "," if t.rfind(",") > t.rfind(".") else "."
        t = t.replace("." if dec == "," else ",", "").replace(",", ".")
    elif "," in t:
        parts = t.split(",")
        # 1,234 / 12,345,678 are groups; 1,5 / 12,34 are decimals
        t = t.replace(",", "") if all(len(p) == 3 for p in parts[1:]) and len(parts) > 1 else t.replace(",", ".")
    elif t.count(".") > 1:
        t = t.replace(".", "")
    try:
        v = float(t + exp)
    except ValueError:
        return None
    return (-v if neg else v), _decimals_of(t), pct


_CLOCK = re.compile(r"^(\d{1,2}):([0-5]\d)(?::([0-5]\d)(?:\.\d+)?)?$")


def clock_fraction(s) -> float | None:
    """A clock time or duration written "H:MM" / "HH:MM:SS" (hedgehog writes time-of-day cells that way) as a
    fraction of a day, the value the source stores (09:30:00 -> 0.395833). Hours up to 47 (Excel's elapsed time
    shows 24:00:00 and above); anything else (ratios "1:2" with one-digit minutes, dates) is None."""
    if not isinstance(s, str):
        return None
    m = _CLOCK.match(s.strip())
    if not m or int(m.group(1)) > 47:
        return None
    return (int(m.group(1)) * 3600 + int(m.group(2)) * 60 + int(m.group(3) or 0)) / 86400.0


def typed_number(v) -> float | None:
    """A typed value as a number: a number, or a clock time as its day fraction (else None)."""
    p = parse_number(v)
    if p:
        return p[0]
    return clock_fraction(v)


def _decimals_of(t: str) -> int:
    if "e" in t.lower():
        return 6
    return len(t.split(".", 1)[1]) if "." in t else 0


def num_match(answer: tuple[float, int, bool], cell: float) -> bool:
    """Does `cell` (a stored number) equal the answer as shown (its decimals; percent as fraction too)?"""
    v, d, pct = answer
    for c, dd in [(v, d)] + ([(v / 100.0, d + 2)] if pct else []):
        if math.isclose(c, cell, rel_tol=1e-9, abs_tol=1e-12):
            return True
        if round(cell, min(dd, 12)) == round(c, min(dd, 12)) and (dd > 0 or abs(c) >= 1):
            return True
    return False


# ------------------------------------------------------------------------------------------------
# question words

STOP = set("""a an the of in on at to for from by with and or is are was were be been what which who whom whose
how many much when where why does do did that this these those it its as than then there their they them his her
our your my me we you he she i not no yes all any each per into over under between among about than more most less
least total value values table tables number numbers amount data list give show find tell please""".split())
_CJK = re.compile(r"[㐀-鿿豈-﫿]")


def words(s: str) -> set[str]:
    """Content words: Latin words of 3+ letters not in STOP, numbers, and CJK character bigrams."""
    n = norm_text(s)
    out: set[str] = set()
    for w in n.split():
        if _CJK.search(w):
            chars = [c for c in w if _CJK.match(c)]
            out.update(a + b for a, b in zip(chars, chars[1:]))
            if len(chars) == 1:
                out.add(chars[0])
            rest = _CJK.sub(" ", w).split()
            out.update(x for x in rest if len(x) >= 3 and x not in STOP)
        elif (len(w) >= 3 or w.isdigit()) and w not in STOP:
            out.add(w)
    return out


# ------------------------------------------------------------------------------------------------
# tokens: the repo's estimate (eval/info/score.py): tiktoken o200k_base when importable, else chars / 4

_ENC = None
TOKENIZER = "chars/4"
try:
    import tiktoken  # type: ignore

    _ENC = tiktoken.get_encoding("o200k_base")
    TOKENIZER = "tiktoken o200k_base"
except Exception:  # noqa: BLE001
    pass


def count_tokens(text: str) -> int:
    if not text:
        return 0
    if _ENC is None:
        return (len(text) + 3) // 4
    return len(_ENC.encode(text, disallowed_special=()))


# ------------------------------------------------------------------------------------------------
# A1 helpers


def col_letter(c: int) -> str:
    """0-based column -> letters."""
    s = ""
    c += 1
    while c:
        c, r = divmod(c - 1, 26)
        s = chr(65 + r) + s
    return s


def parse_a1(a1: str) -> tuple[int, int]:
    """'B9' -> (row 0-based, col 0-based)."""
    m = re.match(r"^\$?([A-Za-z]+)\$?(\d+)$", a1.strip())
    if not m:
        raise ValueError(a1)
    c = 0
    for ch in m.group(1).upper():
        c = c * 26 + ord(ch) - 64
    return int(m.group(2)) - 1, c - 1


# ------------------------------------------------------------------------------------------------
# answer atoms: the values a short or narrative answer names

_QUOTED = re.compile(r'"([^"\n]{1,80})"|“([^”\n]{1,80})”|「([^」\n]{1,80})」|『([^』\n]{1,80})』|《([^》\n]{1,80})》|‘([^’\n]{1,80})’')
_NUMBER = re.compile(r"(?<![A-Za-z0-9_.])[-−]?\d{1,3}(?:,\d{3})+(?:\.\d+)?%?(?![A-Za-z0-9])"
                     r"|(?<![A-Za-z0-9_.,])[-−]?\d+(?:\.\d+)?%?(?![A-Za-z0-9])")


def atoms_from_narrative(answer: str, question: str) -> list[dict]:
    """Quoted phrases and numbers of a free-text answer that the question does not already contain."""
    qnums = [p for p in (parse_number(m.group(0)) for m in _NUMBER.finditer(question)) if p]
    qtext = norm_text(question)
    out, seen = [], set()
    for m in _QUOTED.finditer(answer):
        t = next(g for g in m.groups() if g).strip().rstrip(".,;:!?。，；：！？").strip()
        n = norm_text(t)
        if n and n not in seen and n not in qtext:
            seen.add(n)
            out.append({"text": t, "kind": "number" if parse_number(t) else "text"})
    for m in _NUMBER.finditer(answer):
        t = m.group(0)
        p = parse_number(t)
        if not p or any(abs(p[0] - q[0]) < 1e-9 for q in qnums) or t in seen:
            continue
        seen.add(t)
        out.append({"text": t, "kind": "number"})
    return out


def atoms_from_short(answer: str) -> list[dict]:
    """A short answer ('1955, 62170', 'Germany', 'A; B') split into its values."""
    parts = [p.strip().strip(".").strip() for p in re.split(r"\s*;\s*|,\s+|、|\n", answer) if p.strip()]
    return [{"text": p, "kind": "number" if parse_number(p) else "text"} for p in parts if p]
