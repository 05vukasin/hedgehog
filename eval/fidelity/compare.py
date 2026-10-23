"""Value comparison for the cell-fidelity gate: a source cell (source.Src) against hedgehog's value.

Independent of hedgehog's parsers: text numbers and text dates are read here with their own small
rules and every plausible reading is kept (US and European grouping, day/month and month/day), so
hedgehog's value is accepted when it equals one of them; the column pass in check.py then decides
the order and grouping questions that a single cell cannot.

compare(src, hh, col) -> (class, subclass), class one of
  ok | value_changed | date_changed | text_changed | intentionally_dropped
"""

from __future__ import annotations

import datetime as dt
import math
import re
import unicodedata

# ------------------------------------------------------------------------------------------------
# hedgehog's documented normalisations (docs/normalized-output.md)

# Missing tokens (docs: "-", "n/a", "..", ":", "x", "null", ...; core/src/values/missing.rs).
_PLACEHOLDER = re.compile(r"^\s*([-–—−/.…:?*_\\])\1*\s*$")
MISSING_WORDS = {
    "n/a", "na", "n.a.", "n.a", "null", "none", "nan", "nil", "x", "xx", "xxx", "-/-", "--/--", "(-)", "( - )", "n.p.",
    "nema", "nepoznato", "k.a.", "k. a.", "ka", "s.o.", "s/o", "s.d.", "s/d", "nd", "n.d.", "n/d", "n.v.", "nv", "ø",
    "keine angabe", "keine angaben", "ne", "#n/a", "tbd", "unknown", "nicht verfügbar", "not available", "missing",
    "n.e.", "ne.", "n.z.", "nz", "n.s.", "u", "z", "m", "c", "...", "..", ":", "-", "—", "–",
    "not applicable", "n/app", "n.app.", "nicht zutreffend", "sans objet", "no aplica", "non applicabile",
}
_BRACKET_CODE = re.compile(r"^\[[a-z]{1,4}\]$")  # ONS shorthand: [z] not applicable, [x] unavailable, [c] confidential
EXCEL_ERRORS = {"#n/a", "#ref!", "#div/0!", "#value!", "#name?", "#num!", "#null!", "#spill!", "#calc!", "#getting_data", "#n/a!"}


def is_missing_token(s: str) -> bool:
    t = s.strip()
    if not t:
        return True
    if _PLACEHOLDER.match(t):
        return True
    k = t.lower()
    if k in EXCEL_ERRORS or k in MISSING_WORDS:
        return True
    if _BRACKET_CODE.match(k):
        return True
    if k.startswith(":") and len(k) <= 5 and re.fullmatch(r":\s*[bcdefnprsuzm]{1,3}", k):
        return True
    return False


def ws(s: str) -> str:
    """hedgehog's text normalisation: trimmed, every run of whitespace (line breaks too) one space."""
    return " ".join(str(s).replace("​", "").split())


_FOOTNOTE_TAIL = re.compile(r"(\s*([¹²³⁰-⁹]+|\*+|†|‡|\(\d{1,2}\)|\d{1,2}\)|\([a-z]\)|\[\w{1,3}\]))+\s*$")


def strip_footnote(s: str) -> str:
    return _FOOTNOTE_TAIL.sub("", s).rstrip()


def norm_loose(s) -> str:
    s = unicodedata.normalize("NFKC", str(s)).casefold()
    return re.sub(r"[\W_]+", " ", s).strip()


# ------------------------------------------------------------------------------------------------
# text numbers

_CURRENCY = re.compile(
    r"(?i)^(?:[$€£¥￥₹₽₩₺₪₱฿₫]|us\$|usd|eur|gbp|jpy|cny|rmb|hkd|chf|rsd|din\.?|kn|hrk|bam|km|pln|zł|czk|kč|huf|ft|sek|nok|dkk|kr\.?|yuan|元|rub|inr|brl|r\$|aud|a\$|cad|c\$|nzd|try|mxn)\s*|"
    r"\s*(?:[$€£¥￥₹₽₩₺₪₱฿₫]|usd|eur|gbp|jpy|cny|rmb|hkd|chf|rsd|din\.?|kn|hrk|bam|km|pln|zł|czk|kč|huf|ft|sek|nok|dkk|kr\.?|yuan|元|rub|inr|brl|aud|cad|nzd|try|mxn)$")
_SCALE = [
    (re.compile(r"(?i)\s*(thousand|thousands|tsd\.?|tausend|hiljada|hiljade|mille|k)$"), 1e3),
    (re.compile(r"(?i)\s*(million|millions|mio\.?|mil\.?|mn|mln\.?|miliona|milion|millionen)$"), 1e6),
    (re.compile(r"(?i)\s*(billion|billions|bn|mrd\.?|milliarde[n]?|milijarde?|bln\.?)$"), 1e9),
    (re.compile(r"\s*万$"), 1e4),
    (re.compile(r"\s*亿$"), 1e8),
]
_US = re.compile(r"^\d{1,3}(,\d{3})+(\.\d+)?$")
_EU = re.compile(r"^\d{1,3}(\.\d{3})+(,\d+)?$")
_SP = re.compile(r"^\d{1,3}([ '’\u00a0\u202f]+\d{3})+([.,]\d+)?$")
_PLAIN = re.compile(r"^\d+(\.\d+)?$|^\.\d+$")
_COMMA_DEC = re.compile(r"^\d+,\d+$")
_SCI = re.compile(r"^\d+(\.\d+)?[eE][+-]?\d+$")


def number_readings(s: str) -> list[tuple[float, str]]:
    """Every plausible numeric reading of a text cell: [(value, tag)], tag one of text_number,
    text_percent, text_scale_word, text_currency. Empty when the text is no number."""
    t = " ".join(unicodedata.normalize("NFKC", s).split())
    t = t.replace("−", "-").replace("–", "-")
    if not t or len(t) > 40:
        return []
    tags = []
    neg = False
    if t.startswith("(") and t.endswith(")"):
        neg, t = True, t[1:-1].strip()
    for _ in range(2):
        t2 = _CURRENCY.sub("", t).strip()
        if t2 != t:
            tags.append("text_currency")
            t = t2
    if t[:1] in "+-":
        neg ^= t[0] == "-"
        t = t[1:].strip()
        t2 = _CURRENCY.sub("", t).strip()
        if t2 != t:
            tags.append("text_currency")
            t = t2
    if t.endswith("-") and _PLAIN.match(t[:-1] or "x"):  # SAP-style trailing minus
        neg, t = True, t[:-1]
    pct = False
    if t.endswith("%"):
        pct, t = True, t[:-1].strip()
    scale = 1.0
    for rx, f in _SCALE:
        t2 = rx.sub("", t)
        if t2 != t and t2:
            scale, t = f, t2.strip()
            break
    t = _FOOTNOTE_NUM.sub("", t)
    unit = None
    m = _UNIT.match(t) if not (_PLAIN.match(t) or _US.match(t) or _EU.match(t) or _SP.match(t) or _COMMA_DEC.match(t)) else None
    if m:
        t, unit = m.group(1), m.group(2)
    cores: list[float] = []
    if _PLAIN.match(t) or _SCI.match(t):
        cores.append(float(t))
        if re.fullmatch(r"\d{1,3}\.\d{3}", t):
            cores.append(float(t.replace(".", "")))  # 1.234 = 1234 in Europe
    elif _US.match(t):
        cores.append(float(t.replace(",", "")))
        if re.fullmatch(r"\d{1,3},\d{3}", t):
            cores.append(float(t.replace(",", ".")))
    elif _EU.match(t):
        cores.append(float(t.replace(".", "").replace(",", ".")))
    elif _SP.match(t):
        cores.append(float(re.sub(r"[ '’\u00a0\u202f]+", "", t).replace(",", ".")))
    elif _COMMA_DEC.match(t):
        cores.append(float(t.replace(",", ".")))
    if not cores:
        return []
    out = []
    tag = "text_percent" if pct else "text_scale_word" if scale != 1.0 else "text_number_unit" if unit else (tags[0] if tags else "text_number")
    for v in cores:
        v = -v if neg else v
        if pct:
            out.append((v / 100.0, tag))
            out.append((v, tag + "_x100"))
        else:
            out.append((v * scale, tag))
            if scale != 1.0:
                out.append((v, "text_scale_word_dropped"))
            if unit in ("MM", "mm") and "text_currency" in tags:
                out.append((v * 1e6, "text_scale_word"))  # $5MM
    return out


# a number followed by a unit or an annotation: 12mm, 20L, 100(m3), 2(东航), 38(1), 26th
_UNIT = re.compile(r"^(\d+(?:[.,]\d+)?)\s*((?:[^\W\d_]{1,12}\.?\d?|[(（][^()（）]{1,12}[)）]|[²³°′″'\"/]+[^\W\d_]{0,6}))$")
_FOOTNOTE_NUM = re.compile(r"(\s*([¹²³⁰-⁹]+|\*+|†|‡|\s\(?[rpe]\)?))+$")


def num_equal(a: float, b: float, exact_int: bool = False) -> bool:
    if exact_int and float(a).is_integer() and float(b).is_integer():
        return float(a) == float(b)
    if a == b:
        return True
    return abs(a - b) <= 1e-9 * max(abs(a), abs(b))


def parse_hh_number(v: str) -> float | None:
    try:
        f = float(v)
    except ValueError:
        return None
    return None if math.isnan(f) or math.isinf(f) else f


# ------------------------------------------------------------------------------------------------
# text dates

MONTHS = {}
for i, names in enumerate([
    "january jan januar janvier enero gennaio januari janeiro styczen siječanj sijecanj jän jänner janv ene genn",
    "february feb februar fevrier février febrero febbraio februari fevereiro luty veljača veljaca fev févr feb febr",
    "march mar märz marz maerz mars marzo maart março marco marzec ožujak ozujak mart mär mrz",
    "april apr avril abril aprile kwiecien travanj avr abr",
    "may mai mayo maggio mei maio maj svibanj",
    "june jun juni juin junio giugno junho czerwiec lipanj jun juin",
    "july jul juli juillet julio luglio julho lipiec srpanj juil",
    "august aug august août aout agosto augustus sierpien kolovoz avgust ago",
    "september sep sept septembre septiembre settembre setembro wrzesien rujan septembar set",
    "october oct oktober octobre octubre ottobre outubro pazdziernik listopad oktobar okt ott out",
    "november nov novembre noviembre novembro listopad studeni novembar",
    "december dec dezember decembre décembre diciembre dicembre dezembro grudzien prosinac decembar dez dic",
], start=1):
    for n in names.split():
        MONTHS.setdefault(n, i)
WEEKDAYS = set("monday mon tuesday tue tues wednesday wed thursday thu thur thurs friday fri saturday sat sunday sun "
               "montag dienstag mittwoch donnerstag freitag samstag sonntag lundi mardi mercredi jeudi vendredi samedi dimanche".split())


def _y(v: str) -> int | None:
    if len(v) == 4:
        return int(v)
    if len(v) == 2:
        n = int(v)
        return 2000 + n if n < 30 else 1900 + n
    return None


def _mk(y, m, d):
    try:
        return dt.date(y, m, d)
    except (TypeError, ValueError):
        return None


_TIME = re.compile(r"(?i)[ T]+(\d{1,2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?\s*(am|pm)?\s*(z|[+-]\d{2}:?\d{2})?$")


def split_time(s: str):
    m = _TIME.search(s)
    if not m:
        return s, None
    h, mi, se = int(m.group(1)), int(m.group(2)), int(m.group(3) or 0)
    ap = (m.group(4) or "").lower()
    if ap == "pm" and h < 12:
        h += 12
    if ap == "am" and h == 12:
        h = 0
    if h > 23 or mi > 59 or se > 59:
        return s, None
    return s[: m.start()], dt.time(h, mi, se)


def date_readings(s: str) -> tuple[list[tuple[object, str]], str | None]:
    """Readings of a text date: ([(date or datetime, tag)], note). tag: iso | dmy | mdy | ymd |
    month_name | month_year | quarter | cjk. note "no_year" for a day and month without a year
    ("March 25", "3月25日"), "time_only" for a clock time; ([], None) when the text is no date."""
    t = unicodedata.normalize("NFKC", s).strip()
    if not t or len(t) > 48 or not re.search(r"\d", t):
        return [], None
    if re.fullmatch(r"\d{1,2}:\d{2}(:\d{2})?(\s*[ap]m)?", t, re.I):
        return [], "time_only"
    out: list[tuple[object, str]] = []
    # CJK: 2024年3月12日, 3月12日
    m = re.fullmatch(r"(?:(\d{4})\s*年)?\s*(\d{1,2})\s*月\s*(?:(\d{1,2})\s*[日号])?", t)
    if m:
        y, mo, d = m.groups()
        if not y:
            return [], "no_year" if d else None
        r = _mk(int(y), int(mo), int(d or 1))
        return ([(r, "cjk")] if r else []), None
    body, tm = split_time(t)
    body = body.strip().rstrip(".").strip()
    body = re.sub(r"(?i)^(%s)[,.]?\s+" % "|".join(sorted(WEEKDAYS, key=len, reverse=True)), "", body)
    # ISO / ymd
    m = re.fullmatch(r"(\d{4})([-/.])(\d{1,2})\2(\d{1,2})(T.*)?", body)
    if m:
        r = _mk(int(m.group(1)), int(m.group(3)), int(m.group(4)))
        if r:
            out.append((r, "iso" if m.group(2) == "-" else "ymd"))
    m = re.fullmatch(r"(\d{4})\s*[-/.]\s*(\d{1,2})|(\d{1,2})\s*[-/]\s*(\d{4})", body)
    if m:
        y, mo = (m.group(1), m.group(2)) if m.group(1) else (m.group(4), m.group(3))
        r = _mk(int(y), int(mo), 1)
        if r:
            out.append((r, "month_year"))
    m = re.fullmatch(r"(\d{1,2})\s*([-/.])\s*(\d{1,2})\s*\2\s*(\d{4}|\d{2})", body)
    if m:
        a, b, y = int(m.group(1)), int(m.group(3)), _y(m.group(4))
        dm, md = _mk(y, b, a), _mk(y, a, b)
        if dm:
            out.append((dm, "dmy"))
        if md and md != dm:
            out.append((md, "mdy"))
    # month names
    words = re.findall(r"[^\W\d_]+\.?|\d+", body.lower())
    nums = [w for w in words if w.isdigit()]
    mws = [MONTHS.get(w.rstrip(".")) for w in words if not w.isdigit()]
    mws = [x for x in mws if x]
    other = [w for w in words if not w.isdigit() and not MONTHS.get(w.rstrip(".")) and w.rstrip(".") not in
             ("de", "of", "the", "st", "nd", "rd", "th", "er", "godine", "god", "g", "день", "le", "del", "di")]
    if len(mws) == 1 and not other and not out:
        mo = mws[0]
        if len(nums) == 2:
            y4 = [n for n in nums if len(n) == 4]
            if y4:
                d = [n for n in nums if n != y4[0]] or [y4[0]]
                r = _mk(int(y4[0]), mo, int(d[0]))
                if r:
                    out.append((r, "month_name"))
            else:  # 12-Mar-24
                r = _mk(_y(nums[1]), mo, int(nums[0])) if len(nums[1]) == 2 else None
                if r:
                    out.append((r, "month_name"))
        elif len(nums) == 1:
            n = nums[0]
            sep_ym = re.search(r"[-/'’]", body) is not None
            if len(n) == 4:
                out.append((_mk(int(n), mo, 1), "month_year"))
            elif len(n) == 2 and sep_ym:
                out.append((_mk(_y(n), mo, 1), "month_year"))  # Mar-24, Jul '99
            elif len(n) <= 2 and 1 <= int(n) <= 31:
                return [], "no_year"  # March 25, 25 March
            elif len(n) == 2:
                out.append((_mk(_y(n), mo, 1), "month_year"))
    m = re.fullmatch(r"(?i)(?:q([1-4])[\s/-]*(\d{4})|(\d{4})[\s/-]*q([1-4]))", body)
    if m:
        q = int(m.group(1) or m.group(4))
        y = int(m.group(2) or m.group(3))
        out.append((dt.date(y, 3 * q - 2, 1), "quarter"))
    out = [(d, tag) for d, tag in out if d is not None]
    if tm is not None:
        out = [(dt.datetime.combine(d, tm), tag) for d, tag in out]
    return out, None


# ------------------------------------------------------------------------------------------------
# hedgehog's values (CSV text)

_ISO_D = re.compile(r"^(\d{4})-(\d{2})-(\d{2})$")
_ISO_DT = re.compile(r"^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2})(?::(\d{2}))?")
_HMS = re.compile(r"^(\d{1,2}):(\d{2})(?::(\d{2}))?$")


def parse_hh_date(v: str):
    """A date / datetime / time from hedgehog's CSV text; None when it is not one."""
    v = v.strip()
    m = _ISO_DT.match(v)
    if m:
        y, mo, d, h, mi, s = (int(x or 0) for x in m.groups())
        try:
            return dt.datetime(y, mo, d, h, mi, s)
        except ValueError:
            return None
    m = _ISO_D.match(v)
    if m:
        try:
            return dt.date(*(int(x) for x in m.groups()))
        except ValueError:
            return None
    m = _HMS.match(v)
    if m:
        h, mi, s = (int(x or 0) for x in m.groups())
        if h < 24 and mi < 60 and s < 60:
            return dt.time(h, mi, s)
    return None


def same_moment(a, b) -> bool:
    """Source date/datetime/time a vs hedgehog's b, to the second."""
    if isinstance(a, dt.datetime):
        if isinstance(b, dt.datetime):
            return abs((a - b).total_seconds()) <= 1
        if isinstance(b, dt.date):
            return a.time() == dt.time(0) and a.date() == b
        return False
    if isinstance(a, dt.date):
        if isinstance(b, dt.datetime):
            return b.time() == dt.time(0) and b.date() == a
        return isinstance(b, dt.date) and a == b
    if isinstance(a, dt.time):
        if isinstance(b, dt.datetime):
            b = b.time()
        if isinstance(b, dt.time):
            sa = a.hour * 3600 + a.minute * 60 + a.second
            sb = b.hour * 3600 + b.minute * 60 + b.second
            return abs(sa - sb) <= 1
    return False


TRUE = {"true", "1", "yes", "ja", "da", "oui", "si", "sí", "✓", "✔", "y", "wahr"}
FALSE = {"false", "0", "no", "nein", "ne", "non", "✗", "✘", "n", "falsch"}


def show(v) -> str:
    if isinstance(v, float) and v.is_integer() and abs(v) < 1e15:
        return str(int(v))
    if isinstance(v, (dt.date, dt.time)):
        return v.isoformat(sep=" ") if isinstance(v, dt.datetime) else v.isoformat()
    return str(v)


# ------------------------------------------------------------------------------------------------
# the comparison


def compare(src, hh: str, dtype: str, col: dict | None = None) -> tuple[str, str]:
    """One source cell against hedgehog's value (CSV text) in a column of `dtype`."""
    col = col or {}
    hv = hh if hh is not None else ""
    kind = src.kind
    if hv == "":
        if kind == "error":
            return "intentionally_dropped", "excel_error"
        if kind == "text":
            if is_missing_token(src.value):
                return "intentionally_dropped", "missing_token"
            if src.value.strip().lower() in ("none",):
                return "intentionally_dropped", "missing_token"
            return "text_changed", "lost"
        if kind in ("date", "datetime", "time"):
            return "date_changed", "lost"
        return "value_changed", "lost"
    if kind == "error":
        return ("ok", "error_text") if hv.strip().lower() == str(src.value).lower() else ("value_changed", "error_replaced")
    if kind == "bool":
        h = hv.strip().lower()
        if (src.value and h in TRUE) or (not src.value and h in FALSE):
            return "ok", "bool"
        return "value_changed", "bool"
    if kind == "number":
        return _cmp_number(src, hv, dtype)
    if kind in ("date", "datetime", "time"):
        return _cmp_date(src, hv, dtype)
    return _cmp_text(src, hv, dtype, col)


def _cmp_number(src, hv: str, dtype: str):
    v = src.value
    x = parse_hh_number(hv)
    if x is None:
        # a number written into a text column: its text form
        y = parse_hh_number(hv.replace(",", "")) if re.fullmatch(r"-?[\d,]+(\.\d+)?", hv) else None
        if y is not None and num_equal(float(v), y):
            return "ok", "number_as_text"
        d = parse_hh_date(hv)
        if d is not None:
            if float(v) == 0 and hv.strip() == "00:00:00":
                return "ok", "zero_serial"
            from source import serial_to_datetime
            sd = serial_to_datetime(float(v)) if 0 < float(v) < 2958466 else None
            if sd is not None and same_moment(sd if sd.time() != dt.time(0) else sd.date(), d):
                return "ok", "serial_number_to_date"
            return "value_changed", "number_to_date"
        return "value_changed", "number_text_mismatch"
    exact = isinstance(v, int) or float(v).is_integer()
    if num_equal(float(v), x, exact_int=exact):
        if re.search(r"[eE]", hv) and abs(float(v)) >= 1e15:
            return "text_changed", "scientific_notation"
        if "%" in (src.fmt or "") and dtype == "percent":
            return "ok", "percent_fraction"
        return "ok", "number"
    if "%" in (src.fmt or "") and num_equal(float(v) * 100, x):
        return "value_changed", "percent_x100"
    for k in (3, 6, 9, -3, -6, -9, 4, 8, 2, -2):
        if num_equal(float(v) * 10 ** k, x):
            return "value_changed", "scaled"
    if num_equal(round(float(v), 6), x) or (abs(float(v) - x) <= 1e-6 * max(1.0, abs(x))):
        return "value_changed", "rounded"
    return "value_changed", "number_mismatch"


def _cmp_date(src, hv: str, dtype: str):
    if src.raw is not None and src.raw == 0 and hv.strip() in ("0", "00:00:00", ""):
        return "ok", "zero_serial"  # Excel shows 0-Jan-1900 / 00:00 for a date-formatted 0
    d = parse_hh_date(hv)
    if d is None:
        x = parse_hh_number(hv)
        if x is not None and src.raw is not None and num_equal(float(src.raw), x):
            return "date_changed", "serial_left_as_number"
        if x is not None and src.kind == "time":
            return "date_changed", "serial_left_as_number"
        if x is not None:
            return "date_changed", "date_to_number"
        # text form of a date in a text column ("2024-03-31" or the displayed text)
        if _date_text_matches(src, hv):
            return "ok", "date_as_text"
        return "date_changed", "date_text_mismatch"
    if same_moment(src.value, d):
        return "ok", src.kind
    if isinstance(src.value, (dt.date, dt.datetime)) and isinstance(d, (dt.date, dt.datetime)):
        a = src.value.date() if isinstance(src.value, dt.datetime) else src.value
        b = d.date() if isinstance(d, dt.datetime) else d
        if a == b:
            return "date_changed", "time_changed"
        if a.year == b.year and a.month == b.day and a.day == b.month:
            return "date_changed", "day_month_swap"
        return "date_changed", "other_date"
    return "date_changed", "kind_changed"


def _date_text_matches(src, hv: str) -> bool:
    for d, _tag in date_readings(hv)[0]:
        if same_moment(src.value, d):
            return True
    return False


def _cmp_text(src, hv: str, dtype: str, col: dict):
    s = src.value
    if ws(s) == ws(hv):
        return "ok", "text"
    if dtype in ("integer", "number", "currency", "percent"):
        x = parse_hh_number(hv)
        if x is None:
            return "text_changed", "kept_text_differs"
        rd = number_readings(s)
        hits = [tag for v, tag in rd if num_equal(v, x)]
        if not hits:
            if re.fullmatch(r"\d+", ws(s)) and ws(s).startswith("0") and len(ws(s)) > 1:
                return "text_changed", "leading_zero_lost"
            return "value_changed", "text_number_mismatch" if rd else "text_to_number_unverified"
        tag = hits[0]
        if tag.endswith("_x100"):
            return "value_changed", "percent_x100"
        if tag == "text_scale_word_dropped":
            return "value_changed", "scale_word_dropped"
        if len({round(v, 9) for v, _ in rd if not _.endswith("_x100") and _ != "text_scale_word_dropped"}) > 1:
            return "ok", "text_number_ambiguous_grouping"
        return "ok", tag
    if dtype in ("date", "datetime"):
        d = parse_hh_date(hv)
        if d is None:
            return "text_changed", "kept_text_differs"
        rd, note = date_readings(s)
        if note == "no_year":
            return "date_changed", "year_invented"
        if not rd:
            x = parse_hh_number(ws(s))
            if x is not None:
                return "date_changed", "number_text_to_date"
            return "date_changed", "text_to_date_unverified"
        for r, tag in rd:
            if same_moment(r, d) or (isinstance(r, dt.datetime) and isinstance(d, dt.date) and not isinstance(d, dt.datetime) and r.date() == d):
                if len({x for x, _ in rd}) > 1:
                    return "ok", f"text_date_ambiguous:{tag}"
                return "ok", f"text_date:{tag}"
        return "date_changed", "text_date_mismatch"
    if dtype == "bool":
        h = hv.strip().lower()
        if h in TRUE or h in FALSE:
            return "ok", "text_bool"
        return "text_changed", "bool"
    # text column, text differs
    if re.fullmatch(r"\d+", ws(s)) and parse_hh_number(hv) is not None and ws(s) != hv:
        return "text_changed", "id_changed"
    if strip_footnote(ws(s)) == ws(hv) and (col.get("footnote_marks") or strip_footnote(ws(s)) != ws(s)):
        return "ok", "footnote_mark_stripped"
    if norm_loose(s) == norm_loose(hv):
        return "text_changed", "punctuation_or_case"
    if ws(hv) and ws(hv) in ws(s):
        return "text_changed", "truncated_or_cut"
    return "text_changed", "text_mismatch"
