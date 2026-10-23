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
  - thresholds (> >= < <=) with a number of the question on a question-grounded numeric column (counts only);
  - numeric ranges ("aged 18 to 34", "65 and over"): the rows whose label ranges tile the question's range (a
    set: never a single-row lookup; a table with such rows names no row outside the range);
  - the year next to the one named, when the question asks for a change ("up from fy 2016"): only as the
    partner of the named year in a difference / growth, never an answer alone.
Wholes: hedgehog's total / subtotal rows, total-labelled rows and columns, arithmetic wholes (a row equal to
  the sum of the 2+ rows right below / above it, a column equal to the sum of the 2+ columns next to it), and
  'All X' rows named by X. A whole the question does not name enters a program only as the denominator of a
  share of a named part inside its block (|part| <= |whole|).
Aggregates over a row set (data rows only: `_row_type` = data; total / subtotal / of-which rows are used
only by single-row lookups, which may pick any row a filter names):
  value lookup (one row), sum, avg, min, max, range (max-min), count (non-empty), count of rows, count of
  distinct labels; for one looked-up row: sum / avg / min / max across its question-grounded numeric
  columns, the named years' columns and all its numeric columns; rank of the value in its column (desc and
  asc); argmax / argmin: the value of a named column (or the year label) in the row where a fully named
  column is largest / smallest ("which ... has the highest ..."), the year of a row's largest / smallest
  year column ("in which year"); the k-th largest / smallest ("second highest"); counts of rows where one
  named column exceeds another; 100 - x of a percentage whose row label negates what the question asks.
Binary programs over pairs of grounded values (lookups and aggregates): a-b, a+b, a/b, 100 a/b,
(a-b)/b, 100 (a-b)/b, 1 - a/b (complement share); date lookups: difference in days. Pair guards: a share
divides a part by a whole unless the question asks for a ratio ("times", "ratio", "as a percentage of");
growth compares two periods (or two entities with "than" / "compared" / "from A to B"), never a column of
changes; "+" never sums overlapping row sets; two measures of one thing ('Mean' / 'S.D.') are no pair;
IN-lists only as one set in two columns or as the part of a share; "how many times" is only a ratio.
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

Miss causes: every missed atom gets one family (library / oracle-limit / gold/question), a cause and an
evidence string: causes.py (source header grounding for generated column names, HiTab's gold operand cells
and header paths, the question's shape).

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
    "count": r"\bhow many\b(?!\s+(?:percent|percentage|times|more|less|fewer|dollars?|millions?|billions?|thousands?|acres|hours|points|\$))|number of|\bcount|多少|几",
    "rank": r"\brank|position|\bplace\b|排名|第几",
    "-": r"differ|more|less|fewer|higher|lower|greater|smaller|exceed|gap|minus|subtract|increase|decrease|change|"
         r"compar|than|between|grow|drop|rise|rose|fell|fall|gain|loss|surplus|deficit|remain|left|balance|"
         r"declin|reduc|差|多|少|增|减|变化|比|剩",
    "+": r"total|sum|combined|together|plus|altogether|合计|加|共",
    "/": r"ratio|times|percent|proportion|share|fraction|rate|\bper\b|%|out of|divid|account|比例|占|倍|率|百分",
    "growth": r"grow|increase|decrease|change|rose|rise|fell|fall|drop|declin|growth|\bup\b|\bdown\b|gain|loss|增长|下降|变化|增幅|降幅",
}
_OPS = {k: re.compile(v, re.I) for k, v in _OPS.items()}
_OP_OF = {"sum": "sum", "avg": "avg", "max": "max", "min": "min", "range": "range", "count": "count",
          "count rows": "count", "count distinct": "count", "rank desc": "rank", "rank asc": "rank",
          "row-sum": "sum", "row-avg": "avg", "row-max": "max", "row-min": "min",
          "argmax-value": "max", "argmin-value": "min", "argmax-year": "max", "argmin-year": "min",
          "kth-max": "max", "kth-min": "min", "compl": "/",
          "-": "-", "+": "+", "/": "/", "100*/": "/", "(a-b)/b": "growth", "100*(a-b)/b": "growth"}


_TIMES_Q = re.compile(r"how many times|\btimes (as|more|less|higher|greater|larger|lower|that|than)|\bfold\b", re.I)


def op_named(name: str, q: str) -> bool:
    k = _OP_OF.get(name)
    if k in ("-", "growth") and _TIMES_Q.search(q):
        return False  # "how many times ... than": a ratio, not a difference
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


_SYN = {"men": "male", "man": "male", "males": "male", "women": "female", "woman": "female", "females": "female"}


def stem(w: str) -> str:
    if w in _SYN:
        return _SYN[w]
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


_AMP = re.compile(r"(?<![A-Za-z])([A-Za-z])\s*&\s*([A-Za-z])(?![A-Za-z])")


def _pre(s) -> str:
    """'R&D', 'S&E', 'S & P' -> 'RD', 'SE', 'SP': norm_text would split them into single letters, which no
    word test keeps."""
    return _AMP.sub(r"\1\2", str(s)) if s is not None else ""


# Header words that name no measure: a header is fully named without them ("In-Stock Details / Amount").
_GENERIC = {"detail", "information", "info", "statistic", "situation", "record", "status", "summary", "registration",
            "form", "list", "description", "particular", "indicator"}

_UNIT_WORDS = {"current", "constant", "nominal", "number", "percent", "percentage", "pct", "rate", "dollar", "thousand", "million", "billion", "unit",
               "yuan", "usd", "eur", "count", "value", "amount", "share", "proportion", "per", "cent"}


def cwords(s: str) -> set[str]:
    """Content words for column grounding: like common.words, but keeping 'amount', 'number', 'value',
    'total' (column names use them) and folding plurals."""
    out = set()
    for w in norm_text(_pre(s)).split():
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


_RANGE_Q = re.compile(r"(?<![\d,.])\b\d{1,3}\s*(?:to|-)\s*\d{1,3}\b(?![\d,.%]|\s*percent)|\bbetween\s+\d{1,3}\s+and\s+\d{1,3}\b(?![\d,.%])|(?<![\d,.])\b\d{1,3}\s*(?:years?\s*)?(?:and over|and older|or older|or more|plus)\b|\b(?:under|less than|younger than)\s+\d{1,3}\b")


def _range_of(text: str):
    """(lo, hi) of a label or question range ("18 to 24", "25-34", "65 years and over", "under 15"); hi None =
    open; (None, None) when the text is no range."""
    t = re.sub(r"\s+", " ", str(text).lower().replace("–", "-").replace("−", "-"))
    m = re.search(r"(?<![\d.])(\d{1,3})\s*(?:years?\s*)?(?:to|and|-)\s*(\d{1,3})(?![\d.])", t) if not re.search(r"\band (over|older)\b", t) else None
    if m:
        lo, hi = int(m.group(1)), int(m.group(2))
        return (lo, hi) if lo < hi else (None, None)
    m = re.search(r"(?<![\d,.])\b(\d{1,3})\s*(?:years?\s*)?(?:and over|and older|or older|or more|plus)\b", t) or re.search(r"(?<![\d,.])\b(\d{1,3})\s*\+", str(text))
    if m:
        return int(m.group(1)), None
    m = re.search(r"\b(?:under|less than|younger than)\s+(\d{1,3})\b(?![\d,.])", t)
    if m:
        return 0, int(m.group(1)) - 1
    return None, None


_CHANGE_Q = re.compile(r"\b(increas\w*|decreas\w*|chang\w*|grow\w*|growth|rose|rise[sn]?|fell|fall\w*|drop\w*|declin\w*|up|down|"
                       r"previous|prior|last|next|year earlier|a year before|compared)\b")
_NEG = re.compile(r"\b(not|no|never|without|neither|nor|none)\b|n t\b")


_CMPR_Q = re.compile(r"\b(than|compared|relative to|versus|vs)\b|\bfrom\b(?!\s+(fy\s+)?(19|20)\d\d)[^,?]+\bto\b")
_STAT_WORDS = {"mean", "average", "median", "s d", "sd", "standard deviation", "standard error", "se", "cv", "n", "number",
               "percent", "percentage", "%", "rate", "count", "lower", "upper", "low", "high", "lower bound", "upper bound",
               "95 ci", "confidence interval", "min", "max", "minimum", "maximum", "total", "share", "proportion"}


def _stat(seg: str) -> bool:
    return _n(re.sub(r"\([^)]*\)", " ", seg)) in _STAT_WORDS
_RATIO_Q = re.compile(r"\b(times|ratio|fold|multiple|as (much|many|high|large|likely)|relative|compared|per)\b|"
                      r"\bas an? (percentage|proportion|share|fraction) of\b|\b(of|to) th(at|ose) of\b")


def _in_range(lo, hi, qlo, qhi) -> bool:
    if lo < qlo:
        return False
    if qhi is None:
        return True
    return hi is not None and hi <= qhi


class Question:
    def __init__(self, q: str):
        self.text = q
        self.norm = norm_text(_pre(q))
        self.stemmed = " " + " ".join(stem(w) for w in self.norm.split()) + " "
        self.words = {stem(w) for w in words(_pre(q))}
        self.tokens = set(self.norm.split())
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
        # numeric ranges the question names ("aged 18 to 34", "65 and over"): rows whose label ranges tile it
        self.ranges = []
        for m in _RANGE_Q.finditer(re.sub(r"\s+", " ", q.lower().replace("–", "-"))):
            lo, hi = _range_of(m.group(0))
            if lo is not None and not (1900 <= lo <= 2100 and (hi or 0) <= 2100 and hi is not None and hi >= 1900):
                self.ranges.append((lo, hi))
        # one named year and a change word: the change is to / from the adjacent year ("up from fy 2016",
        # "compared to the previous year")
        self.adj_years = set()
        if len(self.years) == 1 and not self.year_range and _CHANGE_Q.search(self.norm):
            y = next(iter(self.years))
            self.adj_years = {y - 1, y + 1}
        self.year_q = bool(re.search(r"\b(which|what|in what) (fiscal )?(year|period)|\bwhen\b", self.norm))
        self.cmp_q = bool(re.search(r"\b(more|greater|higher|larger|exceed\w*|above|over|less|lower|smaller|below|under|fewer)\b", self.norm))
        self.neg_q = bool(_NEG.search(self.norm))
        # "which X has the highest Y", "the province with the lowest rate": a row picked by an extreme
        self.arg_q = bool(re.search(r"\b(which|whose|who|where|that|with|having|has|had)\b[^?]{0,80}?\b(highest|largest|most|greatest|"
                                    r"biggest|maximum|lowest|smallest|least|fewest|minimum)\b", self.norm))
        # ordinal superlatives: "the second highest", "3rd largest"
        km = re.search(r"\b(second|third|fourth|fifth|2nd|3rd|4th|5th)\s+(?:\w+\s+)?(highest|largest|biggest|most|greatest|lowest|smallest|least|fewest)\b", self.norm)
        self.kth = ({"second": 2, "third": 3, "fourth": 4, "fifth": 5, "2nd": 2, "3rd": 3, "4th": 4, "5th": 5}[km.group(1)],
                    "max" if km.group(2) in ("highest", "largest", "biggest", "most", "greatest") else "min") if km else None

    def entity(self, value, strict=False) -> int:
        """0 no, 2 phrase at word boundaries, 1 all content words (2+)."""
        value = _pre(value)
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
        m = re.match(r"(?:all|total)\s+(.{3,})$", n)  # "All R&D expenditures": the whole of "R&D expenditures"
        if m and words(m.group(1)) and f" {' '.join(stem(w) for w in m.group(1).split())} " in self.stemmed:
            return 2
        ws = {stem(w) for w in words(value)}
        if len(ws) >= 2 and _wmatch(ws, self.words) == ws:
            return 1
        ini = "".join(w[0] for w in n.split() if w not in ("and", "of", "the", "for", "on", "in", "de", "et"))
        if len(ini) >= 3 and ini in self.tokens and len(n.split()) >= 3:
            return 2  # an acronym of the label: "NASA" ~ "National Aeronautics and Space Administration"
        if not strict and len(ws) >= 2 and len(_wmatch(ws, self.words)) >= max(2, 0.6 * len(ws)):
            return 1  # most content words: "poor or fair mental health" ~ "Negative (fair, poor)"
        return 0

    def col_grounded(self, header: str) -> int:
        """2: every content word of one header segment is in the question; 1: some word; 0: none."""
        hw = cwords(header)
        if not _wmatch(hw, self.cwords):
            return 0
        strip = lambda x: re.sub(r"\([^)]*\)|（[^）]*）|\[[^]]*\]", " ", x)  # units in brackets do not count
        segs = [x for x in header.split(" / ") if cwords(strip(x)) - _UNIT_WORDS - _GENERIC] or header.split(" / ")
        leaf = cwords(strip(segs[-1])) - _UNIT_WORDS - _GENERIC
        whole = cwords(strip(header)) - _UNIT_WORDS - _GENERIC
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
        self.wr, self.wc = self._wholes()
        self.filters = self._filters(q, dtypes)
        self.vals, self.grd, self.meta, self.flags = [], [], [], []
        self.dates = []  # (date tuple, desc, operand)
        self._enumerate(q, dtypes)

    # wholes ---------------------------------------------------------------------------------
    def _wholes(self):
        """Rows and columns that are a whole of others: hedgehog's total / subtotal rows, total-labelled rows
        and columns ("Total", "All agencies"), and arithmetic wholes - a row equal to the sum of the block of
        2+ rows right below or above it, a column equal to the sum of the 2+ columns right after or before
        it (0.5 %, in 2+ columns / rows and 60 % of the comparable ones). The denominators of shares."""
        t, X = self.t, self.X
        n, k = X.shape
        wr = np.zeros(n, dtype=bool)
        if t.row_types:
            wr |= np.array([x in ("total", "subtotal") for x in t.row_types], dtype=bool)
        cm = getattr(t, "colmeta", None) or []
        own = sorted(c for c in t.label_cols - t.parent_cols if not (c < len(cm) and cm[c].get("role") == "group")) or \
            sorted(t.label_cols - t.parent_cols)
        self.wr_label = np.zeros(n, dtype=bool)
        for i, r in enumerate(t.rows):
            lab = " ".join(norm_text(r[c]) for c in own if c < len(r) and r[c] not in (None, ""))
            if lab and TOTAL_LABEL.search(lab):
                self.wr_label[i] = True
        wr |= self.wr_label
        wc = np.array([any(TOTAL_LABEL.search(_n(seg)) for seg in t.headers[c].split(" / ")) for c in self.numcols], dtype=bool)
        if k == 0:
            return wr, wc

        def blocks(A, maxm):
            """Mask over A's rows: the row equals the sum of the next / previous m >= 2 rows (per column)."""
            nn = A.shape[0]
            Z = np.where(np.isnan(A), 0.0, A)
            N = np.isnan(A).astype(int)
            C = np.vstack([np.zeros((1, A.shape[1])), np.cumsum(Z, axis=0)])
            CN = np.vstack([np.zeros((1, A.shape[1]), dtype=int), np.cumsum(N, axis=0)])
            res = np.zeros(nn, dtype=bool)
            spans = {}
            for i in range(nn):
                x = A[i]
                okx = ~np.isnan(x) & (x != 0)
                if okx.sum() == 0:
                    continue
                need = 1 if A.shape[1] == 1 else 2
                for lo, hi in ((i + 1, min(nn, i + 1 + maxm)), (max(0, i - maxm), i)):
                    if hi - lo < 2:
                        continue
                    if lo == i + 1:
                        ends = np.arange(lo + 2, hi + 1)  # block i+1 .. e-1
                        S = C[ends] - C[i + 1]
                        NN = CN[ends] - CN[i + 1]
                    else:
                        starts = np.arange(hi - 2, lo - 1, -1)  # block s .. i-1
                        S = C[i] - C[starts]
                        NN = CN[i] - CN[starts]
                    tol = 0.005 * np.abs(x) + 1e-9
                    hit = (np.abs(S - x) <= tol) & (NN == 0) & okx
                    comp = (NN == 0) & okx
                    good = (hit.sum(axis=1) >= need) & (hit.sum(axis=1) >= 0.6 * np.maximum(comp.sum(axis=1), 1))
                    if good.any():
                        res[i] = True
                        g = int(np.nonzero(good)[0][0])
                        spans[i] = (i + 1, int(ends[g])) if lo == i + 1 else (int(starts[g]), i)
                        break
            return res, spans

        # the rows (columns) a whole is the whole of: its block, else the whole table
        self.rspan, self.cspan = {}, {}
        if n <= 400:
            r, sp = blocks(X, 40)
            wr |= r
            self.rspan = sp
        if k >= 3 and n >= 1 and n <= 2000:
            r, sp = blocks(X.T.copy(), 20)
            wc |= r
            self.cspan = sp
        return wr, wc

    # filters --------------------------------------------------------------------------------
    def _filters(self, q: Question, dtypes) -> list[tuple]:
        t = self.t
        out = []
        labelish = sorted(t.label_cols | t.parent_cols | {c for c, d in enumerate(dtypes) if d in ("text", "string")})
        partial = {}
        for c in labelish:
            seen = {}
            for i, r in enumerate(t.rows):
                v = r[c] if c < len(r) else None
                if v in (None, "") or isinstance(v, (int, float)) and not isinstance(v, bool) and not (1900 <= v <= 2100):
                    continue
                key = str(v)
                if key not in seen:
                    seen[key] = q.entity(key) if not isinstance(v, (int, float)) else (2 if int(v) in q.years and v == int(v) else 0)
                    if seen[key] == 1 and not isinstance(v, (int, float)) and q.entity(key, strict=True) == 0:
                        partial.setdefault(c, set()).add(key)
                if seen[key]:
                    out.append((c, key, seen[key]))
                elif isinstance(v, (int, float)) and v == int(v) and int(v) in q.adj_years:
                    out.append((c, f"adj-year={int(v)}", 0))
        # a value named by most of its words ("National Laboratory") names nothing when several values of its
        # column are named that way
        out = [x for x in out if not (x[0] in partial and x[1] in partial[x[0]] and len(partial[x[0]]) >= 3)]
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
                    if isinstance(v, (int, float)) and not isinstance(v, bool) and v == int(v) and int(v) in q.adj_years:
                        out.append((c, f"adj-year={int(v)}", 0))
        # year tokens inside labels ("2015 Q1"), a year range, row ids
        for c in labelish:
            for r in t.rows:
                v = r[c] if c < len(r) else None
                if isinstance(v, str):
                    ys = {int(y) for y in re.findall(r"(?<!\d)((?:19|20)\d\d)(?!\d)", v)}
                    for y in ys & q.years:
                        out.append((c, f"has-year={y}", 1))
                    for y in ys & q.adj_years:
                        out.append((c, f"adj-year={y}", 0))
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
        # numeric ranges: rows whose label ranges lie inside a range the question names and tile it
        for qlo, qhi in q.ranges:
            for c in labelish:
                got = []
                for r in t.rows:
                    v = r[c] if c < len(r) else None
                    if isinstance(v, str):
                        lo, hi = _range_of(v)
                        if lo is not None and _in_range(lo, hi, qlo, qhi):
                            got.append((lo, hi))
                if got and min(x[0] for x in got) == qlo and (
                        (qhi is None and any(x[1] is None for x in got)) or (qhi is not None and max(x[1] or -1 for x in got) == qhi)):
                    out.append((c, f"range={qlo}..{'' if qhi is None else qhi}", 2))
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
                if key.startswith("range="):
                    lo, hi = _range_of(v) if isinstance(v, str) else (None, None)
                    a, b = key[6:].split("..")
                    m[i] = lo is not None and _in_range(lo, hi, int(a), int(b) if b else None)
                    continue
                if key.startswith(("has-year=", "year-in=", "id=", "adj-year=")):
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
                    if kind in ("has-year", "adj-year"):
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
    def _add(self, v, g, meta, flags=(False, False, False)):
        self.vals.append(v)
        self.grd.append(g)
        self.meta.append(meta)
        self.flags.append(flags)

    def _aggs(self, desc, mask, rowg, ops=True, adj=False):
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
                i0 = int(np.nonzero(mask)[0][0])
                # a row the question names by its own (non-total) label is a part, whatever hedgehog typed it
                wrow = bool(self.wr_label[i0]) or (bool(self.wr[i0]) and rowg == 0) or desc == "all data rows"
                self._add(s[j], g, ("value", desc, j, mask), (wrow, bool(self.wc[j]), adj))
                continue
            for name, val in (("sum", s[j]), ("avg", s[j] / cnt[j]), ("max", mx[j]), ("min", mn[j]),
                              ("range", mx[j] - mn[j]), ("count", float(cnt[j]))):
                self._add(val, g, (name, desc, j, mask), (False, False, adj))

    def _enumerate(self, q: Question, dtypes):
        t = self.t
        D = self.data
        hy = [{int(y) for y in re.findall(r"(?<!\d)((?:19|20)\d\d)(?!\d)", t.headers[c])} for c in self.numcols]
        self.ycols = np.array([len(h) == 1 and bool(h & q.years) for h in hy], dtype=bool)
        self.yearof = [next(iter(h)) if len(h) == 1 else None for h in hy]
        self.pctcol = []
        for jj, c in enumerate(self.numcols):
            col = self.X[:, jj]
            col = col[~np.isnan(col)]
            pc = dtypes[c] == "percent" or bool(re.search(r"percent|%|proportion|share|rate\b", t.headers[c], re.I))
            self.pctcol.append(0 if not pc or not len(col) or col.min() < 0 else 1 if col.max() <= 1 and dtypes[c] == "percent" else 2 if col.max() <= 100 else 0)
        # a column of the year next to the one named; only a change partner of the named year's column
        self.adjc = np.array([bool(h & q.adj_years) and not (h & q.years) for h in hy], dtype=bool)
        self._aggs("all data rows", D, 0)
        self._add(float(D.sum()), (0, int(q.count)), ("count rows", "all data rows", None, D))
        fl = self.filters
        sets = [(d, m, 1) for d, m, c in fl]
        pairs = 0
        for (d1, m1, c1), (d2, m2, c2) in combinations(fl, 2):
            if c1 == c2 or pairs >= MAX_PAIRS or "'adj-year=" in d1 + d2:
                continue
            m = m1 & m2
            if m.any():
                sets.append((f"{d1} and {d2}", m, 2))
                pairs += 1
        bycol = defaultdict(list)
        for d, m, c in fl:
            if "'adj-year=" not in d:
                bycol[c].append((d, m))
        for c, lst in bycol.items():
            if 2 <= len(lst) <= 12:
                m = np.zeros(len(D), dtype=bool)
                for _, mm in lst:
                    m |= mm
                if (m & ~self.wr & D).any():
                    m &= ~self.wr  # a whole and its parts: never summed together
                sets.append((" or ".join(d for d, _ in lst), m, 1))
        for desc, m, rg in sets:
            md = m & D
            if md.sum() >= 2:
                adj = "'adj-year=" in desc
                self._aggs(desc, md, rg, adj=adj)
                self._add(float(md.sum()), (rg, 1), ("count rows", desc, None, md), (False, False, adj))
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
        for d, m, _ in fl:
            if "'range=" not in d:  # a range names its rows as a set, not one by one
                rows |= m
        idx = list(np.nonzero(rows)[0][:MAX_LOOKUP_ROWS])
        # whole rows (denominators of shares), also when the question does not name them
        idx += [i for i in np.nonzero(self.wr & ~rows)[0][:30]]
        nf = np.array([sum(bool(m[i]) for d, m, _ in fl if "'adj-year=" not in d and "'range=" not in d) for i in idx], dtype=int) if len(idx) else []
        na = np.array([sum(bool(m[i]) for d, m, _ in fl if "'adj-year=" in d) for i in idx], dtype=int) if len(idx) else []
        inrange = np.zeros(len(D), dtype=bool)
        for d, m, _ in fl:
            if "'range=" in d:
                inrange |= m
        for i, k, ka in zip(idx, nf, na):
            if inrange.any() and not inrange[i] and k < 2 and not self.wr[i]:
                continue  # the question's age range applies to this table: a row outside it is not the one named
            one = np.zeros(len(D), dtype=bool)
            one[i] = True
            lab = " / ".join(t.labels(i)[:3]) or f"row {i}"
            desc = f"row [{lab}]"
            rg = 2 if k >= 2 else 1 if k == 1 else 0
            adj = k == 0 and ka > 0  # named only as the year next to the question's year
            if adj:
                rg = 1
            self._aggs(desc, one, rg, adj=adj)
            row = self.X[i]
            ok = ~np.isnan(row)
            sets_c = [("grounded columns", (self.colg >= 1) & ok), ("all numeric columns", ok)]
            if q.year_range:
                yc = np.array([any(q.year_range[0] <= int(y) <= q.year_range[1] for y in re.findall(r"(?<!\d)((?:19|20)\d\d)(?!\d)", t.headers[c]))
                               for c in self.numcols], dtype=bool)
                sets_c.append(("columns of the year range", yc & ok))
            if len(q.years) >= 2:
                sets_c.append(("columns of the named years", self.ycols & ok))
            for name_set, sel in sets_c:
                if sel.sum() >= 2:
                    v = row[sel]
                    for name, val in (("sum", v.sum()), ("avg", v.mean()), ("max", v.max()), ("min", v.min())):
                        self._add(val, (rg, 0 if name_set == "all numeric columns" else 2), (f"row-{name}", f"{desc} across {name_set}", ("cols", tuple(np.nonzero(sel)[0])), one), (False, False, adj))
            for j in np.nonzero((self.colg >= 1) & ok)[0]:
                col = self.X[D, j]
                col = col[~np.isnan(col)]
                v = row[j]
                self._add(float(1 + np.sum(col > v)), (rg, 1), ("rank desc", desc, j, one), (False, False, adj))
                self._add(float(1 + np.sum(col < v)), (rg, 1), ("rank asc", desc, j, one), (False, False, adj))
            # "in which year was X highest": the year of the extreme across the row's year columns
            yh = [(jj, self.yearof[jj]) for jj in np.nonzero(ok)[0] if self.yearof[jj]]
            if q.year_q and len(yh) >= 2 and not adj and len({y for _, y in yh}) == len(yh):
                vals = np.array([row[jj] for jj, _ in yh])
                for side, f in (("argmax", np.argmax), ("argmin", np.argmin)):
                    b = int(f(vals))
                    if np.sum(vals == vals[b]) == 1:
                        self._add(float(yh[b][1]), (rg, 2), (f"{side}-year", f"{desc} across the year columns", ("cols", tuple(jj for jj, _ in yh)), one))
            # complement of a percentage: the question asks for the share the row's label negates, or the
            # other way round ("not employed" in the question, "Employed" the row)
            if not adj and PCT_Q.search(q.text.lower()):
                labn = " ".join(norm_text(t.rows[i][c]) for c in sorted(t.label_cols - t.parent_cols)
                                if c < len(t.rows[i]) and t.rows[i][c] not in (None, ""))
                if not q.neg_q and _NEG.search(labn):
                    for j in np.nonzero((self.colg >= 1) & ok)[0]:
                        if self.pctcol[j]:
                            top = 1.0 if self.pctcol[j] == 1 else 100.0
                            self._add(top - row[j], (rg, int(self.colg[j])), ("complement", desc, int(j), one))
            for c, d in enumerate(dtypes):
                if d in ("date", "datetime"):
                    dt = _date(t.rows[i][c] if c < len(t.rows[i]) else None)
                    if dt:
                        self.dates.append((dt, f"{desc}.{t.headers[c]}", (t.id, int(i), c)))
        # argmax / argmin: the value of a named column (or the year label) in the row where a question-
        # grounded column is largest / smallest ("the population of the province with the highest rate")
        if self.numcols:
            yl = self.year_labels()
            for desc, m, rg in [("all data rows", D, 0)] + [(d, mm & D, r) for d, mm, r in sets if "'adj-year=" not in d]:
                if m.sum() < 2:
                    continue
                for a in np.nonzero(self.colg >= 1)[0]:
                    col = np.where(m, self.X[:, a], np.nan)
                    if np.all(np.isnan(col)):
                        continue
                    # the ranking column must be the one the question names
                    for side, f in ((("argmax", np.nanargmax), ("argmin", np.nanargmin)) if self.colg[a] == 2 and q.arg_q else ()):
                        r0 = int(f(col))
                        if np.sum(col == col[r0]) != 1:
                            continue  # a tie names no row
                        one = np.zeros(len(D), dtype=bool)
                        one[r0] = True
                        dd = f"{side} {t.headers[self.numcols[a]]} of {desc}"
                        for b in np.nonzero(self.colg == 2)[0]:
                            if b != a and not np.isnan(self.X[r0, b]):
                                self._add(float(self.X[r0, b]), (max(rg, 1), 2), (f"{side}-value", dd, int(b), one))
                        if q.year_q and yl[r0]:
                            self._add(float(yl[r0]), (max(rg, 1), int(self.colg[a])), (f"{side}-year", dd, int(a), one))
                    # the k-th largest / smallest
                    if q.kth:
                        k, side = q.kth
                        v = np.sort(col[~np.isnan(col)])
                        if len(v) >= k:
                            self._add(float(v[-k] if side == "max" else v[k - 1]), (max(rg, 1), int(self.colg[a])), (f"kth-{side}", f"{k} of {desc}", int(a), m))
        # counts comparing two named columns ("how many cafes have actual > target")
        if q.cmp_q and not q.thresholds:
            nc = [j for j in np.nonzero(self.colg == 2)[0]][:10]
            leaf = lambda j: re.sub(r"\s*\(\d+\)$", "", t.headers[self.numcols[j]].split(" / ")[-1])
            for a, b in combinations(nc, 2):
                if _n(leaf(a)) == _n(leaf(b)):
                    continue  # the same measure of two groups: no comparison the question names
                for opn, mm in ((">", self.X[:, a] > self.X[:, b]), ("<", self.X[:, a] < self.X[:, b])):
                    mm = mm & D
                    self._add(float(mm.sum()), (1, 1), ("count rows", f"{t.headers[self.numcols[a]]} {opn} {t.headers[self.numcols[b]]}", None, mm))

    def year_labels(self) -> list:
        """Per row: the year its labels name (a year number, or one year inside a label text), else None."""
        out = []
        for r in self.t.rows:
            ys = set()
            for c in self.t.label_cols:
                v = r[c] if c < len(r) else None
                if isinstance(v, (int, float)) and not isinstance(v, bool) and v == int(v) and 1900 <= v <= 2100:
                    ys.add(int(v))
                elif isinstance(v, str):
                    ys |= {int(y) for y in re.findall(r"(?<!\d)((?:19|20)\d\d)(?!\d)", v)}
            out.append(next(iter(ys)) if len(ys) == 1 else None)
        return out

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
        vals, grd, ref, flags = [], [], [], []
        for ti, tp in enumerate(self.tps):
            vals.extend(tp.vals)
            grd.extend(tp.grd)
            flags.extend(tp.flags)
            ref.extend((ti, k) for k in range(len(tp.vals)))
        fl = np.array(flags, dtype=bool).reshape(-1, 3)
        self.wrow = fl[:, 0] if len(fl) else np.zeros(0, bool)
        self.wcol = fl[:, 1] if len(fl) else np.zeros(0, bool)
        self.colj = np.array([self.tps[ti].meta[k][2] if isinstance(self.tps[ti].meta[k][2], (int, np.integer)) else -1
                              for ti, k in ref], dtype=np.int64)
        self.tab = np.array([ti for ti, _ in ref], dtype=np.int64)
        adjc = np.array([j >= 0 and bool(self.tps[ti].adjc[j]) for (ti, _), j in zip(ref, self.colj)], dtype=bool)
        # adjacent-year operands: the partner of a change from / to the named year, never an answer alone
        self.adjrow = fl[:, 2] if len(fl) else np.zeros(0, bool)
        self.adjcol = adjc
        self.adj = self.adjrow | adjc
        self.chgcol = np.array([j >= 0 and bool(re.search(r"chang|growth|increase|decrease|differen|revision|\bchg\b|%\s*ch",
                                                           self.tps[ti].t.headers[self.tps[ti].numcols[j]], re.I))
                                for (ti, _), j in zip(ref, self.colj)], dtype=bool)
        self.hasyr = np.array([j >= 0 and bool(re.search(r"(?<!\d)(19|20)\d\d(?!\d)", self.tps[ti].t.headers[self.tps[ti].numcols[j]]))
                               for (ti, _), j in zip(ref, self.colj)], dtype=bool)
        self.vals = np.array(vals, dtype=float)
        g = np.array(grd, dtype=int).reshape(-1, 2)
        self.rowg = g[:, 0] if len(g) else np.zeros(0, int)
        self.colg = g[:, 1] if len(g) else np.zeros(0, int)
        self.strong = (self.rowg + np.minimum(self.colg, 1)) >= 1
        self.ref = ref
        qt = q.text
        names = [self.tps[ti].meta[k][0] for ti, k in ref]
        descs = [self.tps[ti].meta[k][1] for ti, k in ref]
        opg = np.array([op_named(n, qt) or (n == "sum" and (" or " in d or "'range=" in d)) for n, d in zip(names, descs)], dtype=bool)
        self.whole = np.array([d == "all data rows" for d in descs], dtype=bool) | self.wrow
        nonval = np.array([n != "value" for n in names], dtype=bool)
        self.isval = ~nonval
        self.alldata = np.array([d == "all data rows" for d in descs], dtype=bool)
        # operands of one period: a year in the row set (filter or label) or in the column
        self.rtimed = np.array([bool(re.search(r"'(has-year|year|year-in|year-month|adj-year)=|'(19|20)\d\d'|(?<!\d)(19|20)\d\d(?!\d)", d))
                                for d in descs], dtype=bool)
        self.inlist = np.array([" or " in d for d in descs], dtype=bool)
        self.qobj = q
        self.descid = np.unique(np.array([f"{ti}\x00{self.tps[ti].meta[k][1]}" for ti, k in ref], dtype=object), return_inverse=True)[1] \
            if ref else np.zeros(0, int)
        # tight: the operation is named, and the row set and the column are named (an aggregate over all
        # data rows needs a fully named column; a count needs only its row set)
        iscount = np.array([_OP_OF.get(n) == "count" for n in names], dtype=bool)
        iscount &= np.array([n != "count" for n in names], dtype=bool) | (self.colg >= 1)  # non-empty cells of a named column
        rowagg = np.array([n.startswith("row-") for n in names], dtype=bool)
        whole_count = np.array([n in ("count rows", "count distinct") and d == "all data rows" for n, d in zip(names, descs)], dtype=bool)
        if _TIMES_Q.search(qt):
            opg &= False  # "how many times ... than": only a ratio answers it
        self.tight = opg & (((self.rowg >= 1) & (self.colg >= 1)) | (self.whole & nonval & (self.colg == 2)) |
                            (iscount & (self.rowg >= 1)) | (whole_count & (self.colg >= 1)) |
                            (rowagg & (self.rowg >= 1))) & ~self.adj
        self.operand_ok = (self.colg == 2) & ((self.rowg >= 1) | self.whole)
        # a whole column (a total column) is the column a share of a named row divides by, and the column of
        # a question that names rows but no column
        self.opok = self.operand_ok | (self.wcol & ((self.rowg >= 1) | self.wrow)) | (self.adjcol & ((self.rowg >= 1) | self.whole))
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
            whole = bool(self.whole[i])
            if self.rowg[i] >= 1 or whole:
                by_col[(ti, int(j), kind)].append(i)
                by_hdr[(_n(self.tps[ti].t.headers[self.tps[ti].numcols[j]]), kind)].append(i)
            if self.colg[i] or self.wcol[i] or self.adjcol[i]:
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
                dif = [(p, q) for p, q in zip(sx, sy) if p != q] if len(sx) == len(sy) else []
                if len(dif) == 1 and _stat(dif[0][0]) and _stat(dif[0][1]):
                    continue  # 'Mean' and 'S.D.', 'number' and 'percent': two measures of one thing
                out[n] = len(dif) == 1 or (self.wcol[x] != self.wcol[y]) or (self.adjcol[x] != self.adjcol[y])
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

    def family(self, i: int) -> str:
        """The program family of a unary program (validation tallies)."""
        ti, k = self.ref[i]
        name, desc = self.tps[ti].meta[k][:2]
        rows = "all rows" if desc == "all data rows" else "range" if "'range=" in desc else "in-list" if " or " in desc else \
            "top/bottom" if desc.startswith(("top ", "bottom ")) else "row" if desc.startswith("row [") else "filter"
        return f"{name} | {rows}"

    def pair_family(self, op: str, a: int, b: int) -> str:
        ta, tb = self.ref[a][0], self.ref[b][0]
        ja, jb = self.tps[ta].meta[self.ref[a][1]][2], self.tps[tb].meta[self.ref[b][1]][2]
        shape = "2 tables" if ta != tb else "same row" if self.descid[a] == self.descid[b] else "same col" if ja == jb else "other"
        w = " / whole col" if self.wcol[b] else " / whole row" if self.whole[b] else ""
        return f"{op} | {shape}{w}"

    def operands(self, i: int) -> frozenset:
        ti, k = self.ref[i]
        return self.tps[ti].operands(k)

    def pair_tight(self, op: str, I: np.ndarray, J: np.ndarray) -> np.ndarray:
        """Both operands named (row set and column), like kinds: '+' adds values / sums, ratios and growth
        take values / sums / averages, a difference any like pair."""
        ok = self.opok[I] & self.opok[J] & (self.vals[I] != self.vals[J])
        # two whole-column aggregates or two unnamed whole rows: a row set must be named (two columns of one
        # whole row are fine)
        same_row = (self.descid[I] == self.descid[J]) & self.isval[I] & self.isval[J]
        ok &= ~(self.whole[I] & self.whole[J]) | same_row
        ok &= ~(self.wcol[I] & self.wcol[J] & same_row & ((self.colg[I] < 2) | (self.colg[J] < 2)))
        k = self.kind[I]
        if op != "-":
            ok &= k == "vs"
        # A whole the question does not name (a total row / column found by its label, hedgehog's row type or
        # arithmetic) enters a program only as the denominator of a share of a named part that lies inside it
        # (its block) and is not larger; two columns of one unnamed total row need a total word.
        uwr = self.wrow & (self.rowg == 0) & self.isval & ~self.alldata
        uwc = self.wcol & (self.colg < 2)
        ok &= ~(uwr[I] & ~same_row) & ~uwc[I]
        if not self.qobj.total:
            ok &= ~(same_row & uwr[I] & uwr[J])
        # adjacent-year operands: one per pair, in a change (difference, growth, ratio) with the named year's
        # operand in the same row (columns) or the same column (rows)
        adj = self.adj[I] | self.adj[J]
        samecol = (self.tab[I] == self.tab[J]) & (self.colj[I] == self.colj[J])
        wv = self.whole & self.isval
        ok &= ~(self.adj[I] & self.adj[J])
        ok &= ~adj | ((self.adjcol[I] | self.adjcol[J]) & same_row) | ((self.adjrow[I] | self.adjrow[J]) & samecol)
        if op in ("+", "compl"):
            ok &= ~adj
        # a set of rows the question lists (an IN-list) is an operand only as the part of a share of its whole
        # (or one listed set in two named columns)
        same_set = self.descid[I] == self.descid[J]
        ok &= (~self.inlist[J] | same_set) & (~self.inlist[I] | same_set | ((op in ("/", "100*/", "compl")) & (wv[J] | self.wcol[J] | self.alldata[J])))
        if op == "compl":
            ok &= (same_row & self.wcol[J] & ~self.wcol[I]) | (samecol & wv[J] & ~wv[I])
            for n in np.nonzero(ok)[0]:  # the question asks for the share the part's label negates (or v.v.)
                ok[n] = not self.qobj.neg_q and self._negated(int(I[n]))
        if op in ("/", "100*/") and not _RATIO_Q.search(self.qobj.norm):
            # one measure in two years: a ratio only when the question asks for one ("times", "ratio")
            ok &= ~(same_row & self.hasyr[I] & self.hasyr[J])
        if op in ("/", "100*/"):
            # a share divides a part by its whole, never the whole by a part
            ok &= ~(same_row & self.wcol[I] & ~self.wcol[J]) & ~(samecol & wv[I] & ~wv[J])
            if not _RATIO_Q.search(self.qobj.norm):
                # "what percent of X ...": the denominator is a whole (a total row / column, all rows)
                ok &= self.whole[J] | self.wcol[J] | self.alldata[J] | self._named_whole(J)
        if op in ("(a-b)/b", "100*(a-b)/b"):
            ok &= ~(self.chgcol[I] | self.chgcol[J])  # the growth of a column of changes: not a question shape
            if not _CMPR_Q.search(self.qobj.norm):
                # a change over time compares two periods; two entities need "than" / "compared" / "from A to B"
                # two periods: two year columns of one row set, or two year rows of one column
                tm = (self.hasyr[I] & self.hasyr[J] & ~samecol) | (self.rtimed[I] & self.rtimed[J] & ~same_row) | adj
                for n in np.nonzero(ok & ~tm & ~same_row)[0]:
                    tm[n] = self._yrow(int(I[n])) and self._yrow(int(J[n]))
                ok &= tm
        if op == "+":
            for n in np.nonzero(ok & samecol)[0]:  # a sum of two row sets that overlap counts rows twice
                ok[n] = not self._overlap(int(I[n]), int(J[n]))
        need = (uwr[J] & ~same_row) | uwc[J]
        if op not in ("/", "100*/", "compl"):
            ok &= ~need
        else:
            ok &= ~need | (np.abs(self.vals[I]) <= np.abs(self.vals[J]) * (1 + 1e-9))
            for n in np.nonzero(ok & need)[0]:
                ok[n] = self._inside(int(I[n]), int(J[n]))
        return ok

    def _named_whole(self, J: np.ndarray) -> np.ndarray:
        """Operands whose row set or column the question calls a whole ("all", "total" in its own label)."""
        out = np.zeros(len(J), dtype=bool)
        for n, b in enumerate(J):
            ti, k = self.ref[int(b)]
            d = self.tps[ti].meta[k][1]
            out[n] = bool(re.search(r"\b(all|total|overall)\b", _n(d.split(" / ")[-1]))) or bool(self.wcol[int(b)])
        return out

    def _yrow(self, a: int) -> bool:
        """Program a looks up one row that holds a year (a label or a year column): a period."""
        ti, k = self.ref[a]
        tp = self.tps[ti]
        rows = np.nonzero(tp.meta[k][3])[0]
        if len(rows) != 1:
            return False
        for v in tp.t.rows[int(rows[0])]:
            if isinstance(v, (int, float)) and not isinstance(v, bool) and v == int(v) and 1900 <= v <= 2100:
                return True
            if isinstance(v, str) and re.search(r"(?<!\d)(19|20)\d\d(?!\d)", v):
                return True
        return False

    def _negated(self, a: int) -> bool:
        ti, k = self.ref[a]
        tp = self.tps[ti]
        rows = np.nonzero(tp.meta[k][3])[0]
        if len(rows) != 1:
            return False
        r = tp.t.rows[int(rows[0])]
        own = sorted(tp.t.label_cols - tp.t.parent_cols)
        return bool(_NEG.search(" ".join(norm_text(r[c]) for c in own if c < len(r) and r[c] not in (None, ""))))

    def _overlap(self, a: int, b: int) -> bool:
        ta, ka = self.ref[a]
        tb, kb = self.ref[b]
        return ta == tb and bool(np.any(self.tps[ta].meta[ka][3] & self.tps[tb].meta[kb][3]))

    def _inside(self, a: int, b: int) -> bool:
        """Program a's rows (same column) or column (same row) lie inside whole b's block."""
        ta, ka = self.ref[a]
        tb, kb = self.ref[b]
        if ta != tb:
            return False
        tp = self.tps[ta]
        _, _, ja, ma = tp.meta[ka]
        _, _, jb, mb = tp.meta[kb]
        if self.wcol[b] and ja != jb:
            lo, hi = tp.cspan.get(int(jb), (0, len(tp.numcols)))
            return lo <= int(ja) < hi and not tp.wc[int(ja)]
        rb = int(np.nonzero(mb)[0][0])
        lo, hi = tp.rspan.get(rb, (0, len(tp.t.rows)))
        rows = np.nonzero(ma)[0]
        return bool(len(rows)) and bool(np.all((rows >= lo) & (rows < hi) & (rows != rb)))

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
            yield "compl", np.where(B != 0, 1 - A / B, np.nan)

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
        res["fam"] = Counter(self.family(i) for i in strong if self.tight[i])
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
                    res["fam"].update(self.pair_family(op, int(a), int(b)) for a, b in zip(I[pt][:200], J[pt][:200]))
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
            import causes  # noqa: PLC0415  (causes imports this module)
            c = causes.classify(it, a, tables, q, r, progs)
            rec["family"] = c["family"]
            rec["cause"] = f"{c['family']}: {c['cause']}"
            rec["evidence"] = c["evidence"]
            if c.get("witness"):
                rec["witnesses"] = [c["witness"]] + rec["witnesses"]
                rec["source_witness"] = True
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
                s["miss_" + a.get("family", "unclassified")] += 1
                s["loose_only"] += a["loose"] > 0
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
