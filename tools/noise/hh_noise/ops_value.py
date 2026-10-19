"""Stage A — value operators. Each works on the data cells of one block, keeps provenance, tags
every touched cell and records value-level truth (`cell.true`) when the written value is text that
stands for a number, date, boolean or missing value."""
from __future__ import annotations

import datetime as dt
import random

from .model import Block, Cell, excel_serial
from .table import is_measure
from .vocab import BOOL_STYLES, MONTHS, MONTHS_ABBR, UNITS

NBSP = " "
NNBSP = " "

# number styles: (thousands sep, decimal sep)
NUM_STYLES = {
    "us": (",", "."),
    "eu": (".", ","),
    "fr": (NBSP, ","),
    "fr_space": (" ", ","),
    "fr_nnbsp": (NNBSP, ","),
    "ch": ("'", "."),
    "plain": ("", "."),
    "plain_comma": ("", ","),
}
LANG_NUM = {"en": "us", "de": "eu", "fr": "fr", "es": "eu", "it": "eu", "nl": "eu", "sr": "eu"}


def decimals_for(values) -> int:
    nums = [v for v in values if isinstance(v, (int, float)) and not isinstance(v, bool)]
    if all(float(v).is_integer() for v in nums):
        return 0
    return 2


def fmt_num(x: float, style: str, decimals: int) -> str:
    th, dec = NUM_STYLES[style]
    neg = x < 0
    s = f"{abs(x):,.{decimals}f}"  # 1,234.50
    ip, _, fp = s.partition(".")
    ip = ip.replace(",", th)
    out = ip + (dec + fp if fp else "")
    return ("-" if neg else "") + out


def parse_back(x: float, decimals: int) -> float:
    return round(float(x), decimals)


def _num_cells(b: Block, c: int):
    return [(r, cell) for r, cell in b.col_cells(c) if isinstance(cell.v, (int, float)) and not isinstance(cell.v, bool)]


def numeric_cols(b: Block, measures_only=False):
    out = []
    for c, ci in enumerate(b.columns):
        if ci.stype in ("int", "float", "money", "count", "percent", "lat", "lon") or (not measures_only and ci.stype == "year"):
            if measures_only and not (is_measure(ci.stype) or ci.stype == "percent"):
                continue
            if _num_cells(b, c):
                out.append(c)
    return out


def text_cols(b: Block):
    return [c for c, ci in enumerate(b.columns) if ci.stype in ("category", "city", "country", "state", "name", "text", "code")]


def date_cols(b: Block):
    return [c for c, ci in enumerate(b.columns) if ci.stype in ("date", "datetime", "month")
            and any(isinstance(cell.v, (dt.date, dt.datetime)) for _, cell in b.col_cells(c))]


def _set_num_text(cell: Cell, text: str, num: float, op: str, extra: dict | None = None):
    cell.v = text
    cell.true = {"num": num, **(extra or {})}
    cell.tag(op)


# ---------------------------------------------------------------- operators


def missing_tokens(b: Block, rng: random.Random, ctx) -> bool:
    cols = [c for c in range(b.n_cols) if list(b.col_cells(c))]
    if not cols:
        return False
    picked = rng.sample(cols, min(len(cols), rng.choice([1, 1, 2, 3])))
    tokens = ["", "", "-", "n/a", "N/A", "–", "..", "x", "?", "NA", "#N/A", "null", "—", "0"]
    done = 0
    for c in picked:
        rate = min(0.5, rng.betavariate(1.2, 10))
        main = rng.choice(tokens)
        for r, cell in list(b.col_cells(c)):
            if rng.random() >= rate:
                continue
            tok = main if rng.random() < 0.8 else rng.choice(tokens)
            if tok == "0" and not isinstance(cell.v, (int, float)):
                tok = "-"
            if tok == "":
                del b.cells[(r, c)]
                b.implied.append({"r": r, "c": c, "src": cell.src, "orig": cell.orig, "ops": cell.ops + ["missing_tokens"], "removed": True})
                done += 1
                continue
            cell.v = 0 if tok == "0" else tok
            cell.error = tok == "#N/A"
            cell.true = {"missing": tok}
            cell.tag("missing_tokens")
            done += 1
    if done:
        b.log("missing_tokens", cols=picked, cells=done)
    return bool(done)


def numbers_as_text(b: Block, rng, ctx) -> bool:
    cols = numeric_cols(b)
    if not cols:
        return False
    picked = rng.sample(cols, max(1, int(len(cols) * rng.uniform(0.2, 1.0))))
    n = 0
    for c in picked:
        frac = 1.0 if rng.random() < 0.6 else rng.uniform(0.1, 0.9)
        for _, cell in _num_cells(b, c):
            if rng.random() < frac:
                v = cell.v
                text = str(int(v)) if float(v).is_integer() else repr(round(float(v), 6))
                if rng.random() < 0.1:
                    text = "'" + text if False else text  # apostrophe prefix is not stored in the cell value
                _set_num_text(cell, text, float(text), "numbers_as_text")
                n += 1
    b.log("numbers_as_text", cols=picked, cells=n)
    return n > 0


def locale_numbers(b: Block, rng, ctx) -> bool:
    cols = [c for c in numeric_cols(b) if b.columns[c].stype not in ("year", "lat", "lon")]
    if not cols:
        return False
    style = ctx.num_style
    picked = rng.sample(cols, max(1, int(len(cols) * rng.uniform(0.3, 1.0))))
    n = 0
    for c in picked:
        cells = _num_cells(b, c)
        dec = decimals_for([x.v for _, x in cells])
        if rng.random() < 0.3:
            dec = rng.choice([0, 1, 2])
        for _, cell in cells:
            x = parse_back(cell.v, dec)
            _set_num_text(cell, fmt_num(x, style, dec), x, "locale_numbers", {"style": style})
            n += 1
    b.log("locale_numbers", style=style, cols=picked, cells=n)
    return n > 0


def currency_units(b: Block, rng, ctx) -> bool:
    cols = [c for c in numeric_cols(b, measures_only=True) if b.columns[c].stype in ("money", "count", "float", "int")]
    if not cols:
        return False
    c = rng.choice(cols)
    st = b.columns[c].stype
    style = ctx.num_style
    if st == "money":
        unit = rng.choice(UNITS["money"] if style == "us" else ["€", "EUR", "RSD", "din.", "CHF", "€"])
    else:
        unit = rng.choice(UNITS["count"] if st in ("count", "int") else UNITS["float"])
    prefix = unit in ("$", "£") or (unit in ("EUR", "USD", "CHF") and rng.random() < 0.3)
    space = "" if unit in ("$", "£") else (NBSP if rng.random() < 0.2 else " ")
    cells = _num_cells(b, c)
    dec = decimals_for([x.v for _, x in cells]) if st != "money" else rng.choice([0, 2, 2])
    n = 0
    for _, cell in cells:
        x = parse_back(cell.v, dec)
        body = fmt_num(abs(x), style, dec)
        s = f"{unit}{space}{body}" if prefix else f"{body}{space}{unit}"
        if x < 0:
            s = "-" + s
        _set_num_text(cell, s, x, "currency_units", {"unit": unit})
        n += 1
    b.log("currency_units", col=c, unit=unit, prefix=prefix, cells=n)
    return n > 0


def percent_text(b: Block, rng, ctx) -> bool:
    cols = [c for c, ci in enumerate(b.columns) if ci.stype == "percent" and _num_cells(b, c)]
    if not cols:
        return False
    c = rng.choice(cols)
    cells = _num_cells(b, c)
    frac = max(abs(float(x.v)) for _, x in cells) <= 1.0
    style = ctx.num_style
    dec = rng.choice([0, 1, 1, 2])
    sp = rng.choice(["", "", " ", NBSP]) if style != "us" else ""
    for _, cell in cells:
        pct = float(cell.v) * (100 if frac else 1)
        pct = round(pct, dec)
        _set_num_text(cell, fmt_num(pct, style, dec) + sp + "%", pct / 100.0, "percent_text")
    b.log("percent_text", col=c, fraction_source=frac)
    return True


def negative_formats(b: Block, rng, ctx) -> bool:
    cols = [c for c in numeric_cols(b, measures_only=True) if any(float(x.v) < 0 for _, x in _num_cells(b, c))]
    if not cols:
        return False
    c = rng.choice(cols)
    kind = rng.choice(["parens", "trailing", "unicode"])
    cells = _num_cells(b, c)
    dec = decimals_for([x.v for _, x in cells])
    n = 0
    for _, cell in cells:
        x = parse_back(cell.v, dec)
        body = fmt_num(abs(x), ctx.num_style, dec)
        if x < 0:
            s = {"parens": f"({body})", "trailing": f"{body}-", "unicode": f"−{body}"}[kind]
        else:
            s = body
        _set_num_text(cell, s, x, "negative_formats", {"neg": kind})
        n += 1
    b.log("negative_formats", col=c, kind=kind)
    return n > 0


def date_text(d, fmt: str, lang: str) -> str:
    if isinstance(d, dt.datetime):
        d = d.date()
    m, dd, y = d.month, d.day, d.year
    M, Ma = MONTHS[lang][m - 1], MONTHS_ABBR[lang][m - 1]
    return {
        "iso": f"{y:04d}-{m:02d}-{dd:02d}",
        "us": f"{m}/{dd}/{y}",
        "us2": f"{m:02d}/{dd:02d}/{y % 100:02d}",
        "eu_dot": f"{dd:02d}.{m:02d}.{y}",
        "sr_dot": f"{dd:02d}.{m:02d}.{y}.",
        "eu_slash": f"{dd:02d}/{m:02d}/{y}",
        "eu_dash": f"{dd:02d}-{m:02d}-{y}",
        "d_mon_yy": f"{dd}-{MONTHS_ABBR['en'][m - 1]}-{y % 100:02d}",
        "mon_d_y": f"{MONTHS['en'][m - 1]} {dd}, {y}",
        "d_month_y": f"{dd} {M} {y}",
        "de_long": f"{dd}. {M} {y}",
        "es_long": f"{dd} de {M} de {y}",
        "abbr": f"{dd} {Ma} {y}",
        "ymd_slash": f"{y}/{m:02d}/{dd:02d}",
    }[fmt]


DATE_FMTS = {
    "en": ["iso", "us", "us2", "d_mon_yy", "mon_d_y", "d_month_y", "eu_slash"],
    "de": ["eu_dot", "de_long", "iso", "abbr"],
    "fr": ["eu_slash", "d_month_y", "iso", "abbr"],
    "es": ["eu_slash", "es_long", "iso", "abbr"],
    "it": ["eu_slash", "d_month_y", "iso", "abbr"],
    "nl": ["eu_dash", "d_month_y", "iso", "abbr"],
    "sr": ["sr_dot", "eu_dot", "d_month_y", "iso"],
}


def dates_as_text(b: Block, rng, ctx) -> bool:
    cols = date_cols(b)
    if not cols:
        return False
    c = rng.choice(cols)
    lang = b.lang if rng.random() < 0.8 else rng.choice(list(DATE_FMTS))
    fmt = rng.choice(DATE_FMTS[lang])
    mixed = rng.random() < 0.15
    n = 0
    for _, cell in b.col_cells(c):
        if not isinstance(cell.v, (dt.date, dt.datetime)):
            continue
        f = rng.choice(DATE_FMTS[lang]) if mixed and rng.random() < 0.3 else fmt
        d = cell.v
        cell.v = date_text(d, f, lang)
        dd = d.date() if isinstance(d, dt.datetime) else d
        cell.true = {"date": dd.isoformat(), "serial": excel_serial(dd), "fmt": f, "lang": lang}
        if f in ("us", "us2", "eu_slash", "eu_dash") and dd.day <= 12 and dd.day != dd.month:
            cell.true["ambiguous"] = True  # dd/mm vs mm/dd only decidable from the rest of the column
        cell.tag("dates_as_text")
        n += 1
    b.log("dates_as_text", col=c, fmt=fmt, lang=lang, mixed=mixed, cells=n)
    return n > 0


def date_serials(b: Block, rng, ctx) -> bool:
    cols = date_cols(b)
    if not cols:
        return False
    c = rng.choice(cols)
    for _, cell in b.col_cells(c):
        if isinstance(cell.v, (dt.date, dt.datetime)):
            d = cell.v.date() if isinstance(cell.v, dt.datetime) else cell.v
            cell.true = {"date": d.isoformat(), "serial": excel_serial(d)}
            cell.v = excel_serial(d)
            cell.tag("date_serials")
    b.log("date_serials", col=c)
    return True


def whitespace_noise(b: Block, rng, ctx) -> bool:
    cands = [(r, c, cell) for (r, c), cell in b.cells.items() if cell.role in ("data", "header") and isinstance(cell.v, str) and cell.v.strip()]
    if not cands:
        return False
    rate = rng.uniform(0.03, 0.3)
    n = 0
    for _, _, cell in cands:
        if rng.random() >= rate:
            continue
        k = rng.random()
        v = cell.v
        if k < 0.35:
            v = v + rng.choice([" ", "  ", NBSP, "\t"])
        elif k < 0.6:
            v = rng.choice([" ", "  ", NBSP]) + v
        elif k < 0.8 and " " in v.strip():
            v = v.replace(" ", rng.choice(["  ", NBSP]), 1)
        else:
            v = " " + v + " "
        cell.v = v
        cell.tag("whitespace_noise")
        n += 1
    b.log("whitespace_noise", cells=n)
    return n > 0


def case_noise(b: Block, rng, ctx) -> bool:
    cols = [c for c in text_cols(b) if b.columns[c].stype != "text"]
    if not cols:
        return False
    c = rng.choice(cols)
    rate = rng.uniform(0.05, 0.4)
    n = 0
    for _, cell in b.col_cells(c):
        if isinstance(cell.v, str) and "missing" not in (cell.true or {}) and rng.random() < rate:
            f = rng.choice([str.upper, str.lower, str.title])
            nv = f(cell.v)
            if nv != cell.v:
                cell.true = {**(cell.true or {}), "canon": cell.orig}
                cell.v = nv
                cell.tag("case_noise")
                n += 1
    b.log("case_noise", col=c, cells=n)
    return n > 0


def alias(v: str, rng) -> str:
    words = v.split()
    if len(words) >= 2:
        k = rng.random()
        if k < 0.5:
            return "".join(w[0] for w in words if w[:1].isalpha()).upper()
        if k < 0.8:
            return words[0][0] + ". " + " ".join(words[1:])
        return " ".join(words[:-1]) + " " + words[-1][:3] + "."
    if len(v) > 5:
        return v[: rng.randint(3, 4)] + "."
    return v.upper()


def categorical_variants(b: Block, rng, ctx) -> bool:
    cols = [c for c in text_cols(b) if b.columns[c].stype in ("category", "city", "country", "state", "name")]
    if not cols:
        return False
    c = rng.choice(cols)
    rate = rng.uniform(0.05, 0.3)
    fixed: dict[str, str] = {}
    n = 0
    for _, cell in b.col_cells(c):
        if isinstance(cell.v, str) and isinstance(cell.orig, str) and "missing" not in (cell.true or {}) and rng.random() < rate:
            a = fixed.setdefault(cell.orig, alias(cell.orig, rng))
            if a != cell.v:
                cell.v = a
                cell.true = {**(cell.true or {}), "canon": cell.orig}
                cell.tag("categorical_variants")
                n += 1
    b.log("categorical_variants", col=c, cells=n)
    return n > 0


KEYB = "qwertyuiop asdfghjkl zxcvbnm"


def typo(s: str, rng) -> str:
    if len(s) < 3:
        return s
    i = rng.randrange(1, len(s) - 1)
    k = rng.random()
    if k < 0.3:
        return s[:i] + s[i + 1] + s[i] + s[i + 2:]
    if k < 0.55:
        return s[:i] + s[i + 1:]
    if k < 0.75:
        return s[:i] + s[i] + s[i:]
    ch = s[i].lower()
    j = KEYB.find(ch)
    if j < 0:
        return s[:i] + s[i + 1:]
    nb = KEYB[max(0, j - 1)] if rng.random() < 0.5 else KEYB[min(len(KEYB) - 1, j + 1)]
    nb = nb if nb != " " else ch
    return s[:i] + (nb.upper() if s[i].isupper() else nb) + s[i + 1:]


def typos(b: Block, rng, ctx) -> bool:
    cols = text_cols(b)
    if not cols:
        return False
    c = rng.choice(cols)
    rate = rng.uniform(0.02, 0.1)
    n = 0
    for _, cell in b.col_cells(c):
        if isinstance(cell.v, str) and "missing" not in (cell.true or {}) and rng.random() < rate:
            nv = typo(cell.v, rng)
            if nv != cell.v:
                cell.v = nv
                cell.true = {**(cell.true or {}), "canon": cell.orig}
                cell.tag("typos")
                n += 1
    b.log("typos", col=c, cells=n)
    return n > 0


def scale_thousands(b: Block, rng, ctx) -> bool:
    cols = [c for c, ci in enumerate(b.columns) if ci.stype in ("money", "count") and _num_cells(b, c)]
    if not cols or any(isinstance(x.v, str) for c in cols for _, x in b.col_cells(c)):
        return False
    if max(abs(float(x.v)) for c in cols for _, x in _num_cells(b, c)) < 10_000:
        return False
    dec = rng.choice([0, 0, 1])
    for c in cols:
        for _, cell in _num_cells(b, c):
            v = round(float(cell.v) / 1000.0, dec)
            cell.v = int(v) if dec == 0 else v
            cell.true = {"num": cell.v, "scale": 1000}
            cell.tag("scale_thousands")
    b.scale = 1000
    b.log("scale_thousands", cols=cols, decimals=dec)
    return True


def value_drift(b: Block, rng, ctx) -> bool:
    """A value sits in the neighbouring column (the user's example: a city in E instead of D)."""
    if b.n_cols < 2:
        return False
    rows = b.data_rows()
    n = 0
    for _ in range(rng.choice([1, 1, 2, 3])):
        r = rng.choice(rows)
        filled = [c for c in range(b.n_cols) if (r, c) in b.cells and b.cells[(r, c)].role == "data"]
        if not filled:
            continue
        c = rng.choice(filled)
        d = rng.choice([1, -1]) if 0 < c < b.n_cols - 1 else (1 if c == 0 else -1)
        t = c + d
        if any(m[0] <= r <= m[2] and (m[1] <= c <= m[3] or m[1] <= t <= m[3]) for m in b.merges):
            continue
        if b.columns[t].stype == "spacer" if t < len(b.columns) else True:
            continue
        cell = b.cells.pop((r, c))
        cell.tag("value_drift")
        tc = (cell.true or {}).get("true_col", c)
        cell.true = {**(cell.true or {}), "true_col": tc, "drift": t - tc}
        if (r, t) in b.cells:  # swap with the neighbour
            other = b.cells.pop((r, t))
            other.tag("value_drift")
            to = (other.true or {}).get("true_col", t)
            other.true = {**(other.true or {}), "true_col": to, "drift": c - to}
            b.cells[(r, c)] = other
        b.cells[(r, t)] = cell
        n += 1
    b.log("value_drift", moves=n)
    return n > 0


FOOT = ["*", "¹", " (a)", "(1)", "**", "†", " e", " p"]


def footnote_markers(b: Block, rng, ctx) -> bool:
    cands = [cell for cell in b.cells.values() if cell.role == "data" and isinstance(cell.v, (int, float, str))
             and not isinstance(cell.v, bool) and not cell.error and "missing" not in (cell.true or {})
             and "date" not in (cell.true or {}) and "bool" not in (cell.true or {})]
    if not cands:
        return False
    mark = rng.choice(FOOT)
    n = 0
    for cell in rng.sample(cands, min(len(cands), rng.randint(1, 3))):
        if isinstance(cell.v, (int, float)):
            num = float(cell.v)
            cell.v = (str(int(num)) if num.is_integer() else str(round(num, 4))) + mark
            cell.true = {"num": num, "footnote": mark.strip()}
        elif "num" in (cell.true or {}):
            cell.v = cell.v + mark
            cell.true["footnote"] = mark.strip()
        else:
            cell.v = cell.v + mark
            cell.true = {**(cell.true or {}), "footnote": mark.strip()}
        cell.tag("footnote_markers")
        n += 1
    b.stats["footnote"] = mark.strip()
    b.log("footnote_markers", marker=mark.strip(), cells=n)
    return n > 0


def excel_errors(b: Block, rng, ctx) -> bool:
    cands = [cell for cell in b.cells.values() if cell.role == "data" and isinstance(cell.v, (int, float)) and not isinstance(cell.v, bool)]
    if not cands:
        return False
    for cell in rng.sample(cands, min(len(cands), rng.randint(1, 2))):
        tok = rng.choice(["#N/A", "#REF!", "#DIV/0!", "#VALUE!"])
        cell.v, cell.error = tok, True
        cell.true = {"missing": tok}
        cell.tag("excel_errors")
    b.log("excel_errors")
    return True


def bool_variants(b: Block, rng, ctx) -> bool:
    cols = [c for c, ci in enumerate(b.columns) if ci.stype == "bool"]
    if not cols:
        return False
    c = rng.choice(cols)
    yes, no = rng.choice(BOOL_STYLES)
    for (r, cc), cell in list(b.cells.items()):
        if cc == c and cell.role == "data" and isinstance(cell.v, bool):
            val = cell.v
            s = yes if val else no
            if s == "":
                del b.cells[(r, cc)]
                b.implied.append({"r": r, "c": cc, "src": cell.src, "orig": cell.orig, "ops": cell.ops + ["bool_variants"], "removed": True})
                continue
            cell.v = s
            cell.true = {"bool": val}
            cell.tag("bool_variants")
    b.log("bool_variants", col=c, style=[yes, no])
    return True


def header_rename(b: Block, rng, ctx) -> bool:
    n = 0
    for (r, c), cell in b.cells.items():
        if cell.role != "header" or r != b.header_rows - 1 or not isinstance(cell.v, str) or rng.random() > 0.5:
            continue
        v, st = cell.v, b.columns[c].stype if c < len(b.columns) else ""
        k = rng.random()
        if k < 0.25:
            nv = v.replace("_", " ").replace(".", " ").strip().title()
        elif k < 0.4:
            nv = v.upper()
        elif k < 0.6 and st in ("money", "count", "float", "percent"):
            unit = {"money": rng.choice(["(USD)", "($)", "[EUR]", "(000 RSD)", "in €"]), "count": "(#)", "float": rng.choice(["[kg]", "(t)", "(km)"]), "percent": "(%)"}[st]
            nv = f"{v} {unit}"
        elif k < 0.7:
            nv = v.replace("Number", "No.").replace("number", "no.").replace("Total", "Tot.")
            nv = nv if nv != v else (v[:4] + "." if len(v) > 6 else v)
        elif k < 0.8:
            nv = v + rng.choice(["*", " (1)", "¹"])
        else:
            nv = v.lower().replace(" ", "_")
        if nv != v:
            cell.v = nv
            cell.tag("header_rename")
            n += 1
    b.log("header_rename", cells=n)
    return n > 0


VALUE_OPS = {
    "missing_tokens": (missing_tokens, "replace values with missing tokens ('', '-', 'n/a', '..', '#N/A' …), per column rate ~Beta(1.2,10)"),
    "numbers_as_text": (numbers_as_text, "numbers stored as text cells ('1234.5')"),
    "locale_numbers": (locale_numbers, "numbers as text in US/EU/FR/CH formats: 1,234.50 / 1.234,50 / 1 234,50 / 1'234.50"),
    "currency_units": (currency_units, "currency or unit glued to the number: $1,200 / 1.200 € / 1,200 USD / 12 kg"),
    "percent_text": (percent_text, "percent column as text '12.5%' / '12,5 %'"),
    "negative_formats": (negative_formats, "negatives as (1,234) / 1234- / U+2212"),
    "dates_as_text": (dates_as_text, "dates as text in many formats/locales incl. month names (en/de/fr/es/it/nl/sr)"),
    "date_serials": (date_serials, "dates written as bare Excel serial numbers without a date format"),
    "whitespace_noise": (whitespace_noise, "leading/trailing spaces, NBSP, tabs, doubled inner spaces"),
    "case_noise": (case_noise, "UPPER/lower/Title case variants of categorical values"),
    "categorical_variants": (categorical_variants, "aliases/abbreviations of categorical values (New York -> NY / N. York)"),
    "typos": (typos, "keyboard typos (swap/drop/double/neighbour key)"),
    "scale_thousands": (scale_thousands, "money/count columns shown in thousands; title/header says so"),
    "value_drift": (value_drift, "a value shifted into the neighbouring column (swapped if occupied)"),
    "footnote_markers": (footnote_markers, "footnote marks glued to values ('1234*', '567 (a)') + matching note line"),
    "excel_errors": (excel_errors, "Excel error values (#N/A, #REF!, #DIV/0!) in data cells"),
    "bool_variants": (bool_variants, "booleans as Yes/No, Y/N, 1/0, ✓/blank, Da/Ne …"),
    "header_rename": (header_rename, "header text variants: case, underscores, units '(USD)', abbreviations, markers"),
}
