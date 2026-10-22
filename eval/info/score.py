"""Model-free information scoreboard: does hedgehog's catalog hand an agent what each check needs?

    python3 -I eval/info/score.py                       # all cases: run hedgehog mcp, score, write REPORT.md + results.json
    python3 -I eval/info/score.py --cases at-cpi,nw-orders
    python3 -I eval/info/score.py --offline             # re-score from the session cache (no hedgehog calls)
    python3 -I eval/info/score.py --question            # also open each file with the case's prompt as the
                                                        # question (B150): QUESTION.md + results-question.json

Run it under scripts/run_capped.sh (pt-contracts needs 4G). For every check of every case in
eval/agents/cases.json (judge checks are out of scope) it decides, without an LLM, one of:

  a  in the catalog: the reference value (or phrase) is in `open_file`'s text, attributable to it;
  b  one query away: the check's reference SQL names only tables, columns and literal values that
     the catalog shows (or that are discoverable, see `literal_ok`), and running it reproduces the value;
  c  reachable with more work: the data is in the tables, but the query needs names the catalog does
     not show (describe_table, a lookup query first, several queries or cell reads);
  d  wrong or missing: the reference query fails or does not reproduce the value, the catalog's data
     view gives a different answer (rows it calls totals are data), or the value is in no table.

It also counts catalog tokens (tiktoken o200k_base when importable, else characters / 4) and, for
the ablation, classifies every catalog line by kind and re-scores with each kind removed. One
`hedgehog mcp` session per case (eval/agents/sql.py's client); a second one only for cases whose
checks reference source cells. README.md in this folder describes the method.
"""

from __future__ import annotations

import argparse
import csv
import io
import json
import os
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
AGENTS = HERE.parent / "agents"
sys.path.insert(0, str(AGENTS))
import sql as S  # noqa: E402  (the JSON-RPC client for `hedgehog mcp`)
from check_cases import case_files, cell_values, data_root, first_value  # noqa: E402
from run import numbers  # noqa: E402  (the answer checks' number reader)

CATS = ("a", "b", "c", "d")
CAT_NAMES = {"a": "in catalog", "b": "one query away", "c": "more work", "d": "wrong or missing"}

# ---------------------------------------------------------------------------------------------
# hand-written probes: natural SQL for checks whose reference is not SQL over hedgehog's tables
# (ref_py over the raw CSV, text_any answers) and, under "alt", a natural alternative to a
# reference that filters on a code the question does not give. A probe is scored like a ref_sql.
# `pair`: the num check whose value this text answer belongs to (an argmax); `same_as`: a word
# answer derived from another check.

PROBES: dict[str, dict[str, dict]] = {
    "uk-gender-pay-gap": {
        "employers": {"sql": "SELECT COUNT(*) FROM T1"},
        "median": {"sql": "SELECT AVG(v) FROM (SELECT DiffMedianHourlyPercent v FROM T1 ORDER BY v "
                          "LIMIT 2 - (SELECT COUNT(*) FROM T1) % 2 OFFSET ((SELECT COUNT(*) FROM T1) - 1) / 2)"},
        "men-share": {"sql": "SELECT 100.0 * SUM(DiffMedianHourlyPercent > 0) / COUNT(*) FROM T1"},
        "band-500": {"sql": "SELECT AVG(DiffMedianHourlyPercent) FROM T1 WHERE EmployerSize = '500 to 999'"},
        "late": {"sql": "SELECT COUNT(*) FROM T1 WHERE SubmittedAfterTheDeadline = 1"},
    },
    "enron-henry-hub": {
        "jan-day": {"pair": "jan-max", "sql": "SELECT period FROM T3_long_all WHERE col_B = 'Receipts HH' "
                                              "AND _row_type = 'total' ORDER BY value DESC LIMIT 1"},
    },
    "de-truck-toll": {
        "max-date": {"pair": "max-2023", "sql": "SELECT Datum FROM T3 WHERE Datum LIKE '2023%' ORDER BY unbereinigt DESC LIMIT 1"},
    },
    "enron-mdea-pl": {  # the day-ahead blocks' total rows, all days in the union view U1
        "epmi": {"sql": "SELECT SUM(\"Net Profit/Savings Summary / EPMI / (Profit)\") FROM U1_all WHERE _row_type = 'total' AND variant LIKE 'Day-Ahead%'"},
        "mdea": {"sql": "SELECT SUM(\"Net Profit/Savings Summary / MDEA / (Savings)\") FROM U1_all WHERE _row_type = 'total' AND variant LIKE 'Day-Ahead%'"},
        "best-value": {"sql": "SELECT MAX(\"Net Profit/Savings Summary / MDEA / (Savings)\") FROM U1_all WHERE _row_type = 'total' AND variant LIKE 'Day-Ahead%'"},
        "best-day": {"pair": "best-value", "sql": "SELECT sheet FROM U1_all WHERE _row_type = 'total' AND variant LIKE 'Day-Ahead%' ORDER BY \"Net Profit/Savings Summary / MDEA / (Savings)\" DESC LIMIT 1"},
    },
    "enron-storage-inventory": {
        "peak-month": {"pair": "peak", "sql": "SELECT period FROM T2_long_all WHERE \"LDC / Virtual Storage / Description\" "
                                              "= 'Total Volumes' ORDER BY value DESC LIMIT 1"},
    },
    "at-salzburg-budget": {"surplus-word": {"same_as": "surplus"}},
    "wb-gdp-wide": {  # the references filter on ISO codes; the question names the countries
        "srb-2010": {"alt": {"sql": "SELECT \"2010\" FROM T1 WHERE \"Country Name\" = 'Serbia'"}},
        "srb-2024": {"alt": {"sql": "SELECT \"2024\" FROM T1 WHERE \"Country Name\" = 'Serbia'"}},
        "srb-growth": {"alt": {"sql": "SELECT 100.0 * \"2024\" / \"2010\" - 100 FROM T1 WHERE \"Country Name\" = 'Serbia'"}},
        "pol-eu": {"alt": {"sql": "SELECT 100.0 * (SELECT \"2024\" FROM T1 WHERE \"Country Name\" = 'Poland') / \"2024\" "
                                  "FROM T1_all WHERE \"Country Name\" = 'European Union'"}},
        "somalia": {"alt": {"sql": "SELECT \"2024\" FROM T1 WHERE \"Country Name\" = 'Somalia'"}},
    },
    "fr-insee-pop": {  # the references filter on département codes; the question names them
        "paris-2023": {"alt": {"sql": "SELECT \"Ensemble / Total\" FROM T1 WHERE \"Départements (2)\" = 'Paris'"}},
        "paris-1990": {"alt": {"sql": "SELECT \"Ensemble / Total\" FROM T34 WHERE \"Départements (2)\" = 'Paris'"}},
        "paris-change": {"alt": {"sql": "SELECT 100.0 * (SELECT \"Ensemble / Total\" FROM T1 WHERE \"Départements (2)\" = 'Paris') "
                                        "/ \"Ensemble / Total\" - 100 FROM T34 WHERE \"Départements (2)\" = 'Paris'"}},
        "somme": {"alt": {"sql": "SELECT \"Ensemble / Total\" FROM T1 WHERE \"Départements (2)\" = 'Somme'"}},
    },
    "nw-orders": {  # the references list the 1997 order IDs; the natural query joins the two files
        "revenue-1997": {"alt": {"file": "northwind/order_details.csv", "sql":
                         "SELECT SUM(b.unitPrice * b.quantity * (1 - b.discount)) FROM f1.T1 AS a JOIN f2.T1 AS b "
                         "ON a.orderID = b.orderID WHERE a.orderDate LIKE '1997%'"}},
        "employee-4": {"alt": {"file": "northwind/order_details.csv", "sql":
                       "SELECT v FROM (SELECT a.employeeID, SUM(b.unitPrice * b.quantity * (1 - b.discount)) AS v "
                       "FROM f1.T1 AS a JOIN f2.T1 AS b ON a.orderID = b.orderID WHERE a.orderDate LIKE '1997%' "
                       "GROUP BY a.employeeID ORDER BY v DESC LIMIT 1)"}},
    },
    "us-chicago-salaries": {
        "top-name": {"pair": "top-salary", "sql": "SELECT name FROM T1 ORDER BY annual_salary DESC LIMIT 1"},
    },
    "at-cpi": {
        "peak-month": {"pair": "peak", "sql": "SELECT \"Jahr / Monat_period\" FROM T1 ORDER BY \"% zu Vorjahr\" DESC LIMIT 1"},
    },
    "uk-beis-spending": {
        "top-day": {"pair": "top-day-amount", "sql": "SELECT \"Date of Payment\" FROM T1 GROUP BY 1 ORDER BY SUM(Amount) DESC LIMIT 1"},
    },
    "uk-ons-labour": {  # "the latest period in Table 1"
        "period": {"sql": "SELECT col_A FROM T2 ORDER BY period DESC LIMIT 1"},
        "employment-rate": {"sql": "SELECT \"All aged 16 to 64 / Employment / rate (%) / LF24\" FROM T2 ORDER BY period DESC LIMIT 1"},
        "unemployment-rate": {"sql": "SELECT \"All aged 16 & over / Unemployment / rate (%) / MGSX\" FROM T2 ORDER BY period DESC LIMIT 1"},
        "employed": {"sql": "SELECT \"All aged 16 & over / Total in employment / level / MGRZ\" FROM T2 ORDER BY period DESC LIMIT 1"},
        "unemployed": {"sql": "SELECT \"All aged 16 & over / Unemployed / level / MGSC\" FROM T2 ORDER BY period DESC LIMIT 1"},
    },
    "pt-social-report": {  # totals by label: "Relação Jurídica" = 'Total', Sexo = 'T' (all) / 'F' (women)
        "workers": {"sql": "SELECT TOTAL FROM T1_all WHERE \"Relação Jurídica\" = 'Total' AND Sexo = 'T'"},
        "women": {"sql": "SELECT TOTAL FROM T1_all WHERE \"Relação Jurídica\" = 'Total' AND Sexo = 'F'"},
        "women-share": {"sql": "SELECT 100.0 * (SELECT TOTAL FROM T1_all WHERE \"Relação Jurídica\" = 'Total' AND Sexo = 'F') "
                               "/ TOTAL FROM T1_all WHERE \"Relação Jurídica\" = 'Total' AND Sexo = 'T'"},
        "tecnico": {"sql": "SELECT \"Técnico Superior\" FROM T1_all WHERE \"Relação Jurídica\" = 'Total' AND Sexo = 'T'"},
        "costs": {"sql": "SELECT \"Valor / (Euros)\" FROM T15 WHERE \"Encargos com Pessoal\" = 'TOTAL'"},
        "base-share": {"sql": "SELECT 100.0 * \"Valor / (Euros)\" / (SELECT \"Valor / (Euros)\" FROM T15 WHERE "
                              "\"Encargos com Pessoal\" = 'TOTAL') FROM T15 WHERE \"Encargos com Pessoal\" = 'Remuneração base'"},
        "providers": {"sql": "SELECT Total FROM T2_all WHERE col_A = 'Total' AND Sexo = 'T'"},
    },
}

# ---------------------------------------------------------------------------------------------
# catalog lines by kind

KINDS = ("file", "time", "sheets", "question", "brief", "table", "columns", "units", "column_notes", "entity", "share",
         "values", "mixed", "long", "no_sum", "table_status", "total_rows", "signs", "inline", "numbers",
         "quality", "formulas", "totals", "notes", "warnings", "sql", "sql_long", "union", "files", "cut",
         "timing", "other")

_TOP = [("FILE ", "file"), ("TIME ", "time"), ("SHEETS without", "sheets"), ("(tables listed briefly", "brief"),
        ("QUESTION ", "question"), ("(other tables, one line each", "brief"), ("(the other tables)", "brief"),
        ("FORMULAS ", "formulas"), ("TOTALS ", "totals"), ("NOTES ", "notes"), ("WARNINGS ", "warnings"),
        ("SQL view ", "sql"), ("SQL views ", "sql"), ("no tables detected", "sql"), ("SQL long view", "sql_long"),
        ("SQL union view", "union"), ("FILES ", "files"), ("JOIN KEYS", "files"), ("… cut:", "cut"),
        ("(normalized in", "timing"), ("(cached)", "timing")]
_IND = [("columns (", "columns"), ("units:", "units"), ("column notes:", "column_notes"), ("share of the total", "share"),
        ("rows mix measures", "mixed"), ("do not sum:", "no_sum"), ("total rows:", "total_rows"), ("signs:", "signs"),
        ("numbers:", "numbers"), ("quality:", "quality"), ("group column:", "table_status"), ("totals:", "table_status"),
        ("warnings:", "table_status"), ("header: none", "table_status"), ("total rows that stay", "total_rows")]


def line_kinds(text: str) -> list[str]:
    """The kind of every line of a catalog (the producers are agent/src/catalog.rs, facts.rs, …)."""
    kinds, inline = [], False
    for line in text.split("\n"):
        s = line.lstrip()
        if not s:
            kinds.append("other")
            continue
        if line == s:  # top level
            inline = False
            k = next((k for p, k in _TOP if s.startswith(p)), None)
            if k is None and re.match(r"T\d+(?:\.\.T\d+)?\s", s):
                k = "table"
            kinds.append(k or "other")
            continue
        k = next((k for p, k in _IND if s.startswith(p)), None)
        if k is None:
            if re.match(r"(all|first) [\d,]+ rows?\b.*:$", s):
                k, inline = "inline", True
            elif re.match(r"T\d+_long:", s) or " is the long form of " in s:
                k = "long"
            elif re.match(r'("[^"]*"|[^\s"]+) values: ', s):
                k = "values"
            elif " — different measures" in s:
                k = "numbers"
            elif re.match(r'("[^"]*"|[^\s"]+) = "<id>', s) or " is the ID of " in s:
                k = "entity"
            elif inline:
                k = "inline"
        if k and k != "inline":
            inline = False
        kinds.append(k or "other")
    return kinds


# ---------------------------------------------------------------------------------------------
# traps: warnings that prevent a wrong answer (the ablation cannot see them, see README)

# Per case: (what, regex) pairs the catalog must match. at-tourism (B105): "Österreich gesamt" is the
# group of guests from Austria, not Austria's total; the grand total is "Insgesamt".
TRAP_CHECKS: dict[str, list[tuple[str, str]]] = {
    "at-tourism-2months": [
        ("the grand total is named first, as such", r"grand total 'Insgesamt'"),
        ("'Österreich gesamt' is flagged as the total of one group, not of the table",
         r"'Österreich gesamt'[^\n]*total one group[^\n]*not the table"),
    ],
}

# Naive-query traps (B160–B162): for a fact, the query an agent writes first, and what keeps it
# from a wrong answer. A trap passes when that query gives the right value on hedgehog's views, or
# when the catalog warns about it (`warn`, a regex; None: only the right value counts). They run in
# the case's session (`naive:<k>`) and are listed with the trap checks.
NAIVE_TRAPS: dict[str, list[dict]] = {
    "pt-social-report": [
        {"what": "workers: SUM over T1_all's total rows adds Sexo M + F + T (twice the workers)",
         "sql": "SELECT SUM(TOTAL) FROM T1_all WHERE _row_type = 'total'", "want": 7478,
         "warn": r"(?m)^T1 [^\n]*\bT'? = M \+ F"},
        {"what": "service providers: SUM over T2_all's total rows (M + F + T)",
         "sql": "SELECT SUM(Total) FROM T2_all WHERE _row_type = 'total'", "want": 560,
         "warn": r"(?m)^T2 [^\n]*\bT'? = M \+ F"},
        {"what": "workers: SUM over the data view T1 is right", "sql": "SELECT SUM(TOTAL) FROM T1", "want": 7478, "warn": None},
    ],
    "pt-tax-benefits": [
        {"what": "a column name copied in the catalog's grouped form a / {b} works in SQL",
         "sql": "SELECT COUNT(*) FROM T7 WHERE \"RESTITUIÇÃO DO IMPOSTO / {Associações e Corpos Bombeiros - DL n.º 84/2017, "
                "de 21/07}\" > 0", "want": 414, "warn": None},
    ],
    "uk-ons-labour": [
        {"what": "previous non-overlapping 3-month period: the row before the latest overlaps it (rolling windows)",
         "sql": "SELECT col_A FROM T2 ORDER BY period DESC LIMIT 1 OFFSET 1", "want": "Feb-Apr 2026",
         "warn": r"periods: rolling 3-month windows[^\n]*non-overlapping[^\n]*'Feb-Apr 2026'"},
    ],
    "enron-henry-hub": [
        {"what": "January day with the highest receipts: SUM over T3_long_all counts the total rows too",
         "sql": "SELECT SUM(value) AS v FROM T3_long_all WHERE col_B = 'Receipts HH' GROUP BY period ORDER BY v DESC LIMIT 1",
         "want": 606356, "warn": r"sum T3_long; T3_long_all adds the total rows"},
    ],
}


def naive_checks(case: str, text: str, session: dict[str, str]) -> list[dict]:
    """The case's naive-query traps: ok when the naive query is right or the catalog warns."""
    out = []
    for k, t in enumerate(NAIVE_TRAPS.get(case, [])):
        v = result_value(session.get(f"naive:{k}", ""))
        if isinstance(t["want"], str):
            right = isinstance(v, str) and v.strip() == t["want"]
        else:
            right = isinstance(v, float) and abs(v - t["want"]) <= 1e-6 * max(1.0, abs(t["want"]))
        warned = bool(t["warn"]) and re.search(t["warn"], text) is not None
        how = "the naive query is right" if right else "the catalog warns" if warned else f"wrong: the naive query gives {v!r}"
        out.append({"what": "naive query: " + t["what"], "ok": right or warned, "how": how})
    return out


_TRAP_WORDS = (("do not sum:", "nosum"), ("signs:", "signs"), ("rows mix measures", "mixed"),
               ("different measures", "measures"), ("units:", "units"), ("amounts in ", "units"),
               ("COUNT(DISTINCT", "distinct"), ("counts by this date leave them out", "datecount"),
               ("duplicate", "duplicates"), ("without a saved value", "formulas"), ("filter = 1", "bool"))


def trap_facts(text: str) -> list[str]:
    """The trap warnings of a catalog as `<kind>:<table>[:<detail>]` facts, so that two catalogs can
    be compared: do-not-sum columns, total rows that stay in the data (exclude before summing),
    grand totals, signs, mixed measures, different look-alike measures, scales, COUNT(DISTINCT …)
    keys, empty dates, duplicates, formulas without values, boolean filters. A table line for a run
    (`T2..T10 …`) stands for every table of the run."""
    out: set[str] = set()
    tables: list[str] = []
    for line, kind in zip(text.split("\n"), line_kinds(text)):
        s = line.strip()
        if kind == "table":
            m = re.match(r"T(\d+)(?:\.\.T(\d+))?\s", s)
            tables = [f"T{k}" for k in range(int(m.group(1)), int(m.group(2) or m.group(1)) + 1)]
        where = tables or ["-"]
        for t in where:
            if re.search(r"\bstays? in (?:T\d+|each)\b", s):
                out.add(f"kept:{t}")
            for g in re.findall(r"grand total '([^']*)'", s):
                out.add(f"grand:{t}:{g}")
            for w, k in _TRAP_WORDS:
                if w in s:
                    out.add(f"{k}:{t}")
        if kind == "no_sum":
            for name in re.findall(r'(?:: |, )("[^"]*"|[^\s,(]+) \(', s):
                out.add(f"nosum-col:{name.strip(chr(34))}")
        if kind in ("columns", "table"):
            # brace groups written out, each member with its own kind: "a / {b[rate], c}" → "a / b"[rate]
            s = re.sub(r'"([^"]*\{[^"]*)"', lambda m: " ".join(
                re.sub(r"^(.*?)(\[[^\]]*\])?$", lambda x: f'"{x.group(1)}"{x.group(2) or ""}', e)
                for e in expand_braces(m.group(1), True)), s)
            for name, k in re.findall(r'("[^"]*"|[^\s\[\]"]+)\[([^\]]*)\]', s):
                if re.search(r"\b(?:rate|avg|index)\b|don't sum", k):
                    out.add(f"nosum-col:{name.strip(chr(34))}")
        if s.startswith("FORMULAS "):
            out.add("formulas:-")
    return sorted(out)


def trap_checks(case: str, text: str, session: dict[str, str] | None = None) -> list[dict]:
    return ([{"what": w, "ok": re.search(rx, text) is not None} for w, rx in TRAP_CHECKS.get(case, [])]
            + naive_checks(case, text, session or {}))


# Parts of a line, ablated like kinds: (the line kind they live in, the pattern removed).
SEGMENTS = {
    "types": ("columns", re.compile(r"\[[^\]]*\]")),  # [text · dim], [15 × number · amount 2..9]
    "table_cols": ("table", re.compile(r" · \d+ cols(?:: .*?(?= · T\d+_long|$)|, same as T\d+)")),  # brief column lists
    "table_totals": ("table", re.compile(r" · \d+ total rows? \(only in [^)]*\): .*?(?= · \d+ cols| · amounts in|$)"
                                         r"| \(\+\d+ total rows?: .*?— only in T\d+_all\)")),
}


def strip_kinds(text: str, drop: set[str]) -> str:
    """The catalog without the lines of the dropped kinds (and without the dropped SEGMENTS)."""
    if not drop:
        return text
    out = []
    for line, kind in zip(text.split("\n"), line_kinds(text)):
        if kind in drop:
            continue
        for seg in drop & SEGMENTS.keys():
            k, rx = SEGMENTS[seg]
            if kind == k:
                line = rx.sub("", line)
        out.append(line)
    return "\n".join(out)


def segment_tokens(text: str) -> dict[str, int]:
    """Tokens each SEGMENTS kind costs in a catalog."""
    out = {}
    for seg, (k, rx) in SEGMENTS.items():
        n = 0
        for line, kind in zip(text.split("\n"), line_kinds(text)):
            if kind == k:
                n += count_tokens(line) - count_tokens(rx.sub("", line))
        out[seg] = n
    return out


# ---------------------------------------------------------------------------------------------
# tokens

_ENC = None
TOKENIZER = "chars/4"
try:  # an isolated venv with tiktoken (see README); the stdlib-only fallback is characters / 4
    import tiktoken  # type: ignore

    _ENC = tiktoken.get_encoding("o200k_base")
    TOKENIZER = "tiktoken o200k_base"
except Exception:  # noqa: BLE001
    pass


def count_tokens(text: str, chars_only: bool = False) -> int:
    """o200k tokens when tiktoken is importable, else ceil(characters / 4)."""
    if not text:
        return 0
    if _ENC is None or chars_only:
        return (len(text) + 3) // 4
    return len(_ENC.encode(text))


# ---------------------------------------------------------------------------------------------
# SQL identifiers

SQL_WORDS = set("""select from where and or not in is null like glob between as on join left right inner outer cross
group by order having limit offset desc asc distinct case when then else end union all exists cast integer real text
with over partition escape collate nocase""".split())
SQL_FUNCS = set("""sum count avg min max abs round coalesce ifnull nullif lower upper replace substr length trim
strftime date datetime julianday total group_concat typeof instr printf iif cast""".split())

_TOK = re.compile(r"""\s+|'(?:[^']|'')*'|"(?:[^"]|"")*"|\d+(?:\.\d+)?(?:e\d+)?|[A-Za-z_][\w]*|<>|!=|>=|<=|\|\||.""", re.S)


def sql_tokens(sql: str) -> list[tuple[str, str]]:
    """(kind, text) tokens: str, qid (quoted identifier), num, word, op."""
    out = []
    for m in _TOK.finditer(sql):
        t = m.group()
        if t.isspace():
            continue
        if t.startswith("'"):
            out.append(("str", t[1:-1].replace("''", "'")))
        elif t.startswith('"'):
            out.append(("qid", t[1:-1].replace('""', '"')))
        elif re.fullmatch(r"\d.*", t):
            out.append(("num", t))
        elif re.fullmatch(r"[A-Za-z_]\w*", t):
            out.append(("word", t))
        else:
            out.append(("op", t))
    return out


TABLE_RE = re.compile(r"(?:f\d+\.)?[TU]\d+(?:_long)?(?:_all)?$")


def sql_refs(sql: str) -> dict:
    """Tables, columns and compared literals of a query.

    Returns {"tables", "columns", "literals": [(column or None, op, value)], "numbers": [(column, value)]}.
    Columns are quoted identifiers plus unquoted words that are not SQL words, functions, tables or
    aliases (`AS x`, or a word right after an expression or a table). Literals compared with
    strftime(...) and strftime's format argument are left out (they follow from the SQL, not the data).
    """
    toks = sql_tokens(sql)
    tables, columns, aliases = [], [], set()
    # tables (FROM/JOIN x) and aliases
    for i, (k, t) in enumerate(toks):
        low = t.lower()
        if k == "word" and low in ("from", "join") and i + 1 < len(toks):
            j = i + 1
            name = toks[j][1]
            if toks[j][0] == "word" and j + 2 < len(toks) and toks[j + 1][1] == "." and toks[j + 2][0] == "word":
                name = name + "." + toks[j + 2][1]
                j += 2
            if toks[j][0] in ("word", "qid") and TABLE_RE.match(name):
                tables.append(name)
                if j + 1 < len(toks) and toks[j + 1][0] == "word" and toks[j + 1][1].lower() not in SQL_WORDS:
                    aliases.add(toks[j + 1][1].lower())
        if k == "word" and low == "as" and i + 1 < len(toks) and toks[i + 1][0] in ("word", "qid"):
            aliases.add(toks[i + 1][1].lower())
        if k == "word" and low not in SQL_WORDS and low not in SQL_FUNCS and i > 0:
            pk, pt = toks[i - 1]
            nxt = toks[i + 1][1].lower() if i + 1 < len(toks) else ""
            after_expr = pt == ")" or pk in ("qid", "num") or pk == "word" and pt.lower() not in SQL_WORDS | SQL_FUNCS
            if after_expr and nxt in ("", ",", "from", ")", "order", "group", "where", "join", "on"):
                aliases.add(low)
    table_words = {t.lower() for t in tables} | {t.split(".")[-1].lower() for t in tables} | {t.split(".")[0].lower() for t in tables}
    lits, nums = [], []
    for i, (k, t) in enumerate(toks):
        if k == "qid":
            if not (i > 0 and toks[i - 1][1].lower() in ("as",)):
                columns.append(t)
        elif k == "word":
            low = t.lower()
            nxt = toks[i + 1][1] if i + 1 < len(toks) else ""
            prv = toks[i - 1][1] if i > 0 else ""
            if low in SQL_WORDS and prv != "." or low in table_words or low in aliases and prv != ".":
                continue
            if low in SQL_FUNCS and nxt == "(":
                continue
            if nxt == ".":  # alias.column: the column is the next word
                continue
            columns.append(t)
    # compared literals: walk back from each string/number to the column it is compared with
    def subject(i: int) -> str | None:
        j = i - 1
        while j >= 0 and (toks[j][0] == "op" and toks[j][1] in ("=", "<>", "!=", "<", ">", "<=", ">=", ",", "(")
                          or toks[j][0] == "word" and toks[j][1].lower() in ("like", "not", "in", "between", "and", "glob")
                          or toks[j][0] in ("str", "num") and j < i):
            if toks[j][1] == "(" and j > 0 and toks[j - 1][0] == "word" and toks[j - 1][1].lower() not in ("in",):
                return None  # a function argument
            if toks[j][0] == "word" and toks[j][1].lower() == "and" and not any(
                    toks[x][1].lower() == "between" for x in range(max(0, j - 3), j)):
                return None
            j -= 1
        if j < 0:
            return None
        k, t = toks[j]
        if k == "qid" or k == "word" and t.lower() not in SQL_WORDS:
            return t
        if t == ")":  # f(col) = 'x' (e.g. strftime(...), UPPER(col), REPLACE(col, ...))
            depth, x = 0, j
            while x >= 0:
                depth += toks[x][1] == ")"
                depth -= toks[x][1] == "("
                if depth == 0:
                    break
                x -= 1
            fn = toks[x - 1][1].lower() if x > 0 else ""
            if fn == "strftime":
                return "strftime()"
            inner = [tt for kk, tt in toks[x + 1:j] if kk in ("qid", "word") and tt.lower() not in SQL_FUNCS]
            return inner[0] if inner else None
        return None

    def in_function(i: int) -> bool:
        """Is token i an argument of a function call (not an IN list or a parenthesized expression)?"""
        depth, x = 0, i - 1
        while x >= 0:
            if depth == 0 and toks[x][1].lower() in ("=", "<>", "!=", "<", ">", "<=", ">=", "like", "glob", "when", "then", "else", "in"):
                return False  # compared inside the call (CASE WHEN col = 'x' …), not an argument
            depth += toks[x][1] == ")"
            if toks[x][1] == "(":
                if depth == 0:
                    return x > 0 and toks[x - 1][0] == "word" and toks[x - 1][1].lower() in SQL_FUNCS
                depth -= 1
            x -= 1
        return False

    for i, (k, t) in enumerate(toks):
        if k == "str":
            if in_function(i):
                continue  # strftime's format, REPLACE(col, ' ', '') arguments, …
            col = subject(i)
            if col == "strftime()" or t == "":
                continue
            op = "like" if any(toks[x][1].lower() in ("like", "glob") for x in range(max(0, i - 2), i)) else "="
            lits.append((col, op, t))
        elif k == "num":
            if in_function(i):
                continue
            col = subject(i)
            if col and col != "strftime()":
                nums.append((col, t))
    return {"tables": tables, "columns": columns, "literals": lits, "numbers": nums}


# ---------------------------------------------------------------------------------------------
# what the catalog names


class CatalogIndex:
    """Names and values an agent can read off a catalog text (one or several files' catalogs)."""

    def __init__(self, text: str):
        self.text = text
        self.low = text.lower()
        kinds = line_kinds(text)
        lines = text.split("\n")
        self.quoted = set(re.findall(r'"([^"\n]*)"', text))
        self.quoted |= set(re.findall(r"'([^'\n]*)'", text))
        self.cols = set(self.quoted)
        self.values = set(self.quoted)  # values shown as values: quoted, or a field of an inline row
        self.templates: list[tuple[re.Pattern, str, str]] = []
        self.described: set[str] = set()
        self.views_all: list[tuple[int, int]] = []
        self.views: list[tuple[int, int]] = []
        order: dict[str, list[str]] = {}  # a brief table line's column names in order
        for line, kind in zip(lines, kinds):
            s = line.strip()
            if kind == "table":
                m = re.match(r"T(\d+)(?:\.\.T(\d+))?\s", s)  # a line for one table or a run T2..T10
                self.described |= {f"T{k}" for k in range(int(m.group(1)), int(m.group(2) or m.group(1)) + 1)}
                for m in re.finditer(r"(?:cols|headers here): (.*?)(?: \+\d+ more|$| · )", s):
                    self.cols |= {x.strip('"') for x in re.findall(r'"[^"]*"|[^\s\[\]"]+', m.group(1))}
                self.cols |= set(re.findall(r'(?:^|\s)([^\s\[\]"]+)\[', s))
                self.cols |= brace_names(s)
                # B100: columns as an earlier table's with one more level:
                # `Jahr + T27's "a".."b", each + " / % des BIP"`
                d = re.search(r'cols: (.*?)(T\d+)\'s "([^"]*)"\.\."([^"]*)", each \+ " / ([^"]*)"', s)
                if d and d.group(2) in order:
                    names = order[d.group(2)]
                    if d.group(3) in names and d.group(4) in names:
                        a, b = names.index(d.group(3)), names.index(d.group(4))
                        self.cols |= {x + " / " + d.group(5) for x in names[a:b + 1]}
                        self.cols |= {x.strip('"') for x in re.findall(r'"[^"]*"|[^\s"+]+', d.group(1))}
                else:
                    c = re.search(r"cols: (.*?)(?: \+\d+ more|$| · )", s)
                    if c:
                        order[re.match(r"T\d+", s).group(0)] = [n for x in re.findall(r'"[^"]*"|[^\s"]+', c.group(1))
                                                         for n in expand_braces(x.strip('"'))]
            if kind == "columns":
                self.cols |= set(re.findall(r'(?:^|\s)([^\s\[\]"]+)\[', s))
                bare = re.sub(r"\[[^\]]*\]|\{[^}]*\}", " ", s.split("): ", 1)[-1])
                self.cols |= {x.strip('"') for x in re.findall(r'"[^"]*"|[^\s"]+', bare)}
                self.cols |= brace_names(s)
                # shared prefixes: "a_{b, c[type]}" stands for a_b, a_c
                for pre, items in re.findall(r'(?:^|\s)([^\s\[\]"{}]+?)_\{([^}]*)\}', s):
                    self.cols |= {pre + "_" + re.sub(r"\[.*", "", x).strip() for x in items.split(", ")}
            if kind == "values":
                self.cols.add(s.split(" values: ")[0].strip('"'))
            if kind == "inline" and not re.match(r"(all|first) ", s):
                fields = {f.strip() for f in next(csv.reader([s]), [])}
                self.cols |= fields
                self.values |= fields
            if kind == "sql":
                for a, b in re.findall(r"\bT(\d+)\.\.T(\d+)\b", s):
                    self.views.append((int(a), int(b)))
                for a, b in re.findall(r"\bT(\d+)_all\.\.T(\d+)_all\b", s):
                    self.views_all.append((int(a), int(b)))
            for a, b in re.findall(r'"([^"]+)"\.\."([^"]+)"', s):
                self.templates.append(_template(a, b))
        self.words = set(re.findall(r"\b[A-Za-z_]\w*\b", text))
        self.words_low = {w.lower() for w in self.words}
        self.years = _year_ranges(text)
        self.shapes = {re.sub(r"\d", "9", m) for m in re.findall(r"\b\d{4}-[\w-]+\b", text)}
        self.number_tokens = set(re.findall(r"(?<![\w.])\d+(?![\w.])", text))

    def has_table(self, name: str) -> bool:
        base = name.split(".")[-1]
        if "." in name and "FILES open in this session" not in self.text:
            return False  # f<k>.T<n> is named by the FILES line
        m = re.match(r"([TU])(\d+)(_long)?(_all)?$", base)
        if not m:
            return False
        letter, n, long, all_ = m.group(1), int(m.group(2)), m.group(3), m.group(4)
        if letter == "U":
            return re.search(rf"\bU{n}\b", self.text) is not None
        if f"T{n}" not in self.described:
            return False
        if long:
            return re.search(rf"\bT{n}_long\b", self.text) is not None
        if all_:
            return re.search(rf"\bT{n}_all\b", self.text) is not None or any(a <= n <= b for a, b in self.views_all)
        return True

    def has_column(self, name: str, quoted: bool) -> bool:
        if name in self.cols or (not quoted and name.lower() in {c.lower() for c in self.cols}):
            return True
        if name.startswith("_") or name in ("period", "period_label", "value", "sheet", "title", "part", "variant"):
            return re.search(rf"(?<![\w]){re.escape(name)}(?![\w])", self.text) is not None
        return any(_in_template(name, t) for t in self.templates)

    def has_text(self, value: str, case: bool = True) -> bool:
        hay, needle = (self.text, value) if case else (self.low, value.lower())
        for m in re.finditer(re.escape(needle), hay):
            before = hay[m.start() - 1] if m.start() > 0 else " "
            after = hay[m.end()] if m.end() < len(hay) else " "
            if not (before.isalnum() and needle[:1].isalnum()) and not (after.isalnum() and needle[-1:].isalnum()):
                return True
        return False

    def has_value(self, lit: str) -> bool:
        """A filter value the catalog shows: quoted or as an inline-row field; a longer word anywhere."""
        if lit in self.values:
            return True
        return len(lit) >= 4 and not re.fullmatch(r"[\d.,\s-]+", lit) and self.has_text(lit)

    def year_covered(self, y: int) -> bool:
        return any(a <= y <= b for a, b in self.years)


def expand_braces(s: str, keep_kinds: bool = False) -> list[str]:
    """Shared-prefix / suffix groups as the catalog writes them: `a / {b, c / {d, e}} / f` stands for
    "a / b / f", "a / c / d / f", "a / c / e / f". Members are separated by ", ", or by "; " when a
    member has a comma; a member's `[kind]` is not part of its name."""
    i = s.find("{")
    if i < 0:
        return [s]
    depth, j = 0, i
    for j in range(i, len(s)):
        depth += {"{": 1, "}": -1}.get(s[j], 0)
        if depth == 0:
            break
    inner, pre, post = s[i + 1:j], s[:i], s[j + 1:]
    post = re.sub(r"^\[[^\]]*\]", "", post)
    sep = "; " if _top_level(inner, "; ") else ", "
    parts, depth, cur, k = [], 0, "", 0
    while k < len(inner):
        ch = inner[k]
        depth += {"{": 1, "}": -1}.get(ch, 0)
        if depth == 0 and inner.startswith(sep, k):
            parts.append(cur)
            cur, k = "", k + len(sep)
            continue
        cur += ch
        k += 1
    parts.append(cur)
    out = []
    for p in parts:
        p = p.strip()
        kind = re.search(r"\[[^\]{}]*\]$", p)
        p = p[:kind.start()] if kind else p
        out += [x + (kind.group(0) if kind and keep_kinds else "") for x in expand_braces(pre + p + post, keep_kinds)]
    return out


def _top_level(text: str, sep: str) -> bool:
    depth = 0
    for k, ch in enumerate(text):
        depth += {"{": 1, "}": -1}.get(ch, 0)
        if depth == 0 and text.startswith(sep, k):
            return True
    return False


def brace_names(line: str) -> set[str]:
    """Every column name a line's brace groups stand for: quoted groups ("a / {b, c}") and bare
    ones (`biofuel_{x, y}`)."""
    out: set[str] = set()
    for q in re.findall(r'"([^"]*\{[^"]*)"', line):
        out |= set(expand_braces(q))
    for m in re.finditer(r'(?:^|\s)([^\s"{}\[\]]+\{)', line):
        start, depth = m.start(1), 0
        for j in range(m.end(1) - 1, len(line)):
            depth += {"{": 1, "}": -1}.get(line[j], 0)
            if depth == 0:
                out |= set(expand_braces(line[start:j + 1]))
                break
    return out


def _template(a: str, b: str) -> tuple[re.Pattern, str, str]:
    ta, tb = re.findall(r"\d+|\D+?(?=\d|$|\s)|\s", a), re.findall(r"\d+|\D+?(?=\d|$|\s)|\s", b)
    if len(ta) != len(tb):
        pat = re.sub(r"\\d\+", r"\\d+", re.sub(r"\d+", r"\\d+", re.escape(a)))
        return re.compile(pat), a, b
    parts = []
    for x, y in zip(ta, tb):
        if x == y:
            parts.append(re.escape(x))
        elif x.isdigit() and y.isdigit():
            parts.append(r"\d+")
        else:
            parts.append(r"\S+")
    return re.compile("".join(parts)), a, b


def _in_template(name: str, t: tuple[re.Pattern, str, str]) -> bool:
    pat, a, b = t
    if not pat.fullmatch(name):
        return False
    da, db, dn = ("".join(re.findall(r"\d", x)) for x in (a, b, name))
    if len(da) == len(db) == len(dn) and da and not min(da, db) <= dn <= max(da, db):
        return False
    return True


def _year_ranges(text: str) -> list[tuple[int, int]]:
    out = []
    for a, b in re.findall(r"\b((?:19|20)\d\d)(?:[-/][\w:]+)*(?: [\d:]+)?\s*(?:–|\.\.)\s*'?((?:19|20)\d\d)\b", text):
        out.append((min(int(a), int(b)), max(int(a), int(b))))
    for m in re.finditer(r"^TIME ((?:19|20)\d\d)\b", text, re.M):
        out.append((int(m.group(1)), int(m.group(1))))
    return out


def literal_ok(idx: CatalogIndex, col: str | None, op: str, lit: str, prompt: str, top: set[str]) -> str | None:
    """Why a compared literal is discoverable, or None.

    In order: it is in the catalog (exact spelling; a LIKE pattern's pieces case-insensitively);
    it is in the question itself; it is a date or period whose shape the catalog shows and whose year
    the catalog's ranges cover; a year-prefix pattern ('2023%') inside a covered range; it is among
    in the first 5 rows of a ranking query on its column (see top_probes).
    """
    if op == "like":
        parts = [p for p in re.split(r"[%_]+", lit) if p]
        if all(idx.has_text(p, case=False) or re.fullmatch(r"(19|20)\d\d", p) and idx.year_covered(int(p))
               or p.lower() in prompt.lower() for p in parts):
            return "pattern pieces in the catalog"
        rx = re.compile("".join(".*" if c == "%" else "." if c == "_" else re.escape(c) for c in lit), re.I | re.S)
        if any(rx.fullmatch(v) for v in top):
            return "matches a value in the first 5 rows of a ranking query on its column"
        return None
    if idx.has_value(lit):
        return "in the catalog"
    if len(lit) >= 2 and not re.fullmatch(r"[\d.,\s-]+", lit) and re.search(rf"(?<!\w){re.escape(lit)}(?!\w)", prompt, re.I):
        return "in the question"
    m = re.fullmatch(r"((?:19|20)\d\d)(-[\w]+)*", lit)
    if m and idx.year_covered(int(m.group(1))) and (lit == m.group(1) or re.sub(r"\d", "9", lit) in idx.shapes):
        return "a date in the catalog's range and format"
    if lit in top:
        return "in the first 5 rows of a ranking query on its column"
    return None


def number_ok(idx: CatalogIndex, num: str, top: set[str] = frozenset()) -> bool:
    try:
        v = float(num)
    except ValueError:
        return False
    if num in top:
        return True
    if v == 0 or (v.is_integer() and 1900 <= v <= 2100 and idx.year_covered(int(v))):
        return True
    return num in idx.number_tokens or (v.is_integer() and str(int(v)) in idx.number_tokens)


def missing_names(sql: str, idx: CatalogIndex, prompt: str, top: set[str], notes: list[str] | None = None) -> list[str]:
    """What a query uses that the catalog does not name (empty: one query away).

    `notes` collects the literals that are not literally in the catalog, with the rule that found them."""
    r = sql_refs(sql)
    miss = []
    for t in dict.fromkeys(r["tables"]):
        if not idx.has_table(t):
            miss.append(f"table {t}")
    quoted = set(re.findall(r'"((?:[^"]|"")*)"', sql))
    for c in dict.fromkeys(r["columns"]):
        if not idx.has_column(c, c in quoted):
            miss.append(f'column "{c}"')
    for col, op, lit in r["literals"]:
        why = literal_ok(idx, col, op, lit, prompt, top)
        if why is None:
            miss.append(f"value '{lit}'" + (f" of {col}" if col else ""))
        elif notes is not None and why not in ("in the catalog", "pattern pieces in the catalog"):
            notes.append(f"'{lit}': {why}")
    for col, num in r["numbers"]:
        if not number_ok(idx, num, top):
            miss.append(f"number {num} for {col}")
    return list(dict.fromkeys(miss))


# ---------------------------------------------------------------------------------------------
# value in the catalog (a)


def sig_digits(tok: str) -> int:
    d = re.sub(r"[^\d]", "", tok.split(".")[0] if "." in tok and len(tok.split(".")[-1]) == 3 else tok)
    d = d.lstrip("0")
    return len(d.rstrip("0")) if "." not in tok else len(d)


def line_numbers(line: str, inline: bool) -> list[tuple[float, str]]:
    """(value, raw token) for every number reading in a line; CSV rows are split into fields first."""
    out = []
    fields = next(csv.reader([line.strip()])) if inline else [line]
    for f in fields:
        f = f.replace("..", " .. ")  # ranges: 0..30.07 M
        for m in NUM_RE.finditer(f):
            tail = re.match(r"[^\d]{0,16}", f[m.end():]).group()  # the unit word after it (bn, M, k)
            try:
                out += [(v, m.group()) for v in numbers(m.group() + tail)]
            except ValueError:
                continue
    return out


NUM_RE = re.compile(r"(?<![\w.])[-−]?\d(?:[\d.,  ]*\d)?")


def value_in_catalog(ch: dict, text: str, attrib: list[str], scale: list[float]) -> str | None:
    """The catalog line that states the check's value (attributable), or None."""
    kinds = line_kinds(text)
    words = [w.lower() for w in attrib if len(w) >= 2]
    unnamed = None
    for line, kind in zip(text.split("\n"), kinds):
        low = line.lower()
        named = any(w in low for w in words)
        for v, tok in line_numbers(line, kind == "inline"):
            x = abs(v) if ch.get("abs") else v
            if "num" in ch:
                want, tol = ch["num"], ch.get("tol", 0.005)
                hit = any(abs(x * f - want) <= tol * abs(want) + 1e-9 for f in [1.0] + scale)
                if hit and named:
                    return line.strip()[:160]
                if hit and sig_digits(tok) >= 4 and unnamed is None:
                    unnamed = line.strip()[:160]
            elif "pct_range" in ch:
                lo, hi = ch["pct_range"]
                if lo <= x <= hi and named:
                    return line.strip()[:160]
    return unnamed


# ---------------------------------------------------------------------------------------------
# the session: catalogs and every query a check needs


def data_view(sql: str) -> str:
    return re.sub(r"\b(T\d+(?:_long)?)_all\b", r"\1", re.sub(r"\b(U\d+)_all\b", r"\1", sql))


AGG = re.compile(r"\b(SUM|COUNT|AVG|MIN|MAX)\s*\(", re.I)


def top_probes(sql: str) -> list[str]:
    """Ranking queries that show a filtered value without knowing it.

    For `SELECT <expr> FROM T WHERE … col = 'lit' …` (or `col LIKE 'pat'`, or `col = 4`): the values of
    col ranked by <expr> with that condition dropped, both ways (first and last 5); grouped when
    <expr> aggregates. An agent asked "which X has the most Y" runs exactly this query.
    """
    m = re.fullmatch(r"\s*SELECT\s+(.+?)\s+FROM\s+(\S+)\s+WHERE\s+(.+?)\s*", sql, re.S | re.I)
    if not m or "SELECT" in m.group(3).upper() or "SELECT" in m.group(1).upper() or "CASE" in m.group(1).upper():
        return []
    expr, table, where = m.groups()
    agg = AGG.search(expr) is not None
    out = []
    for cm in re.finditer(r"""("(?:[^"]|"")+"|\b[A-Za-z_]\w*)\s*(?:=|\bLIKE\b)\s*('(?:[^']|'')*'|\d+\b)""", where, re.I):
        col = cm.group(1)
        if col.strip('"').startswith("_"):
            continue
        rest = where[:cm.start()] + "1=1" + where[cm.end():]
        for d in ("DESC", "ASC"):
            if agg:
                out.append(f"SELECT {col} FROM {table} WHERE {rest} GROUP BY {col} ORDER BY {expr} {d} LIMIT 5")
            else:
                out.append(f"SELECT {col} FROM {table} WHERE {rest} AND {expr} IS NOT NULL ORDER BY {expr} {d} LIMIT 5")
    return out


def ref_parts(case: dict, ch: dict) -> list[dict]:
    """[{file, sql}] of a check's reference SQL (one per file for multi-file sums)."""
    r = ch.get("ref_sql")
    if r is None:
        return []
    return [{"file": case["file"], "sql": r}] if isinstance(r, str) else list(r)


def plan(case: dict) -> list[tuple[str, tuple[str, dict]]]:
    """(key, tool call) for the case's session."""
    calls = [(f"open:{f}", ("open_file", {"path": f})) for f in case_files(case)]
    probes = PROBES.get(case["id"], {})
    for ch in case["checks"]:
        parts = ref_parts(case, ch)
        if "sql" in probes.get(ch["id"], {}):
            parts = [{"file": case["file"], "sql": probes[ch["id"]]["sql"]}]
        for j, p in enumerate(parts):
            q = lambda sql, lim=5: ("query", {"path": p["file"], "sql": sql, "limit": lim})  # noqa: E731
            calls.append((f"ref:{ch['id']}:{j}", q(p["sql"])))
            if data_view(p["sql"]) != p["sql"]:
                calls.append((f"view:{ch['id']}:{j}", q(data_view(p["sql"]))))
            for k, tq in enumerate(top_probes(p["sql"])):
                calls.append((f"top:{ch['id']}:{j}:{k}", q(tq, 10)))
        if "alt" in probes.get(ch["id"], {}):
            alt = probes[ch["id"]]["alt"]
            calls.append((f"alt:{ch['id']}", ("query", {"path": alt.get("file", case["file"]), "sql": alt["sql"], "limit": 5})))
    for k, t in enumerate(NAIVE_TRAPS.get(case["id"], [])):
        calls.append((f"naive:{k}", ("query", {"path": case["file"], "sql": t["sql"], "limit": 5})))
    return calls


def run_session(case: dict, root: Path) -> dict[str, str]:
    calls = plan(case)
    texts = S.call(root, [c for _, c in calls])
    out = dict(zip([k for k, _ in calls], texts))
    cells_needed = [ch for ch in case["checks"] if "ref_cells" in ch]
    if cells_needed:
        out.update(scan_cells(case, root, out, cells_needed))
    return out


def scan_cells(case: dict, root: Path, first: dict[str, str], checks: list[dict]) -> dict[str, str]:
    """Look up every referenced source cell's value in every table (a second session)."""
    out: dict[str, str] = {}
    calls: list[tuple[str, tuple[str, dict]]] = []
    for f in case_files(case):
        if not any(ch.get("ref_file", case["file"]) == f for ch in checks):
            continue
        cells = cell_values(root / f)
        # the columns of every table (one query per table: a single sqlite_master query is cut)
        n = int(re.search(r"· ([\d,]+) tables?", first[f"open:{f}"]).group(1).replace(",", ""))
        schema = [("open_file", {"path": f})] + [("query", {"path": f, "limit": 1000, "sql":
                  f"SELECT name FROM pragma_table_info('T{k}_all')"}) for k in range(1, n + 1)]
        cols = {f"T{k}_all": [c for c in first_column(t) if not c.startswith("_")]
                for k, t in enumerate(S.call(root, schema)[1:], 1)}
        calls += [(f"open:{f}", ("open_file", {"path": f}))]
        for ch in checks:
            if ch.get("ref_file", case["file"]) != f:
                continue
            for a in ch["ref_cells"]:
                m = re.fullmatch(r"'?(.*?)'?!([A-Z]+[0-9]+)", a)
                raw = cells.get((m.group(1), m.group(2)))
                out[f"cell:{a}"] = raw if raw is not None else ""
                try:
                    v = float(raw)
                except (TypeError, ValueError):
                    continue
                eps = max(1e-9, 1e-6 * abs(v))
                for t, cs in cols.items():
                    conds = [(c, f'abs("{c.replace(chr(34), chr(34) * 2)}" - ({v!r})) <= {eps!r}') for c in cs]
                    which = "CASE " + " ".join(f"WHEN {x} THEN '{c.replace(chr(39), chr(39) * 2)}'" for c, x in conds) + " END"
                    sql = f'SELECT _src_row, {which} FROM "{t}" WHERE ' + " OR ".join(x for _, x in conds)
                    calls.append((f"scan:{a}:{t}", ("query", {"path": f, "limit": 3, "sql": sql})))
    if calls:
        texts = S.call(root, [c for _, c in calls])
        out.update({k: t for (k, _), t in zip(calls, texts) if not k.startswith("open:")})
    return out


# ---------------------------------------------------------------------------------------------
# scoring


def result_value(text: str) -> float | str:
    try:
        return first_value(text)
    except (ValueError, IndexError, StopIteration):
        lines = text.splitlines()
        if len(lines) >= 2 and not text.startswith("SQL error") and not lines[1].startswith("("):
            return next(csv.reader([lines[1]]), [""])[0] if lines[1] else ""
        return "error: " + text.strip()[:200]


def first_column(text: str) -> list[str]:
    lines = text.splitlines()
    out = []
    for l in lines[1:]:
        if l.startswith("(") or l.startswith("note:") or l.startswith("cite as"):
            break
        out.append(next(csv.reader([l]))[0] if l else "")
    return out


def matches(ch: dict, v: float) -> bool:
    x = abs(v) if ch.get("abs") else v
    if "num" in ch:
        return abs(x - ch["num"]) <= max(ch.get("tol", 0.005), 1e-9) * abs(ch["num"]) + 1e-9
    lo, hi = ch["pct_range"]
    return lo <= x <= hi


def text_hit(ch: dict, text: str) -> bool:
    return any(t.lower() in text.lower() for t in ch["text_any"])


def score_check(case: dict, ch: dict, catalog: str, session: dict[str, str], done: dict[str, dict]) -> dict:
    """{cat, reason, value} for one check against one catalog text."""
    probes = PROBES.get(case["id"], {})
    probe = probes.get(ch["id"], {})
    idx = CatalogIndex(catalog)
    prompt = case["prompt"]
    multi = len(case_files(case)) > 1
    if "same_as" in probe:
        other = done[probe["same_as"]]
        return {"cat": other["cat"], "reason": f"follows from {probe['same_as']}: {other['reason']}"}
    parts = ref_parts(case, ch)
    if "sql" in probe:
        parts = [{"file": case["file"], "sql": probe["sql"]}]
    lits = [l for p in parts for _, _, l in sql_refs(p["sql"])["literals"]]
    attrib = [w for w in ch.get("near", []) + lits if not re.fullmatch(r"[\d%]+", w)]
    scale = []
    for p in parts:
        for m in re.finditer(r"\*\s*(1e\d+|10+(?:\.0)?)\b", p["sql"]):
            scale.append(float(m.group(1)))

    # a: in the catalog
    if "text_any" in ch:
        pair = done.get(probe.get("pair", ""))
        if any(idx.has_text(t, case=False) for t in ch["text_any"]) and pair and pair["cat"] == "a":
            return {"cat": "a", "reason": f"the phrase is in the catalog and {probe['pair']} is in the catalog"}
    else:
        line = value_in_catalog(ch, catalog, attrib, scale)
        if line:
            return {"cat": "a", "reason": f"in the catalog: {line}"}

    cells = cells_verdict(ch, session, idx) if "ref_cells" in ch else None
    if not parts:
        return cells or {"cat": "c", "reason": "no reference query or probe"}

    # run results; a probe gives the final value (no ref_scale)
    scale_ref = 1 if "sql" in probe else ch.get("ref_scale", 1)
    src = "natural query (probe)" if "sql" in probe else "reference query"

    def failed(why: str) -> dict:
        if cells:  # the value is in the tables (c) or not (d), whatever the probe got
            return {"cat": cells["cat"], "reason": f"the {src} {why}; {cells['reason']}"}
        return {"cat": "d", "reason": f"the {src} {why}"}

    vals, reasons = [], []
    for j, p in enumerate(parts):
        v = result_value(session.get(f"ref:{ch['id']}:{j}", ""))
        if isinstance(v, str) and v.startswith("error"):
            return failed(f"fails: {v[7:160]}")
        vals.append(v)
    if "text_any" in ch:
        if not text_hit(ch, " ".join(str(v) for v in vals)):
            return failed(f"answers {vals[0]!r}, not {ch['text_any'][0]!r}")
    else:
        try:
            total = sum(float(v) for v in vals) * scale_ref
        except (TypeError, ValueError):
            return failed(f"gives {vals!r}, not a number")
        if not matches(ch, total):
            return failed(f"gives {total:,.6g} on hedgehog's tables, expected {ch.get('num', ch.get('pct_range'))}")

    # data view: does the catalog's 'data rows' view give the same answer?
    for j, p in enumerate(parts):
        vt = session.get(f"view:{ch['id']}:{j}")
        if vt is None or "_row_type" in p["sql"] or not AGG.search(p["sql"]):
            continue
        v = result_value(vt)
        if isinstance(v, float) and not (vals[j] == v or (isinstance(vals[j], float) and abs(vals[j] - v) <= 1e-9 * max(1, abs(v)))):
            reasons.append(f"{data_view(p['sql'])[:60]}… on the data rows gives {v:,.6g}, not {vals[j]:,.6g}: "
                           "rows the catalog calls totals are needed")
    if reasons:
        return {"cat": "d", "reason": reasons[0]}

    # b: names
    top = top_values(session)
    miss, notes = [], []
    for p in parts:
        miss += missing_names(p["sql"], idx, prompt, top, notes)
    if multi and len(parts) > 1 and not re.search(r"\bFILES open in this session\b", catalog):
        miss.append("the FILES line (one query across files)")
    if miss and "alt" in probe:
        alt_miss = missing_names(probe["alt"]["sql"], idx, prompt, top)
        v = result_value(session.get(f"alt:{ch['id']}", ""))
        ok = isinstance(v, float) and matches(ch, v)
        if ok and not alt_miss:
            return {"cat": "b", "reason": "a natural alternative query (names from the question and the catalog) "
                                          f"reproduces it; the reference needs {', '.join(dict.fromkeys(miss))}"}
        miss.append(f"(the natural query gives {v if isinstance(v, str) else f'{v:,.6g}'}" +
                    (f" and needs {', '.join(alt_miss)})" if alt_miss else ")"))
    if miss:
        return {"cat": "c", "reason": "the query needs " + ", ".join(dict.fromkeys(miss))}
    via = "natural query (probe)" if "sql" in probe else "reference query"
    found = f" ({'; '.join(dict.fromkeys(notes))})" if notes else ""
    return {"cat": "b", "reason": f"the {via} uses only names the catalog shows{found}"}


def cells_verdict(ch: dict, session: dict[str, str], idx: CatalogIndex) -> dict:
    """c when every referenced source cell's value is in some table (where), d when one is in none."""
    lost, found = [], []
    for a in ch["ref_cells"]:
        hits = []
        for k, t in session.items():
            rows = list(csv.reader(io.StringIO(t)))[1:2] if k.startswith(f"scan:{a}:") else []
            if rows and len(rows[0]) == 2 and rows[0][0].isdigit():
                tab, col = k.split(":")[-1], rows[0][1]
                shown = "named in the catalog" if idx.has_column(col, True) else "not named in the catalog"
                hits.append(f'{tab} row {rows[0][0]}, column "{col[:50]}" ({shown})')
        (found if hits else lost).append(a + (f" → {hits[0]}" if hits else f" = {session.get('cell:' + a, '?')[:12]}"))
    if lost:
        return {"cat": "d", "reason": f"{len(lost)} of {len(ch['ref_cells'])} source cells in no table: " + "; ".join(lost[:4])}
    return {"cat": "c", "reason": "in the tables but not in the catalog: " + "; ".join(found[:2])
            + (f" (+{len(found) - 2} more cells)" if len(found) > 2 else "")}


TOP_N = 5  # a value counts as discoverable when one ranking query shows it in its first 5 rows


def top_values(session: dict[str, str]) -> set[str]:
    """Every value in the first TOP_N rows of the case's ranking queries (one query reveals them)."""
    out: set[str] = set()
    for k, t in session.items():
        if k.startswith("top:"):
            out |= set(first_column(t)[:TOP_N])
    return out


def score_case(case: dict, catalogs: list[str], session: dict[str, str], drop: set[str] = frozenset()) -> list[dict]:
    text = "\n".join(strip_kinds(c, set(drop)) for c in catalogs)
    done: dict[str, dict] = {}
    out = []
    order = sorted((ch for ch in case["checks"] if not ch.get("judge")),
                   key=lambda ch: "same_as" in PROBES.get(case["id"], {}).get(ch["id"], {}) or "text_any" in ch)
    for ch in order:
        r = score_check(case, ch, text, session, done)
        done[ch["id"]] = r
    for ch in case["checks"]:
        if ch["id"] in done:
            out.append({"check": ch["id"], "kind": next(k for k in ("num", "pct_range", "text_any") if k in ch), **done[ch["id"]]})
    return out


# ---------------------------------------------------------------------------------------------
# report


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cases", default="")
    ap.add_argument("--offline", action="store_true", help="re-score from --cache, no hedgehog calls")
    ap.add_argument("--cache", type=Path, default=Path(os.environ.get("HH_INFO_CACHE", HERE / "cache")),
                    help="where session outputs are kept (default eval/info/cache, not committed)")
    ap.add_argument("--out", type=Path, default=HERE)
    ap.add_argument("--no-ablation", action="store_true")
    ap.add_argument("--question", action="store_true",
                    help="also open each file with the case's prompt as the question and compare (QUESTION.md)")
    args = ap.parse_args()
    cases = json.loads((AGENTS / "cases.json").read_text())
    want = set(filter(None, args.cases.split(",")))
    args.cache.mkdir(parents=True, exist_ok=True)
    data = data_root()
    results = []
    qresults = []
    for c in cases:
        if want and c["id"] not in want:
            continue
        cache = args.cache / f"{c['id']}.json"
        if args.offline or (cache.exists() and os.environ.get("HH_INFO_REUSE")):
            session = json.loads(cache.read_text())
        else:
            session = run_session(c, data / c.get("root", "irregular"))
            cache.write_text(json.dumps(session, ensure_ascii=False, indent=0))
        catalogs = [session[f"open:{f}"] for f in case_files(c)]
        checks = score_case(c, catalogs, session)
        text = "\n".join(catalogs)
        kinds = [k for cat in catalogs for k in line_kinds(cat)]
        lines = [l for cat in catalogs for l in cat.split("\n")]
        by_kind: dict[str, int] = {}
        for l, k in zip(lines, kinds):
            by_kind[k] = by_kind.get(k, 0) + count_tokens(l + "\n")
        by_seg: dict[str, int] = {}
        for cat in catalogs:
            for k, v in segment_tokens(cat).items():
                by_seg[k] = by_seg.get(k, 0) + v
        results.append({"case": c["id"], "files": case_files(c), "tokens": count_tokens(text), "chars": len(text),
                        "tokens_by_kind": by_kind, "tokens_by_segment": by_seg, "checks": checks,
                        "traps": sorted({f"f{k}:{x}" if len(catalogs) > 1 else x
                                         for k, cat in enumerate(catalogs, 1) for x in trap_facts(cat)}),
                        "trap_checks": trap_checks(c["id"], text, session),
                        "other_lines": [l for l, k in zip(lines, kinds) if k == "other" and l.strip()][:5]})
        n_ok = sum(x["cat"] in "ab" for x in checks)
        print(f"{c['id']}: {n_ok}/{len(checks)} sufficient, {results[-1]['tokens']} tokens", flush=True)
        if args.question:
            qr = question_case(c, session, args.cache / f"{c['id']}.q.json", data, args.offline)
            qresults.append(qr)
            q_ok = sum(x["cat"] in "ab" for x in qr["checks"])
            print(f"{c['id']} with question: {q_ok}/{len(checks)} sufficient, {qr['tokens']} tokens", flush=True)
    ablation = []
    if not args.no_ablation:
        sessions = {r["case"]: json.loads((args.cache / f"{r['case']}.json").read_text()) for r in results}
        base = {(r["case"], x["check"]): x["cat"] for r in results for x in r["checks"]}
        for kind in KINDS + tuple(SEGMENTS):
            tokens = sum(r["tokens_by_kind"].get(kind, r["tokens_by_segment"].get(kind, 0)) for r in results)
            if not tokens:
                continue
            lost, moved = [], {}
            for r in results:
                c = next(x for x in cases if x["id"] == r["case"])
                cats = [sessions[r["case"]][f"open:{f}"] for f in case_files(c)]
                for x in score_case(c, cats, sessions[r["case"]], {kind}):
                    before = base[(r["case"], x["check"])]
                    if before in "ab" and x["cat"] not in "ab":
                        lost.append(f"{r['case']}/{x['check']}")
                    if before != x["cat"]:
                        moved[f"{before}→{x['cat']}"] = moved.get(f"{before}→{x['cat']}", 0) + 1
            ablation.append({"kind": kind, "tokens": tokens, "lost": lost, "moved": moved})
    write(args.out, results, ablation, bool(want))
    if args.question:
        write_question(args.out, results, qresults, bool(want))


# ---------------------------------------------------------------------------------------------
# question mode (B150): open_file with the case's prompt as the question


def question_catalogs(case: dict, root: Path) -> dict[str, str]:
    """The case's catalogs opened with its prompt as the question (one session, like an agent's)."""
    calls = [(f"open:{f}", ("open_file", {"path": f, "question": case["prompt"]})) for f in case_files(case)]
    return dict(zip([k for k, _ in calls], S.call(root, [c for _, c in calls])))


def table_count(catalog: str) -> int:
    m = re.search(r"· ([\d,]+) tables?\b", catalog.split("\n", 1)[0])
    return int(m.group(1).replace(",", "")) if m else 0


def matched_tables(catalog: str) -> list[str]:
    """The tables a catalog's QUESTION line names."""
    m = re.search(r"^QUESTION matches (.*?): (?:first|T\d)", catalog, re.M)
    return re.findall(r"\b(T\d+) \(", m.group(1)) if m else []


def question_case(case: dict, session: dict[str, str], cache: Path, data: Path, offline: bool) -> dict:
    if offline or (cache.exists() and os.environ.get("HH_INFO_REUSE")):
        q = json.loads(cache.read_text())
    else:
        q = question_catalogs(case, data / case.get("root", "irregular"))
        cache.write_text(json.dumps(q, ensure_ascii=False, indent=0))
    # The second session finds the file in hedgehog's disk cache: the same timing line as the first
    # one, so that only the question changes the size.
    for k, text in q.items():
        timing = re.compile(r"\n\((?:cached|normalized in [\d.]+ s)\)$")
        if timing.search(text) and timing.search(session.get(k, "")):
            last = "\n" + session[k].rsplit("\n", 1)[1]
            q[k] = timing.sub(lambda _: last, text)
    catalogs = [q[f"open:{f}"] for f in case_files(case)]
    checks = score_case(case, catalogs, {**session, **q})
    text = "\n".join(catalogs)
    matched = [f"f{k}:{t}" if len(catalogs) > 1 else t for k, cat in enumerate(catalogs, 1) for t in matched_tables(cat)]
    return {"case": case["id"], "tokens": count_tokens(text), "chars": len(text), "checks": checks,
            "tables": sum(table_count(cat) for cat in catalogs), "multi_table": any(table_count(cat) >= 2 for cat in catalogs),
            "matched": matched, "question_lines": [l for l in text.split("\n") if l.startswith("QUESTION ")],
            "traps": sorted({f"f{k}:{x}" if len(catalogs) > 1 else x for k, cat in enumerate(catalogs, 1) for x in trap_facts(cat)}),
            "trap_checks": trap_checks(case["id"], text, {**session, **q})}


def lost_traps(before: list[str], after: list[str], matched: list[str]) -> list[str]:
    """Trap facts of the matched tables that the question's catalog lost (the other tables are listed
    one line each, so their warnings may go)."""
    def table_of(fact: str) -> str:
        parts = fact.split(":")
        pre = parts[0] + ":" if re.fullmatch(r"f\d+", parts[0]) else ""
        rest = parts[1:] if pre else parts
        return pre + rest[1] if len(rest) > 1 else ""
    keep = set(matched)
    return [f for f in before if f not in after and table_of(f) in keep]


def write_question(out: Path, results: list[dict], qresults: list[dict], partial: bool) -> None:
    base = {r["case"]: r for r in results}
    rows = []
    for q in qresults:
        b = base[q["case"]]
        ab0 = sum(x["cat"] in "ab" for x in b["checks"])
        ab1 = sum(x["cat"] in "ab" for x in q["checks"])
        lost = [{"check": x["check"], "before": y["cat"], "after": x["cat"], "reason": x["reason"]}
                for x, y in zip(q["checks"], b["checks"]) if y["cat"] in "ab" and x["cat"] not in "ab"]
        rows.append({"case": q["case"], "checks": len(q["checks"]), "ab_without": ab0, "ab_with": ab1,
                     "tokens_without": b["tokens"], "tokens_with": q["tokens"], "chars_without": b["chars"],
                     "chars_with": q["chars"], "tables": q["tables"], "multi_table": q["multi_table"],
                     "matched": q["matched"], "lost": lost, "lost_traps": lost_traps(b["traps"], q["traps"], q["matched"]),
                     "trap_checks": q["trap_checks"], "question_lines": q["question_lines"]})

    def total(rs: list[dict]) -> dict:
        t0, t1 = sum(r["tokens_without"] for r in rs), sum(r["tokens_with"] for r in rs)
        return {"cases": len(rs), "checks": sum(r["checks"] for r in rs), "ab_without": sum(r["ab_without"] for r in rs),
                "ab_with": sum(r["ab_with"] for r in rs), "tokens_without": t0, "tokens_with": t1,
                "token_change": round((t1 - t0) / t0, 4) if t0 else 0}
    summary = {"tokenizer": TOKENIZER, "all": total(rows), "multi_table": total([r for r in rows if r["multi_table"]]),
               "focused": total([r for r in rows if r["matched"]])}
    (out / "results-question.json").write_text(json.dumps({"summary": summary, "cases": rows}, ensure_ascii=False, indent=1) + "\n")
    print(json.dumps({"question": summary}))
    if partial:
        return
    pct = lambda a, b: f"{100 * (b - a) / a:+.0f} %" if a else "–"  # noqa: E731
    L = ["# Question-aware catalog (B150): with vs without the question", "",
         "Generated by [score.py](score.py) `--question`: every file is opened a second time with the case's prompt",
         "as `open_file`'s `question`, and the same checks are scored on that catalog (the query results are the",
         "same). The tables the question matches come first and in full, the others one line each.", "",
         f"Tokens: {TOKENIZER}.", "",
         "## Overall", "",
         "| Cases | Checks | a+b without | a+b with | Tokens without | Tokens with | Change |",
         "|---|---:|---:|---:|---:|---:|---:|"]
    for name, key in (("all", "all"), ("multi-table files", "multi_table"), ("question matched some tables", "focused")):
        t = summary[key]
        L.append(f"| {name} ({t['cases']}) | {t['checks']} | {t['ab_without']} | {t['ab_with']} | {t['tokens_without']:,} | "
                 f"{t['tokens_with']:,} | {pct(t['tokens_without'], t['tokens_with'])} |")
    L += ["", "## Per case", "",
          "| Case | Tables | Matched | a+b without | a+b with | Tokens without | Tokens with | Change |",
          "|---|---:|---|---:|---:|---:|---:|---:|"]
    for r in rows:
        m = ", ".join(r["matched"]) or "–"
        L.append(f"| {r['case']} | {r['tables']} | {m} | {r['ab_without']}/{r['checks']} | {r['ab_with']}/{r['checks']} | "
                 f"{r['tokens_without']:,} | {r['tokens_with']:,} | {pct(r['tokens_without'], r['tokens_with'])} |")
    L += ["", "## Checks lost with the question", ""]
    lost = [(r["case"], x) for r in rows for x in r["lost"]]
    L += ["| Case | Check | Without → with | Reason with the question |", "|---|---|---|---|"] if lost else ["None."]
    for case, x in lost:
        L.append(f"| {case} | {x['check']} | {x['before']} → {x['after']} | {x['reason'].replace('|', '/')[:200]} |")
    L += ["", "## Trap warnings of matched tables lost with the question", ""]
    traps = [(r["case"], f) for r in rows for f in r["lost_traps"]]
    traps += [(r["case"], "trap check failed: " + x["what"]) for r in rows for x in r["trap_checks"] if not x["ok"]]
    L += [f"- {c}: `{f}`" for c, f in traps] or ["None."]
    L += ["", "## QUESTION lines", ""]
    for r in rows:
        for l in r["question_lines"]:
            L.append(f"- {r['case']}: `{l[:300]}`")
    (out / "QUESTION.md").write_text("\n".join(L) + "\n")


KIND_DESC = {
    "file": "FILE line (sheets, cells, tables)", "time": "TIME line", "sheets": "SHEETS without tables",
    "question": "QUESTION line (B150: the tables the question matches)",
    "brief": "'tables listed briefly' note", "table": "table head line (T<n> ← range · rows · totals; brief: cols)",
    "types": "part: [type · role · range] after each column name", "table_cols": "part: brief 'N cols: …' list on table lines",
    "table_totals": "part: total-row labels on table lines",
    "columns": "columns line (names, types, roles, ranges)", "units": "units: amounts in thousands …",
    "column_notes": "column notes (footnotes defining columns)", "entity": "entity / ID–name lines (B2/B18/B80)",
    "share": "share-of-total SQL template (B19)", "values": "<col> values: …", "mixed": "rows mix measures",
    "long": "T<n>_long line, 'is the long form of'", "no_sum": "do not sum: …",
    "table_status": "group column / totals: n ok / warnings: (per table)", "total_rows": "total rows: SELECT … _row_type …",
    "signs": "signs: … all negative", "inline": "inline rows (all N rows, CSV)", "numbers": "numbers: … / different measures",
    "quality": "quality: …", "formulas": "FORMULAS line", "totals": "TOTALS line", "notes": "NOTES line",
    "warnings": "WARNINGS line", "sql": "SQL view(s) T1 / T1_all tail", "sql_long": "SQL long view(s) tail",
    "union": "SQL union view U<n> lines", "files": "FILES / JOIN KEYS (multi-file)", "cut": "… cut: n more tables",
    "timing": "(normalized in n s)", "other": "unclassified",
}


def write(out: Path, results: list[dict], ablation: list[dict], partial: bool) -> None:
    all_checks = [x for r in results for x in r["checks"]]
    n = len(all_checks)
    counts = {k: sum(x["cat"] == k for x in all_checks) for k in CATS}
    suff = counts["a"] + counts["b"]
    tokens = sum(r["tokens"] for r in results)
    summary = {"tokenizer": TOKENIZER, "cases": len(results), "checks": n, "counts": counts,
               "sufficiency": round(suff / n, 4) if n else 0, "catalog_tokens": tokens,
               "sufficient_per_1k_tokens": round(1000 * suff / tokens, 3) if tokens else 0}
    (out / "results.json").write_text(json.dumps({"summary": summary, "cases": results, "ablation": ablation},
                                                 ensure_ascii=False, indent=1) + "\n")
    if partial:
        print(json.dumps(summary))
        return
    L = ["# Information scoreboard (model-free)", "",
         "Generated by [score.py](score.py); the method is in [README.md](README.md). For each automatic check",
         "of [eval/agents/cases.json](../agents/cases.json): **a** the value is in the catalog, **b** one query",
         "away (the reference query uses only names the catalog shows and reproduces the value), **c** reachable",
         "with more work, **d** wrong or missing. Sufficiency = (a + b) / checks.", "",
         f"Tokens: {TOKENIZER}. Binary: `hedgehog mcp` from $HEDGEHOG_BIN.", "",
         "## Overall", "",
         "| Checks | a | b | c | d | Sufficiency | Catalog tokens | Sufficient checks per 1k tokens |",
         "|---:|---:|---:|---:|---:|---:|---:|---:|",
         f"| {n} | {counts['a']} | {counts['b']} | {counts['c']} | {counts['d']} | **{100 * suff / n:.0f} %** | {tokens:,} | {summary['sufficient_per_1k_tokens']:.2f} |",
         "", "## Per case", "",
         "| Case | Checks | a | b | c | d | Sufficiency | Tokens | Per 1k tokens |", "|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for r in results:
        cs = r["checks"]
        k = {c: sum(x["cat"] == c for x in cs) for c in CATS}
        s = k["a"] + k["b"]
        L.append(f"| {r['case']} | {len(cs)} | {k['a']} | {k['b']} | {k['c']} | {k['d']} | {100 * s / len(cs):.0f} % | "
                 f"{r['tokens']:,} | {1000 * s / r['tokens']:.2f} |")
    L += ["", "## Traps", "",
          "Warnings that prevent a wrong answer, which the checks above cannot see (`trap_facts`: do not sum,",
          "total rows that stay in the data, grand totals, signs, mixed or look-alike measures, scales,",
          "COUNT(DISTINCT …) keys, empty dates, duplicates); results.json lists them per case, so two runs can",
          "be compared. Trap checks are case-specific patterns the catalog must show.", "",
          "| Case | Trap facts | Trap checks |", "|---|---:|---|"]
    for r in results:
        tc = "; ".join(("✓ " if x["ok"] else "✗ ") + x["what"] for x in r.get("trap_checks", []))
        L.append(f"| {r['case']} | {len(r.get('traps', []))} | {tc} |")
    L += ["", "## Gaps (c and d)", "", "| Case | Check | Cat | Reason |", "|---|---|---|---|"]
    for r in results:
        for x in r["checks"]:
            if x["cat"] in "cd":
                L.append(f"| {r['case']} | {x['check']} | {x['cat']} | {x['reason'].replace('|', '/')} |")
    if ablation:
        L += ["", "## Ablation: what each kind of catalog line is worth", "",
              "Each kind's lines are removed from every catalog and all checks re-scored (no model). *Lost* = checks",
              "that were a/b and are not any more. Tokens are summed per line over the 30 catalogs.", "",
              "| Kind | What | Tokens | Share | Checks lost | Tokens per lost check | Moves | Lost checks |",
              "|---|---|---:|---:|---:|---:|---|---|"]
        for a in sorted(ablation, key=lambda a: (-len(a["lost"]), -a["tokens"])):
            per = f"{a['tokens'] / len(a['lost']):,.0f}" if a["lost"] else "∞"
            moves = ", ".join(f"{k} {v}" for k, v in sorted(a["moved"].items()))
            lost = ", ".join(a["lost"][:6]) + (f" +{len(a['lost']) - 6}" if len(a["lost"]) > 6 else "")
            L.append(f"| {a['kind']} | {KIND_DESC.get(a['kind'], '')} | {a['tokens']:,} | {100 * a['tokens'] / tokens:.1f} % | "
                     f"{len(a['lost'])} | {per} | {moves} | {lost} |")
    L += ["", "## Every check", "", "| Case | Check | Cat | Reason |", "|---|---|---|---|"]
    for r in results:
        for x in r["checks"]:
            L.append(f"| {r['case']} | {x['check']} | {x['cat']} | {x['reason'].replace('|', '/')[:200]} |")
    (out / "REPORT.md").write_text("\n".join(L) + "\n")
    print(json.dumps(summary))


if __name__ == "__main__":
    main()
