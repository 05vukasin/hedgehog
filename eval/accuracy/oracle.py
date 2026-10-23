"""C: computation reachability. Does a SMALL query over hedgehog's normalized tables give the gold number?

    python3 -I eval/accuracy/oracle.py [--datasets hitab,mimotable,realhitbench] [--show N] [--holdout]
    python3 -I eval/accuracy/oracle.py --validate        # HiTab operand agreement + decoy test + spot sample

No model anywhere. Input: the normalized tables of `hedgehog normalize --csv --keep-totals` (acc.load) and
each item's gold atoms of class C (acc.py). For every C atom the oracle enumerates a small program space
and tests whether one program's result equals the gold at the gold's shown precision.

Program space (per item, over all tables of the item's files)
-------------------------------------------------------------
Filters (row sets), all grounded in the question text:
  - entity equality: a value of a label column (text column, group column, `<label>_parent` outline
    column) whose normalised text occurs in the normalised question at word boundaries (simple plural
    folding), or, for a value of 2+ content words, whose content words all occur in the question;
  - years (1900-2100) named in the question: equality on any column holding that year, the year of a
    date column; month names (English, "N月"): the month of a date column;
  - conjunctions of two such filters on different columns; IN-lists of the filters of one column;
  - thresholds (> >= < <=) with a number of the question on a question-grounded numeric column (counts only).
Aggregates over a row set (data rows only: `_row_type` = data; total / subtotal / of-which rows are used
only by single-row lookups, which may pick any row a filter names):
  value lookup (one row), sum, avg, min, max, range (max-min), count (non-empty), count of rows, count of
  distinct labels; for one looked-up row: sum / avg / min / max across its question-grounded numeric
  columns and across all its numeric columns; rank of the value in its column (desc and asc).
Binary programs over pairs of grounded values (lookups and aggregates): a-b, a+b, a/b, 100 a/b,
(a-b)/b, 100 (a-b)/b; date lookups: difference in days.
Columns: every numeric column (hedgehog's dtype integer / number / percent / currency); a column is
question-grounded when its header shares a content word with the question (eval/public/common.words).

Grounding guard (against coincidental matches): a unary program is *grounded* when its row set is
filtered by a question entity or its column is question-grounded; a binary program when both operands
are. An atom is reachable only through a grounded witness; when only ungrounded programs match, the
item is reported as "ungrounded only" (counted as a miss). Each atom reports how many witnesses exist.

Matching (agent_mimotable's matcher, at the gold's shown precision): absolute values (a narrative
writes "a decrease of 41.67 %"); |x - gold| <= half a unit of the gold's last shown digit (plus 1e-9
relative), or the gold is x truncated to its decimals (when it shows decimals); a percent gold also
matches its fraction; a scale word after a narrative number (million, ten thousand, ...) applies;
a non-percent gold also matches a fraction when the question asks for a percent / rate / share.

Miss causes (one signature per missed atom, first that applies):
  operand cells not in any table (HiTab gold operands) / operand column text (reachable when numbers
  kept as text are parsed) / total row in data (reachable when total-labelled data rows are dropped) /
  only ungrounded programs match / value in no table (gold equals no cell and no program over any table) /
  outside program space.

Validation (--validate): (1) HiTab computed items: among those whose gold is reproduced from the gold
operand cells by a formula of the program space, the share where the oracle's witnesses include one
using exactly those operand cells (agreement); (2) a decoy test: gold x 1.1 and x 0.9 at the same
precision - the rate at which a decoy is "reachable" estimates the false-reachable rate; (3) a seeded
sample of MiMoTable witnesses for a hand check (precision). Holdout items are never printed without
--holdout.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
from collections import Counter, defaultdict
from functools import lru_cache
from itertools import combinations
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
import acc  # noqa: E402
from acc import PS, norm_text, parse_number  # noqa: E402
from common import _NUMBER, words  # noqa: E402

NUMERIC = {"integer", "number", "percent", "currency", "float"}
MAX_FILTERS = 60
MAX_PAIRS = 300
MAX_LOOKUP_ROWS = 80
MAX_PAIR_TOTAL = 400_000
PCT_Q = re.compile(r"percent|percentage|\bpct\b|%|\brate\b|\bratio\b|\bshare\b|\bproportion\b|百分|比例|占比|比率")
COUNT_Q = re.compile(r"\bhow many\b|\bnumber of\b|\bcount\b|多少个|几个|多少人|多少次")
MONTHS = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october",
          "november", "december"]
TOTAL_LABEL = PS.TOTAL_RX


@lru_cache(maxsize=200_000)
def _n(x: str) -> str:
    return norm_text(x)


@lru_cache(maxsize=200_000)
def _is_total(desc: str) -> bool:
    return bool(TOTAL_LABEL.search(_n(desc)))
# The operation a program performs must be named by the question (a coincidence guard; generous lexicons).
_OPS = {
    "sum": r"total|sum|combined|altogether|in all|overall|add|together|aggregate|accumulat|cumulative|whole|entire|合计|总|共",
    "avg": r"average|mean|\bper\b|\bavg\b|typical|平均|均",
    "max": r"highest|maximum|\bmax|most|largest|biggest|greatest|\btop\b|peak|best|longest|最高|最大|最多|最长",
    "min": r"lowest|minimum|\bmin|least|smallest|fewest|bottom|worst|shortest|最低|最小|最少|最短",
    "range": r"range|spread|difference between the (highest|largest|maximum|max)|gap between|极差|差距",
    "count": r"how many|number of|\bcount|多少|几",
    "rank": r"\brank|position|\bplace\b|排名|第几",
    "-": r"differ|more|less|fewer|higher|lower|greater|smaller|exceed|gap|minus|subtract|increase|decrease|change|"
         r"compar|than|between|grow|drop|rise|rose|fell|fall|gain|loss|surplus|deficit|remain|left|balance|"
         r"declin|reduc|差|多|少|增|减|变化|比|剩",
    "+": r"total|sum|combined|together|\band\b|plus|altogether|both|overall|合计|和|加|共",
    "/": r"ratio|times|percent|proportion|share|fraction|rate|\bper\b|%|out of|divid|account|比例|占|倍|率|百分",
    "growth": r"grow|increase|decrease|change|rate|percent|%|rose|rise|fell|fall|drop|declin|growth|增长|下降|变化|增幅|降幅",
}
_OPS = {k: re.compile(v, re.I) for k, v in _OPS.items()}
_OP_OF = {"sum": "sum", "avg": "avg", "max": "max", "min": "min", "range": "range", "count": "count",
          "count rows": "count", "count distinct": "count", "rank desc": "rank", "rank asc": "rank",
          "row-sum": "sum", "row-avg": "avg", "row-max": "max", "row-min": "min",
          "-": "-", "+": "+", "/": "/", "100*/": "/", "(a-b)/b": "growth", "100*(a-b)/b": "growth"}


def op_named(name: str, q: str) -> bool:
    k = _OP_OF.get(name)
    return True if k is None else bool(_OPS[k].search(q))


# ------------------------------------------------------------------------------------------------
# gold matching


def gold_targets(atom: dict, question: str) -> list[tuple[float, float, bool]]:
    """[(target abs value, unit of last shown digit, truncation allowed)]."""
    p = parse_number(atom["text"])
    if p is None:
        return []
    v, d, pct = p
    d = min(d, 12)
    u = 10.0 ** (-d)
    out = [(abs(v), u, d > 0)]
    if pct or PCT_Q.search(question.lower()):
        out.append((abs(v) / 100.0, u / 100.0, d > 0))
    s = atom.get("scale") or 1.0
    if s != 1.0:
        out.append((abs(v) * s, u * s, d > 0))
    return out


def match_mask(vals: np.ndarray, targets) -> np.ndarray:
    a = np.abs(vals)
    m = np.zeros(a.shape, dtype=bool)
    for t, u, trunc in targets:
        eps = 1e-9 * max(1.0, t)
        hit = np.abs(a - t) <= 0.5 * u + eps
        if trunc:
            hit |= (a >= t - eps) & (a < t + u - eps)
        m |= hit
    return m & np.isfinite(a)


def decoys(atom: dict) -> list[dict]:
    p = parse_number(atom["text"])
    if p is None:
        return []
    v, d, pct = p
    out = []
    for f, sgn in ((1.1, 1), (0.9, -1)):
        x = round(v * f, d)
        if x == v:
            x = v + sgn * 10.0 ** (-d)
        out.append({**atom, "text": (f"{x:.{d}f}" if d else str(int(round(x)))) + ("%" if pct else "")})
    return out


# ------------------------------------------------------------------------------------------------
# question parsing


def stem(w: str) -> str:
    if len(w) > 4 and w.endswith("ies"):
        return w[:-3] + "y"
    if len(w) > 3 and w.endswith("s") and not w.endswith("ss"):
        return w[:-1]
    return w


CJK = re.compile(r"[㐀-鿿]")
_STOP = set("""a an the of in on at to for from by with and or is are was were be been what which who whom whose how
when where why does do did that this these those it its as than then there their they them his her our your my me
we you he she i not no yes all any each per into over under between among about please give show find tell list
table tables data""".split())


_MONTH_ABBR = {m: m[:3] for m in MONTHS}
_MONTH_ABBR.update({m[:3]: m[:3] for m in MONTHS})
_MONTH_ABBR["sept"] = "sep"
_MONTH_ABBR.pop("may", None)  # the verb


_UNIT_WORDS = {"number", "percent", "percentage", "pct", "rate", "dollar", "thousand", "million", "billion", "unit",
               "yuan", "usd", "eur", "count", "value", "amount", "share", "proportion", "per", "cent"}


def cwords(s: str) -> set[str]:
    """Content words for column grounding: like common.words, but keeping 'amount', 'number', 'value',
    'total' (column names use them) and folding plurals."""
    out = set()
    for w in norm_text(s).split():
        if CJK.search(w):
            chars = [c for c in w if CJK.match(c)]
            out.update(a + b for a, b in zip(chars, chars[1:]))
            if len(chars) == 1:
                out.add(chars[0])
            out.update(stem(x) for x in CJK.sub(" ", w).split() if len(x) >= 2 and x not in _STOP)
        elif w in _MONTH_ABBR:
            out.add(_MONTH_ABBR[w])
        elif (len(w) >= 2 or w.isdigit()) and w not in _STOP:
            m = re.fullmatch(r"([a-z]{5,})\d", w)  # a footnote mark glued to a word: "threshold2"
            out.add(stem(m.group(1) if m else w))
    return out


def _wmatch(a: set[str], b: set[str]) -> set[str]:
    """Words of a found in b, a word also matching one with the same first 6 letters (agriculture ~
    agricultural, employ ~ employment are not: 6 letters both)."""
    out = a & b
    for w in a - out:
        if len(w) >= 6 and not w.isdigit() and any(len(x) >= 6 and x[:6] == w[:6] for x in b):
            out.add(w)
    return out


class Question:
    def __init__(self, q: str):
        self.text = q
        self.norm = norm_text(q)
        self.stemmed = " " + " ".join(stem(w) for w in self.norm.split()) + " "
        self.words = {stem(w) for w in words(q)}
        self.cwords = cwords(q)
        self.nums = []
        for m in _NUMBER.finditer(q):
            p = parse_number(m.group(0))
            if p:
                self.nums.append(p[0])
        self.years = {int(x) for x in self.nums if x == int(x) and 1900 <= x <= 2100}
        self.months = {i + 1 for i, mn in enumerate(MONTHS) if re.search(rf"\b{mn}\b|\b{mn[:3]}\b", self.norm)}
        self.months |= {int(x) for x in re.findall(r"(\d{1,2})\s*月", q) if 1 <= int(x) <= 12}
        self.total = acc.has_total_word(q)
        self.count = bool(COUNT_Q.search(q.lower()))
        self.thresholds = [x for x in self.nums if not (x == int(x) and 1900 <= x <= 2100)][:4]
        ys = sorted(self.years)
        rng = re.search(r"(?:from|between|during|over)\s+\D{0,25}?((?:19|20)\d\d)\D{1,25}?(?:to|and|through|until|till|[-–])\s+\D{0,25}?((?:19|20)\d\d)", q, re.I)
        self.year_range = (int(rng.group(1)), int(rng.group(2))) if rng else None
        if self.year_range and self.year_range[0] > self.year_range[1]:
            self.year_range = self.year_range[::-1]
        # numbers that may be row ids ("serial number 5", "No. 1", "the 8th project")
        self.ids = {int(x) for x in self.nums if x == int(x) and 0 < x < 100000 and not 1900 <= x <= 2100}
        self.ids |= {int(x) for x in re.findall(r"\b(\d{1,4})(?:st|nd|rd|th)\b", q)}
        tn = re.search(r"\b(?:top|first|last|bottom)\s+(\d{1,2}|two|three|four|five|ten)\b(?!\s*%|\s*percent)|\b(\d{1,2}|two|three|four|five|ten)\s+(?:\w+\s+){0,3}?(?:with the\s+)?(?:highest|largest|lowest|smallest|most|least|best|worst|biggest)\b", q, re.I)
        wn = {"two": 2, "three": 3, "four": 4, "five": 5, "ten": 10}
        self.top_n = None
        if tn:
            x = tn.group(1) or tn.group(2)
            self.top_n = int(x) if x.isdigit() else wn[x.lower()]
        self.hypo = [float(m.group(1)) for m in re.finditer(
            r"\b(?:increase[sd]?|decrease[sd]?|grow[sn]?|rise[sn]?|drop(?:s|ped)?|fall[sn]?|reduce[sd]?|raise[sd]?|cut|change[sd]?)"
            r"\s+by\s+(\d+(?:\.\d+)?)\s*(?:%|percent|percentage points?)?", q, re.I)]
        self.multipliers = [2.0] if re.search(r"\bdoubl", q, re.I) else []
        self.multipliers += [3.0] if re.search(r"\btripl", q, re.I) else []
        self.multipliers += [0.5] if re.search(r"\bhalv", q, re.I) else []

    def entity(self, value) -> int:
        """0 no, 2 phrase at word boundaries, 1 all content words (2+)."""
        n = norm_text(value)
        if not n:
            return 0
        if CJK.search(n):
            return 2 if len(n.replace(" ", "")) >= 2 and n in self.norm else 0
        if len(n) < 2 or not re.search(r"[a-z]", n):
            return 0
        if not words(value) and not TOTAL_LABEL.search(n):
            return 0  # 'value', 'other', 'number': no entity
        ns = " ".join(stem(w) for w in n.split())
        if f" {ns} " in self.stemmed:
            return 2
        ws = {stem(w) for w in words(value)}
        if len(ws) >= 2 and _wmatch(ws, self.words) == ws:
            return 1
        if len(ws) >= 2 and len(_wmatch(ws, self.words)) >= max(2, 0.6 * len(ws)):
            return 1  # most content words: "poor or fair mental health" ~ "Negative (fair, poor)"
        return 0

    def col_grounded(self, header: str) -> int:
        """2: every content word of one header segment is in the question; 1: some word; 0: none."""
        hw = cwords(header)
        if not _wmatch(hw, self.cwords):
            return 0
        strip = lambda x: re.sub(r"\([^)]*\)|（[^）]*）|\[[^]]*\]", " ", x)  # units in brackets do not count
        segs = [x for x in header.split(" / ") if cwords(strip(x)) - _UNIT_WORDS] or header.split(" / ")
        leaf = cwords(strip(segs[-1])) - _UNIT_WORDS
        whole = cwords(strip(header)) - _UNIT_WORDS
        if (leaf and _wmatch(leaf, self.cwords) == leaf) or (whole and _wmatch(whole, self.cwords) == whole) or \
                (len(_wmatch(whole, self.cwords)) >= 2 and 3 * len(_wmatch(whole, self.cwords)) >= 2 * len(whole)):
            return 2
        return 1


# ------------------------------------------------------------------------------------------------
# per-table program enumeration


def _f(v) -> float:
    if isinstance(v, bool) or v is None or v == "":
        return math.nan
    if isinstance(v, (int, float)):
        return float(v)
    return math.nan


def _f_relaxed(v) -> float:
    x = _f(v)
    if math.isnan(x) and isinstance(v, str):
        p = parse_number(v)
        if p:
            return p[0] / 100.0 if p[2] else p[0]
    return x


def _date(v):
    if isinstance(v, str):
        m = re.match(r"^(\d{4})-(\d{2})-(\d{2})", v)
        if m:
            return int(m.group(1)), int(m.group(2)), int(m.group(3))
    return None


class TableProg:
    """Values of all programs over one table: arrays + lazily described metadata."""

    def __init__(self, t, q: Question, relaxed_text=False, drop_total_labels=False):
        self.t = t
        n = len(t.rows)
        ncol = len(t.headers)
        dtypes = getattr(t, "dtypes", ["?"] * ncol)
        conv = _f_relaxed if relaxed_text else _f
        self.numcols = [c for c in range(ncol) if dtypes[c] in NUMERIC or (relaxed_text and dtypes[c] in ("text", "string"))]
        self.X = np.array([[conv(r[c]) if c < len(r) else math.nan for c in self.numcols] for r in t.rows], dtype=float).reshape(n, len(self.numcols))
        rt = t.row_types
        self.data = np.array([x == "data" for x in rt], dtype=bool) if rt else np.ones(n, dtype=bool)
        own = sorted(t.label_cols - t.parent_cols)
        if drop_total_labels:
            lab = [" ".join(norm_text(r[k]) for k in own if k < len(r) and r[k] not in (None, "")) for r in t.rows]
            self.data &= np.array([not TOTAL_LABEL.search(x) for x in lab], dtype=bool)
        self.colg = np.array([q.col_grounded(t.headers[c]) for c in self.numcols], dtype=int)
        self.filters = self._filters(q, dtypes)
        self.vals, self.grd, self.meta = [], [], []
        self.dates = []  # (date tuple, desc, operand)
        self._enumerate(q, dtypes)

    # filters --------------------------------------------------------------------------------
    def _filters(self, q: Question, dtypes) -> list[tuple]:
        t = self.t
        out = []
        labelish = sorted(t.label_cols | t.parent_cols | {c for c, d in enumerate(dtypes) if d in ("text", "string")})
        for c in labelish:
            seen = {}
            for i, r in enumerate(t.rows):
                v = r[c] if c < len(r) else None
                if v in (None, "") or isinstance(v, (int, float)) and not isinstance(v, bool) and not (1900 <= v <= 2100):
                    continue
                key = str(v)
                if key not in seen:
                    seen[key] = q.entity(key) if not isinstance(v, (int, float)) else (2 if int(v) in q.years and v == int(v) else 0)
                if seen[key]:
                    out.append((c, key, seen[key]))
        # equality on years in any column (integer year columns), dates by year / month
        for c, d in enumerate(dtypes):
            if d in ("date", "datetime"):
                for i, r in enumerate(t.rows):
                    dt = _date(r[c] if c < len(r) else None)
                    if not dt:
                        continue
                    if dt[0] in q.years:
                        out.append((c, f"year={dt[0]}", 2))
                    if dt[1] in q.months:
                        out.append((c, f"month={dt[1]}", 2))
                        if dt[0] in q.years:
                            out.append((c, f"year-month={dt[0]}-{dt[1]:02d}", 3))
            elif d in NUMERIC and q.years and c not in labelish:
                for r in t.rows:
                    v = r[c] if c < len(r) else None
                    if isinstance(v, (int, float)) and not isinstance(v, bool) and v == int(v) and int(v) in q.years:
                        out.append((c, str(int(v)), 2))
        # year tokens inside labels ("2015 Q1"), a year range, row ids
        for c in labelish:
            for r in t.rows:
                v = r[c] if c < len(r) else None
                if isinstance(v, str):
                    ys = {int(y) for y in re.findall(r"(?<!\d)((?:19|20)\d\d)(?!\d)", v)}
                    for y in ys & q.years:
                        out.append((c, f"has-year={y}", 1))
                    if q.year_range and any(q.year_range[0] <= y <= q.year_range[1] for y in ys):
                        out.append((c, f"year-in={q.year_range[0]}..{q.year_range[1]}", 2))
        if q.year_range:
            for c, d in enumerate(dtypes):
                for r in t.rows:
                    v = r[c] if c < len(r) else None
                    y = _date(v)[0] if d in ("date", "datetime") and _date(v) else (int(v) if isinstance(v, (int, float)) and not isinstance(v, bool) and v == int(v) and 1900 <= v <= 2100 else None)
                    if y and q.year_range[0] <= y <= q.year_range[1]:
                        out.append((c, f"year-in={q.year_range[0]}..{q.year_range[1]}", 2))
                        break
        for c, d in enumerate(dtypes):
            if d == "integer" and q.ids and q.col_grounded(t.headers[c]) >= 1:
                for r in t.rows:
                    v = r[c] if c < len(r) else None
                    if isinstance(v, (int, float)) and not isinstance(v, bool) and v == int(v) and int(v) in q.ids:
                        out.append((c, f"id={int(v)}", 2))
        # group to masks
        masks: dict[tuple, np.ndarray] = {}
        strength: dict[tuple, int] = {}
        n = len(t.rows)
        for c, key, s in out:
            k = (c, key)
            if k in masks:
                continue
            m = np.zeros(n, dtype=bool)
            for i, r in enumerate(t.rows):
                v = r[c] if c < len(r) else None
                if key.startswith(("has-year=", "year-in=", "id=")):
                    kind, val = key.split("=", 1)
                    if kind == "id":
                        m[i] = isinstance(v, (int, float)) and not isinstance(v, bool) and v == int(val)
                        continue
                    if isinstance(v, str):
                        ys = [int(y) for y in re.findall(r"(?<!\d)((?:19|20)\d\d)(?!\d)", v)]
                    elif _date(v):
                        ys = [_date(v)[0]]
                    elif isinstance(v, (int, float)) and not isinstance(v, bool) and v == int(v):
                        ys = [int(v)]
                    else:
                        ys = []
                    if kind == "has-year":
                        m[i] = int(val) in ys
                    else:
                        lo, hi = (int(x) for x in val.split(".."))
                        m[i] = any(lo <= y <= hi for y in ys)
                    continue
                if key.startswith(("year=", "month=", "year-month=")):
                    dt = _date(v)
                    if not dt:
                        continue
                    kind, val = key.split("=")
                    m[i] = (kind == "year" and dt[0] == int(val)) or (kind == "month" and dt[1] == int(val)) or \
                        (kind == "year-month" and f"{dt[0]}-{dt[1]:02d}" == val)
                elif isinstance(v, (int, float)) and not isinstance(v, bool):
                    m[i] = str(int(v)) == key if v == int(v) else False
                else:
                    m[i] = v is not None and str(v) == key
            if m.any():
                masks[k] = m
                strength[k] = s
        ks = sorted(masks, key=lambda k: (-strength[k], -len(k[1])))[:MAX_FILTERS]
        res = [(f"{self.t.headers[c]}={key!r}", masks[(c, key)], c) for c, key in ks]
        for c, key in ks:  # "outside Quebec", "excluding X", "other than X": the complement
            n = _n(key)
            if n and re.search(rf"\b(outside|except|excluding|other than|apart from|besides)\s+(the\s+)?{re.escape(n)}\b", q.norm):
                res.append((f"{self.t.headers[c]}!={key!r}", ~masks[(c, key)], c))
        return res

    # enumeration ----------------------------------------------------------------------------
    def _add(self, v, g, meta):
        self.vals.append(v)
        self.grd.append(g)
        self.meta.append(meta)

    def _aggs(self, desc, mask, rowg, ops=True):
        if not len(self.numcols):
            return
        sub = self.X[mask]
        if sub.shape[0] == 0:
            return
        cnt = np.sum(~np.isnan(sub), axis=0)
        with np.errstate(all="ignore"):
            s = np.nansum(sub, axis=0)
            mx = np.nanmax(np.where(np.isnan(sub), -np.inf, sub), axis=0)
            mn = np.nanmin(np.where(np.isnan(sub), np.inf, sub), axis=0)
        wide = len(self.numcols) > 40
        for j in range(len(self.numcols)):
            if cnt[j] == 0 or (wide and rowg and not self.colg[j]):
                continue  # wide tables: filtered aggregates only on question-grounded columns
            g = (rowg, int(self.colg[j]))  # colg 0 / 1 / 2
            if cnt[j] == 1 and sub.shape[0] == 1:
                self._add(s[j], g, ("value", desc, j, mask))
                continue
            for name, val in (("sum", s[j]), ("avg", s[j] / cnt[j]), ("max", mx[j]), ("min", mn[j]),
                              ("range", mx[j] - mn[j]), ("count", float(cnt[j]))):
                self._add(val, g, (name, desc, j, mask))

    def _enumerate(self, q: Question, dtypes):
        t = self.t
        D = self.data
        self._aggs("all data rows", D, 0)
        self._add(float(D.sum()), (0, int(q.count)), ("count rows", "all data rows", None, D))
        fl = self.filters
        sets = [(d, m, 1) for d, m, c in fl]
        pairs = 0
        for (d1, m1, c1), (d2, m2, c2) in combinations(fl, 2):
            if c1 == c2 or pairs >= MAX_PAIRS:
                continue
            m = m1 & m2
            if m.any():
                sets.append((f"{d1} and {d2}", m, 2))
                pairs += 1
        bycol = defaultdict(list)
        for d, m, c in fl:
            bycol[c].append((d, m))
        for c, lst in bycol.items():
            if 2 <= len(lst) <= 12:
                m = np.zeros(len(D), dtype=bool)
                for _, mm in lst:
                    m |= mm
                sets.append((" or ".join(d for d, _ in lst), m, 1))
        for desc, m, rg in sets:
            md = m & D
            if md.sum() >= 2:
                self._aggs(desc, md, rg)
                self._add(float(md.sum()), (rg, 1), ("count rows", desc, None, md))
        # top / bottom N of a grounded column
        if q.top_n:
            for desc, m, rg in [("all data rows", D, 0)] + [(d, mm & D, r) for d, mm, r in sets]:
                for j in np.nonzero(self.colg >= 1)[0]:
                    col = self.X[m, j]
                    col = np.sort(col[~np.isnan(col)])
                    if len(col) >= q.top_n:
                        for side, part in (("top", col[-q.top_n:]), ("bottom", col[:q.top_n])):
                            sub = np.zeros(len(D), dtype=bool)
                            for name, val in (("sum", part.sum()), ("avg", part.mean())):
                                self._add(float(val), (max(rg, 1), int(self.colg[j])), (name, f"{side} {q.top_n} of {desc}", int(j), m))
        # thresholds (counts) on grounded columns
        for x in q.thresholds:
            for j in np.nonzero(self.colg >= 1)[0]:
                col = self.X[:, j]
                for op, mm in ((">", col > x), (">=", col >= x), ("<", col < x), ("<=", col <= x)):
                    mm = mm & D
                    self._add(float(mm.sum()), (1, 1), ("count rows", f"{t.headers[self.numcols[j]]} {op} {x:g}", None, mm))
        # distinct labels
        for c in sorted(t.label_cols):
            hg = q.col_grounded(t.headers[c])
            for desc, m, rg in [("all data rows", D, 0)] + [(d, mm & D, r) for d, mm, r in sets]:
                if f"{t.headers[c]}=" in desc or f"{t.headers[c]}!=" in desc:
                    continue  # distinct values of the filtered column itself: 1 by construction
                vals = {str(r[c]) for r, k in zip(t.rows, m) if k and c < len(r) and r[c] not in (None, "")}
                if vals:
                    self._add(float(len(vals)), (rg, int(hg)), ("count distinct", desc, ("label", c), m))
        # single-row lookups (any row type) for rows a filter names
        rows = np.zeros(len(D), dtype=bool)
        for _, m, _ in fl:
            rows |= m
        idx = np.nonzero(rows)[0][:MAX_LOOKUP_ROWS]
        nf = np.array([sum(bool(m[i]) for _, m, _ in fl) for i in idx], dtype=int) if len(idx) else []
        for i, k in zip(idx, nf):
            one = np.zeros(len(D), dtype=bool)
            one[i] = True
            lab = " / ".join(t.labels(i)[:3]) or f"row {i}"
            desc = f"row [{lab}]"
            rg = 2 if k >= 2 else 1
            self._aggs(desc, one, rg)
            row = self.X[i]
            ok = ~np.isnan(row)
            sets_c = [("grounded columns", (self.colg >= 1) & ok), ("all numeric columns", ok)]
            if q.year_range:
                yc = np.array([any(q.year_range[0] <= int(y) <= q.year_range[1] for y in re.findall(r"(?<!\d)((?:19|20)\d\d)(?!\d)", t.headers[c]))
                               for c in self.numcols], dtype=bool)
                sets_c.append(("columns of the year range", yc & ok))
            for name_set, sel in sets_c:
                if sel.sum() >= 2:
                    v = row[sel]
                    for name, val in (("sum", v.sum()), ("avg", v.mean()), ("max", v.max()), ("min", v.min())):
                        self._add(val, (rg, 0 if name_set == "all numeric columns" else 2), (f"row-{name}", f"{desc} across {name_set}", ("cols", tuple(np.nonzero(sel)[0])), one))
            for j in np.nonzero((self.colg >= 1) & ok)[0]:
                col = self.X[D, j]
                col = col[~np.isnan(col)]
                v = row[j]
                self._add(float(1 + np.sum(col > v)), (rg, 1), ("rank desc", desc, j, one))
                self._add(float(1 + np.sum(col < v)), (rg, 1), ("rank asc", desc, j, one))
            for c, d in enumerate(dtypes):
                if d in ("date", "datetime"):
                    dt = _date(t.rows[i][c] if c < len(t.rows[i]) else None)
                    if dt:
                        self.dates.append((dt, f"{desc}.{t.headers[c]}", (t.id, int(i), c)))

    # description / operands -------------------------------------------------------------------
    def describe(self, k: int) -> str:
        name, desc, j, _ = self.meta[k]
        t = self.t
        if j is None:
            col = ""
        elif isinstance(j, tuple):
            col = f"({t.headers[j[1]]})" if j[0] == "label" else ""
        else:
            col = f"({t.headers[self.numcols[j]]})"
        return f"{Path(t.file).name}:{t.id} {name}{col} where {desc}"

    def operands(self, k: int) -> frozenset:
        name, desc, j, mask = self.meta[k]
        t = self.t
        if j is None or (isinstance(j, tuple) and j[0] == "label"):
            return frozenset()
        rows = np.nonzero(mask)[0]
        if isinstance(j, tuple):
            return frozenset((t.id, int(i), self.numcols[c]) for i in rows for c in j[1])
        if name.startswith("rank"):
            return frozenset((t.id, int(i), self.numcols[j]) for i in rows)
        return frozenset((t.id, int(i), self.numcols[j]) for i in rows if not math.isnan(self.X[i, j]))


# ------------------------------------------------------------------------------------------------
# item level


class Programs:
    def __init__(self, tables, q: Question, **kw):
        self.tps = [TableProg(t, q, **kw) for t in tables if t.rows]
        vals, grd, ref = [], [], []
        for ti, tp in enumerate(self.tps):
            vals.extend(tp.vals)
            grd.extend(tp.grd)
            ref.extend((ti, k) for k in range(len(tp.vals)))
        self.vals = np.array(vals, dtype=float)
        g = np.array(grd, dtype=int).reshape(-1, 2)
        self.rowg = g[:, 0] if len(g) else np.zeros(0, int)
        self.colg = g[:, 1] if len(g) else np.zeros(0, int)
        self.strong = (self.rowg + np.minimum(self.colg, 1)) >= 1
        self.ref = ref
        qt = q.text
        names = [self.tps[ti].meta[k][0] for ti, k in ref]
        descs = [self.tps[ti].meta[k][1] for ti, k in ref]
        opg = np.array([op_named(n, qt) or (n == "sum" and " or " in d) for n, d in zip(names, descs)], dtype=bool)
        self.whole = np.array([d == "all data rows" or (n == "value" and _is_total(d))
                               for n, d in zip(names, descs)], dtype=bool)
        nonval = np.array([n != "value" for n in names], dtype=bool)
        # tight: the operation is named, and the row set and the column are named (an aggregate over all
        # data rows needs a fully named column; a count needs only its row set)
        iscount = np.array([_OP_OF.get(n) == "count" for n in names], dtype=bool)
        rowagg = np.array([n.startswith("row-") for n in names], dtype=bool)
        whole_count = np.array([n in ("count rows", "count distinct") and d == "all data rows" for n, d in zip(names, descs)], dtype=bool)
        self.tight = opg & (((self.rowg >= 1) & (self.colg >= 1)) | (self.whole & nonval & (self.colg == 2)) |
                            (iscount & (self.rowg >= 1)) | (whole_count & (self.colg >= 1)) |
                            (rowagg & (self.rowg >= 1)))
        self.operand_ok = (self.colg == 2) & ((self.rowg >= 1) | self.whole)
        self.kind = np.array(["vs" if n in ("value", "sum") else n for n in names], dtype=object)
        self.q = qt
        hyp = []
        for x in q.hypo:
            hyp += [(f"+{x:g} %", lambda v, x=x: v * (1 + x / 100)), (f"-{x:g} %", lambda v, x=x: v * (1 - x / 100)),
                    (f"+{x:g}", lambda v, x=x: v + x), (f"-{x:g}", lambda v, x=x: v - x)]
        for k in q.multipliers:
            hyp.append((f"x{k:g}", lambda v, k=k: v * k))
        self.hyp = hyp or None
        self.pairs = self._pairs()
        self.dates = [d for tp in self.tps for d in tp.dates][:200]

    def _pairs(self) -> tuple[np.ndarray, np.ndarray]:
        """Operand pairs of the binary programs, constrained to the shapes a question asks for:
        (A) one column, two row sets that are each named by the question (or "all data rows" / a total
        row as the whole): a difference between entities, an entity's share of the whole;
        (B) one row set, two question-grounded columns: a change between years / measures;
        (C) two tables with the same column header, row sets named by the question: across sheets / files.
        Operators pair like with like (value/sum with value/sum, avg with avg, ...)."""
        AGG = {"value", "sum", "avg", "max", "min"}
        by_col, by_row, by_hdr = defaultdict(list), defaultdict(list), defaultdict(list)
        for i, (ti, k) in enumerate(self.ref):
            name, desc, j, _ = self.tps[ti].meta[k]
            if name not in AGG or not isinstance(j, (int, np.integer)) or not np.isfinite(self.vals[i]):
                continue
            kind = "vs" if name in ("value", "sum") else name
            if kind not in ("vs", "avg"):
                continue  # differences / ratios of minima, maxima: not a question shape we accept
            whole = desc == "all data rows" or (name == "value" and _is_total(desc))
            if self.rowg[i] >= 1 or whole:
                by_col[(ti, int(j), kind)].append(i)
                by_hdr[(_n(self.tps[ti].t.headers[self.tps[ti].numcols[j]]), kind)].append(i)
            if self.colg[i]:
                by_row[(ti, desc, kind)].append(i)
        I, J = [], []
        total = 0

        def add(grp, cond):
            nonlocal total
            g = np.array(grp[:150], dtype=np.int64)
            if len(g) < 2 or total > MAX_PAIR_TOTAL:
                return
            a, b = np.meshgrid(g, g, indexing="ij")
            a, b = a.ravel(), b.ravel()
            keep = (a != b) & cond(a, b)
            I.append(a[keep])
            J.append(b[keep])
            total += int(keep.sum())

        rowg = self.rowg
        refs = np.array([r[0] for r in self.ref], dtype=np.int64)
        for grp in by_col.values():
            add(grp, lambda a, b: (rowg[a] >= 1) | (rowg[b] >= 1))
        colkey = np.array([self.tps[ti].numcols[self.tps[ti].meta[k][2]] if isinstance(self.tps[ti].meta[k][2], (int, np.integer)) else -1
                           for ti, k in self.ref], dtype=np.int64)

        def one_segment(a, b):
            out = np.zeros(len(a), dtype=bool)
            for n, (x, y) in enumerate(zip(a, b)):
                t = self.tps[self.ref[x][0]].t
                sx, sy = t.headers[colkey[x]].split(" / "), t.headers[colkey[y]].split(" / ")
                out[n] = len(sx) == len(sy) and sum(p != q for p, q in zip(sx, sy)) == 1
            return out
        for grp in by_row.values():
            add(grp, one_segment)
        for grp in by_hdr.values():
            add(grp, lambda a, b: refs[a] != refs[b])
        if not I:
            return np.zeros(0, int), np.zeros(0, int)
        I, J = np.concatenate(I), np.concatenate(J)
        if not len(I):
            return np.zeros(0, int), np.zeros(0, int)
        key = np.unique(I * (len(self.vals) + 1) + J)
        return key // (len(self.vals) + 1), key % (len(self.vals) + 1)

    def describe(self, i: int) -> str:
        ti, k = self.ref[i]
        return self.tps[ti].describe(k)

    def operands(self, i: int) -> frozenset:
        ti, k = self.ref[i]
        return self.tps[ti].operands(k)

    def pair_tight(self, op: str, I: np.ndarray, J: np.ndarray) -> np.ndarray:
        """Both operands named (row set and column), like kinds: '+' adds values / sums, ratios and growth
        take values / sums / averages, a difference any like pair."""
        ok = self.operand_ok[I] & self.operand_ok[J] & (self.vals[I] != self.vals[J])
        ok &= ~(self.whole[I] & self.whole[J])  # two whole-column aggregates: a row set must be named
        k = self.kind[I]
        if op != "-":
            ok &= k == "vs"
        return ok

    def binary(self):
        I, J = self.pairs
        A, B = self.vals[I], self.vals[J]
        with np.errstate(all="ignore"):
            yield "-", A - B
            yield "+", A + B
            yield "/", np.where(B != 0, A / B, np.nan)
            yield "100*/", np.where(B != 0, 100 * A / B, np.nan)
            yield "(a-b)/b", np.where(B != 0, (A - B) / B, np.nan)
            yield "100*(a-b)/b", np.where(B != 0, 100 * (A - B) / B, np.nan)

    def search(self, atom: dict, question: str, want_witnesses=5) -> dict:
        tg = gold_targets(atom, question)
        res = {"strong": 0, "weak": 0, "tight": 0, "witnesses": [], "operand_sets": []}
        if not tg or not len(self.vals):
            return res
        m = match_mask(self.vals, tg)
        hits = np.nonzero(m)[0]
        strong = [i for i in hits if self.strong[i]]
        res["strong"] += len(strong)
        res["weak"] += len(hits) - len(strong)
        res["tight"] += int(sum(bool(self.tight[i]) for i in strong))
        order = sorted(strong, key=lambda i: (-int(self.tight[i]), -(self.rowg[i] + self.colg[i]), i))
        for i in order[:want_witnesses]:
            res["witnesses"].append(self.describe(i))
        res["_unary"] = order
        if not strong and len(hits):
            res["weak_example"] = self.describe(hits[0])
        # hypotheticals: "if X increased by 10 %", "doubled" - on tight values only
        if self.hyp is not None and len(strong) == 0:
            tv = self.vals[self.tight]
            ti = np.nonzero(self.tight)[0]
            for name, f in self.hyp:
                hit = np.nonzero(match_mask(f(tv), tg))[0]
                if len(hit):
                    res["strong"] += len(hit)
                    res["tight"] += len(hit)
                    res["witnesses"].extend(f"{name} of [{self.describe(ti[h])}]" for h in hit[:want_witnesses])
                    res.setdefault("_unary", []).extend(int(ti[h]) for h in hit)
        res["_binary"] = []
        if len(self.pairs[0]):
            for op, M in self.binary():
                hit = np.nonzero(match_mask(M, tg))[0]
                res["strong"] += len(hit)
                if len(hit) and op_named(op, self.q):
                    I, J = self.pairs[0][hit], self.pairs[1][hit]
                    pt = self.pair_tight(op, I, J)
                    res["tight"] += int(np.sum(pt))
                    res.setdefault("_binary_tight", []).extend((op, int(a), int(b)) for a, b in zip(I[pt][:2000], J[pt][:2000]))
                for h in hit[:2000]:
                    res["_binary"].append((op, int(self.pairs[0][h]), int(self.pairs[1][h])))
        for op, a, b in (res.get("_binary_tight") or res["_binary"])[: max(0, want_witnesses - len(res["witnesses"]))]:
            res["witnesses"].append(f"[{self.describe(a)}] {op} [{self.describe(b)}]")
        # dates: difference in days
        if atom.get("kind") == "number" and self.dates:
            import datetime as _dt
            for (d1, s1, o1), (d2, s2, o2) in combinations(self.dates, 2):
                try:
                    days = abs((_dt.date(*d1) - _dt.date(*d2)).days)
                except ValueError:
                    continue
                if match_mask(np.array([float(days)]), tg)[0]:
                    res["strong"] += 1
                    res["tight"] += bool(re.search(r"day|duration|how long|天", self.q, re.I))
                    res["witnesses"].append(f"days between [{s1}] and [{s2}]")
                    res.setdefault("_dates", []).append((o1, o2))
        return res

    def witness_operand_sets(self, res: dict, limit=4000) -> list[frozenset]:
        """Operand cells of the tight witnesses."""
        out = []
        for i in res.get("_unary", [])[:limit]:
            if i < len(self.tight) and self.tight[i]:
                out.append(self.operands(i))
        for op, a, b in res.get("_binary_tight", [])[:limit]:
            out.append(self.operands(a) | self.operands(b))
        return out


# ------------------------------------------------------------------------------------------------


def tables_of(it: dict):
    return [t for f in it["files"] for t in acc.load(it["dataset"], f)[3]]


def c_atoms(it: dict) -> list[dict]:
    """C atoms of an item, with L number atoms that are no source cell moved to C."""
    out = [a for a in it["atoms"] if a["cls"] == "C" and a["kind"] == "number" and parse_number(a["text"])]
    if it["dataset"] != "hitab":
        raw = None
        for a in it["atoms"]:
            if a["cls"] == "L" and a["kind"] == "number" and parse_number(a["text"]):
                raw = raw if raw is not None else acc.raw_values(it["dataset"], it["files"])
                at = PS.Atom(a["text"], "number")
                if not any(at.matches_value(v) for v in raw):
                    out.append({**a, "cls": "C", "moved": "number not a source cell"})
    return out


def hitab_gold_ops(it: dict, tables) -> tuple[frozenset | None, str | None]:
    ops = set()
    for cell in it.get("cells") or []:
        loc = PS.locate(tables, it["sheet"], cell["row"], cell["col"])
        if isinstance(loc, str):
            return None, loc
        t, i, c = loc
        ops.add((t.id, i, c))
    return frozenset(ops), None


def replay_gold(it: dict, gold: dict) -> str | None:
    """Is the gold reproduced from HiTab's gold operand cells (source values) by a formula of the program
    space? Returns the formula or None."""
    vs = []
    for c in it.get("cells") or []:
        p = parse_number(c.get("raw") or c.get("text"))
        if p is None:
            return None
        vs.append(p[0] / 100 if p[2] else p[0])
    if not vs:
        return None
    tg = gold_targets(gold, it["question"])
    cands = {"sum": sum(vs), "avg": sum(vs) / len(vs), "max": max(vs), "min": min(vs), "range": max(vs) - min(vs),
             "count": float(len(vs))}
    if len(vs) == 1:
        cands["value"] = vs[0]
    for i, a in enumerate(vs):
        rest = sum(vs[:i] + vs[i + 1:])
        others = vs[:i] + vs[i + 1:]
        if len(vs) == 2:
            b = others[0]
            cands[f"diff{i}"] = a - b
            if b:
                cands[f"ratio{i}"] = a / b
                cands[f"pct{i}"] = 100 * a / b
                cands[f"growth{i}"] = (a - b) / b
                cands[f"growthpct{i}"] = 100 * (a - b) / b
        if a and len(vs) > 2:
            cands[f"share_of{i}"] = rest / a
            cands[f"sharepct_of{i}"] = 100 * rest / a
            cands[f"diff_of{i}"] = a - rest
    for k, v in cands.items():
        if match_mask(np.array([float(v)]), tg)[0]:
            return k
    return None


def classify_miss(it, atom, tables, q, res) -> str:
    if it["dataset"] == "hitab" and it.get("cells"):
        ops, why = hitab_gold_ops(it, tables)
        if ops is None:
            return "operand cells not in any table"
    if res.get("strong"):
        return "only loosely grounded programs match"
    if res.get("weak"):
        return "only ungrounded programs match"
    tg = gold_targets(atom, it["question"])
    for kw, label in (({"relaxed_text": True}, "operand column text"), ({"drop_total_labels": True}, "total row in data")):
        r2 = Programs(tables, q, **kw).search(atom, it["question"], 1)
        if r2["tight"]:
            return label
    allvals = np.array([_f_relaxed(v) for t in tables for r in t.rows for v in r], dtype=float)
    if len(allvals) and match_mask(allvals, tg).any():
        return "value in a cell, no grounded program"
    return "outside program space"


def run_item(it: dict) -> dict:
    tables = tables_of(it)
    q = Question(it["question"])
    atoms = c_atoms(it)
    out = {"id": it["id"], "dataset": it["dataset"], "group": it["group"], "side": it["side"], "atoms": []}
    if not atoms:
        return out
    progs = Programs(tables, q)
    gold_ops = None
    if it["dataset"] == "hitab" and it.get("cells"):
        gold_ops, _ = hitab_gold_ops(it, tables)
    for a in atoms:
        r = progs.search(a, it["question"])
        rec = {"text": a["text"], "tight": r["tight"], "loose": r["strong"], "ungrounded": r["weak"],
               "witnesses": r["witnesses"], "reachable": r["tight"] > 0}
        if a.get("moved"):
            rec["moved"] = a["moved"]
        if not rec["reachable"]:
            rec["cause"] = classify_miss(it, a, tables, q, r)
            if r.get("weak_example"):
                rec["weak_example"] = r["weak_example"]
        dec = [progs.search(d, it["question"], 0)["tight"] > 0 for d in decoys(a)]
        rec["decoys"] = [len(dec), sum(dec)]
        if it["dataset"] == "hitab":
            rec["replay"] = replay_gold(it, a)
            if gold_ops is not None:
                sets = progs.witness_operand_sets(r)
                rec["gold_ops_located"] = True
                rec["agree"] = any(s == gold_ops for s in sets)
                rec["agree_superset"] = any(gold_ops <= s for s in sets)
                rec["agree_subset"] = any(s and s <= gold_ops for s in sets)
                rec["n_answers"] = len(atoms)
            else:
                rec["gold_ops_located"] = False
        out["atoms"].append(rec)
    out["reachable"] = all(x["reachable"] for x in out["atoms"])
    return out


def run(datasets, side: str, cache: Path | None = None, progress=False) -> list[dict]:
    out = []
    for ds in datasets:
        items = [it for it in acc.load_items(ds, side) if it.get("atoms")]
        for n, it in enumerate(items):
            r = run_item(it)
            if r["atoms"]:
                out.append(r)
            if progress and n % 200 == 0:
                print(f"  {ds} {n}/{len(items)}", file=sys.stderr, flush=True)
    return out


def summarize(rows: list[dict]) -> dict:
    s = Counter()
    causes = Counter()
    for r in rows:
        s["items"] += 1
        s["reachable"] += r["reachable"]
        for a in r["atoms"]:
            s["atoms"] += 1
            s["atoms_reachable"] += a["reachable"]
            s["decoys"] += a["decoys"][0]
            s["decoys_reachable"] += a["decoys"][1]
            if not a["reachable"]:
                causes[a["cause"]] += 1
                s["ungrounded_only"] += a["cause"] == "only ungrounded programs match"
                s["loose_only"] += a["cause"] == "only loosely grounded programs match"
            else:
                s["single_witness"] += a["tight"] == 1
    return {"counts": dict(s), "causes": dict(causes)}


def validate(rows: list[dict], show: bool) -> dict:
    v = Counter()
    for r in rows:
        if r["dataset"] != "hitab":
            continue
        for a in r["atoms"]:
            v["hitab_atoms"] += 1
            if not a.get("gold_ops_located"):
                v["gold_ops_not_located"] += 1
                continue
            v["located"] += 1
            if a.get("replay"):
                v["replayable"] += 1
                v["replayable_reachable"] += a["reachable"]
                v["replayable_agree"] += bool(a.get("agree"))
                v["replayable_agree_superset"] += bool(a.get("agree_superset"))
                v["replayable_agree_subset"] += bool(a.get("agree_subset"))
                if r.get("atoms") and len(r["atoms"]) == 1:
                    v["single_answer_replayable"] += 1
                    v["single_answer_reachable"] += a["reachable"]
                    v["single_answer_agree_exact"] += bool(a.get("agree"))
            else:
                v["not_replayable"] += 1
                v["not_replayable_reachable"] += a["reachable"]
    return dict(v)


def spot_sample(rows: list[dict], n=30) -> list[dict]:
    pool = [r for r in rows if r["dataset"] == "mimotable" and r["reachable"]]
    pool.sort(key=lambda r: hashlib.sha256(f"spot-2026-10-23:{r['id']}".encode()).hexdigest())
    return pool[:n]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--datasets", default="hitab,mimotable,realhitbench")
    ap.add_argument("--holdout", action="store_true")
    ap.add_argument("--show", type=int, default=0, help="print N misses per dataset")
    ap.add_argument("--validate", action="store_true")
    ap.add_argument("--spot", action="store_true", help="print the 30-item MiMoTable spot sample (dev only)")
    ap.add_argument("--out", help="JSONL of per-item results")
    a = ap.parse_args()
    side = "holdout" if a.holdout else "dev"
    rows = run(a.datasets.split(","), side, progress=True)
    if a.out:
        Path(a.out).write_text("".join(json.dumps(r, ensure_ascii=False, default=str) + "\n" for r in rows))
    for ds in a.datasets.split(","):
        print(ds, json.dumps(summarize([r for r in rows if r["dataset"] == ds])))
    if a.validate:
        print("validation", json.dumps(validate(rows, a.show)))
    if a.show:
        items = {it["id"]: it for ds in a.datasets.split(",") for it in acc.load_items(ds, side)}
        for ds in a.datasets.split(","):
            ms = [r for r in rows if r["dataset"] == ds and not r["reachable"]][: a.show]
            for r in ms:
                it = items[r["id"]]
                print(f"\n== {ds} {r['id']} | {it['files'][0]}\nQ: {it['question']}")
                for x in r["atoms"]:
                    print(f"   {x['text']}: {'ok' if x['reachable'] else x['cause']} {x['witnesses'][:2] or x.get('weak_example', '')}")
    if a.spot and not a.holdout:
        items = {it["id"]: it for it in acc.load_items("mimotable", "dev")}
        for r in spot_sample(rows):
            it = items[r["id"]]
            print(f"\n== {r['id']} | {it['files'][0]}\nQ: {it['question']}\nA: {AMtrim(it)}")
            for x in r["atoms"]:
                print(f"   {x['text']}: {x['tight']} witnesses; {x['witnesses'][:3]}")


def AMtrim(it):
    raw = {x["id"]: x for x in acc.read_jsonl(acc.data_dir() / "mimotable" / "items.jsonl")}
    return raw[it["id"]]["answer"][:500]


if __name__ == "__main__":
    main()
