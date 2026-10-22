"""Unit tests for score.py (no hedgehog calls): python3 -I -m unittest discover -s eval/info"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import score as S  # noqa: E402

CATALOG = """FILE x.xlsx · 1 sheet · 244 cells (97% in tables) · 1 table · roles: model
TIME 2011–2025 (year, from column headers in T1)
T1  'Title' ← '2_2'!A3:R18 · 7 rows (+5 total rows: 'Gesamt'@9 — only in T1_all)
    columns (6): "group"[text group] Richtsatz[text · dim] "2011".."2025"[15 × number · mixed] Datum[date · period 2019-06-18..2026-10-09]
    Richtsatz values: "Alleinstehend" | "Verheiratet ***"
    T1_long: one row per ("group", Richtsatz, period) — 15 columns "2011".."2025" as rows: period '2011'..'2025', value
    do not sum: rows "group" = 'Durchschnittshöhe in Euro' (avg EUR) — use AVG
    group column: "group" · totals: 51 ok, 0 mismatch, 47 unknown
    total rows: SELECT … FROM T1_all WHERE _row_type IN ('total', 'of_which') — labels in Richtsatz: totals 'Gesamt'@9
    all 2 rows (CSV, as in T1_all; rows with _row_type other than data are not in T1):
    group,Richtsatz,2024,2025,_row_type
    Anzahl Bezieher:innen,Gesamt,"190,423",189537,total
    quality: Datum empty in 3 rows (1 %)
TOTALS 51 ok, 0 mismatch, 47 unknown (call totals for details)
NOTES none
WARNINGS none
SQL view T1 (data rows) and T1_all (+ total rows, _row_type); both have _src_row; quote names: SELECT "col name" FROM T1
SQL long view T1_long (data rows) and the same with _long_all (+ total rows, _row_type): period columns as rows
(normalized in 0.0 s)"""


class SqlRefs(unittest.TestCase):
    def test_tables_columns_literals(self):
        r = S.sql_refs("SELECT \"2024\" FROM T1_all WHERE Richtsatz = 'Gesamt' AND _row_type = 'total'")
        self.assertEqual(r["tables"], ["T1_all"])
        self.assertEqual(r["columns"], ["2024", "Richtsatz", "_row_type"])
        self.assertEqual(r["literals"], [("Richtsatz", "=", "Gesamt"), ("_row_type", "=", "total")])

    def test_quoted_sql_word_is_a_column(self):
        r = S.sql_refs("SELECT \"2025\" FROM T1_all WHERE \"group\" = 'Durchschnittshöhe in Euro'")
        self.assertIn("group", r["columns"])
        self.assertEqual(r["literals"], [("group", "=", "Durchschnittshöhe in Euro")])

    def test_aliases_are_not_columns(self):
        r = S.sql_refs("SELECT 100.0 * SUM(v) / (SELECT SUM(p) FROM T1) FROM (SELECT SUM(p) v FROM T1 "
                       "GROUP BY COALESCE(a_id, a_name) ORDER BY v DESC LIMIT 10)")
        self.assertNotIn("v", r["columns"])
        self.assertEqual(set(r["columns"]), {"p", "a_id", "a_name"})

    def test_function_arguments_and_strftime(self):
        r = S.sql_refs("SELECT SUM(x) FROM T1_all WHERE REPLACE(Trader, ' ', '') = 'McKay' "
                       "AND strftime('%m', d) = '01' AND ROUND(y, 2) > 0")
        self.assertEqual(r["literals"], [("Trader", "=", "McKay")])
        self.assertNotIn(("y", "2"), r["numbers"])

    def test_case_when_and_like_and_in(self):
        r = S.sql_refs("SELECT 100.0 * SUM(CASE WHEN k = 'Порези' THEN v END) / SUM(v) FROM T1_all "
                       "WHERE UPPER(s) LIKE '%COUNCIL%' AND id IN (10403, 10417) AND jahr = 2024")
        self.assertIn(("k", "=", "Порези"), r["literals"])
        self.assertIn(("s", "like", "%COUNCIL%"), r["literals"])
        self.assertEqual(r["numbers"], [("id", "10403"), ("id", "10417"), ("jahr", "2024")])

    def test_join_across_files(self):
        r = S.sql_refs("SELECT SUM(b.q) FROM f1.T1 AS a JOIN f2.T1 AS b ON a.orderID = b.orderID WHERE a.d LIKE '1997%'")
        self.assertEqual(r["tables"], ["f1.T1", "f2.T1"])
        self.assertEqual(set(r["columns"]), {"q", "orderID", "d"})


class Catalog(unittest.TestCase):
    def test_line_kinds(self):
        kinds = S.line_kinds(CATALOG)
        lines = CATALOG.split("\n")
        got = {l.split()[0] if l.strip() else "": k for l, k in zip(lines, kinds)}
        self.assertEqual(kinds[0], "file")
        self.assertEqual(kinds[2], "table")
        self.assertEqual(kinds[3], "columns")
        self.assertEqual(kinds[4], "values")
        self.assertEqual(kinds[5], "long")
        self.assertEqual(kinds[6], "no_sum")
        self.assertEqual(kinds[7], "table_status")
        self.assertEqual(kinds[8], "total_rows")
        self.assertEqual(kinds[9:12], ["inline"] * 3)
        self.assertEqual(kinds[12], "quality")
        self.assertEqual(kinds[-3:], ["sql", "sql_long", "timing"])
        self.assertNotIn("other", kinds, got)
        self.assertNotIn("Richtsatz values", S.strip_kinds(CATALOG, {"values"}))

    def test_index_names(self):
        idx = S.CatalogIndex(CATALOG)
        self.assertTrue(idx.has_table("T1") and idx.has_table("T1_all") and idx.has_table("T1_long"))
        self.assertFalse(idx.has_table("T2") or idx.has_table("f2.T1"))
        self.assertTrue(idx.has_column("2019", True))  # inside "2011".."2025"
        self.assertFalse(idx.has_column("2030", True))
        self.assertTrue(idx.has_column("richtsatz", False))  # unquoted names are case-insensitive
        self.assertFalse(idx.has_column("richtsatz", True))
        self.assertTrue(idx.has_column("_row_type", True))
        self.assertTrue(idx.year_covered(2023) and not idx.year_covered(2030))

    def test_range_template_with_weekdays(self):
        idx = S.CatalogIndex('    columns (2): "Thu / 2002-01-31".."Tue / 2002-01-01"[31 × integer]')
        self.assertTrue(idx.has_column("Wed / 2002-01-23", True))
        self.assertFalse(idx.has_column("Wed / 2002-02-23", True))

    def test_shared_prefix_columns(self):
        idx = S.CatalogIndex("    columns (130; a_{b, c} = a_b, a_c): country[text · name] "
                             "solar_{cons_change_pct[number · rate], electricity, share_elec[number]}[8 × number]")
        self.assertTrue(idx.has_column("solar_electricity", False) and idx.has_column("solar_share_elec", False))
        self.assertFalse(idx.has_column("solar_x", False))

    def test_grouped_header_levels(self):
        line = ('T1  \'1.1\' ← \'1.1\' · 35 rows · 5 cols: Jahr "In jeweiligen Preisen / {Ursprungswerte / {Mrd. EUR, %}, '
                'X13 / Mrd. EUR}" "Preisbereinigt, verkettet / {Ursprungswerte / {2020 = 100, %}; Werte / x}"')
        idx = S.CatalogIndex(line)
        for name in ("Jahr", "In jeweiligen Preisen / Ursprungswerte / Mrd. EUR", "In jeweiligen Preisen / X13 / Mrd. EUR",
                     "Preisbereinigt, verkettet / Ursprungswerte / %", "Preisbereinigt, verkettet / Werte / x"):
            self.assertTrue(idx.has_column(name, True), name)
        self.assertEqual(S.expand_braces("{a, b / {c, d}} / % des BIP"), ["a / % des BIP", "b / c / % des BIP", "b / d / % des BIP"])

    def test_columns_by_reference_and_table_runs(self):
        text = ('T27 \'1.10\' ← \'1.10\' · 35 rows · 4 cols: "group" Jahr "Einnahmen / {insgesamt, Steuern}" Saldo\n'
                'T29 · 35 rows · 4 cols: Jahr + T27\'s "Einnahmen / insgesamt".."Saldo", each + " / % des BIP"\n'
                "T30..T33 same columns as T27 · 35 rows each · sheets: T30 a, T31 b, T32 c, T33 d")
        idx = S.CatalogIndex(text)
        self.assertTrue(idx.has_column("Einnahmen / Steuern / % des BIP", True))
        self.assertTrue(idx.has_column("Saldo / % des BIP", True))
        self.assertFalse(idx.has_column("group / % des BIP", True))
        self.assertTrue(idx.has_table("T32") and not idx.has_table("T34"))
        self.assertEqual(S.line_kinds(text), ["table", "table", "table"])

    def test_trap_facts(self):
        text = ("T1  'x' ← A!A1:C9 · 7 rows (+1 total row only in T1_all) · 1 total row stays in T1 — exclude it before summing\n"
                '    columns (3): Region "VPI {2025[index], 2020}" Rate[rate 1..2]\n'
                "    total rows: SELECT … FROM T1_all WHERE _row_type = 'total' — labels in Region: grand total 'Insgesamt'@4\n"
                "T2..T4 same columns as T1 · 7 rows each · 1 total row stays in each — exclude it before summing")
        facts = S.trap_facts(text)
        for f in ("kept:T1", "kept:T3", "grand:T1:Insgesamt", "nosum-col:VPI 2025", "nosum-col:Rate"):
            self.assertIn(f, facts)
        self.assertNotIn("nosum-col:VPI 2020", facts)
        ok = S.trap_checks("at-tourism-2months", "grand total 'Insgesamt'@4 · 'Österreich gesamt' each total one group of x, not the table")
        self.assertTrue(all(x["ok"] for x in ok))

    def test_literal_rules(self):
        idx = S.CatalogIndex(CATALOG)
        self.assertEqual(S.literal_ok(idx, "Richtsatz", "=", "Gesamt", "", set()), "in the catalog")
        self.assertIsNone(S.literal_ok(idx, "Richtsatz", "=", "Geschieden", "", set()))
        self.assertEqual(S.literal_ok(idx, "x", "=", "Paris", "the population of Paris?", set()), "in the question")
        self.assertIsNone(S.literal_ok(idx, "x", "=", "SOM", "Somalia's GDP", set()))  # whole words only
        self.assertIsNotNone(S.literal_ok(idx, "Datum", "like", "2023%", "", set()))
        self.assertIsNotNone(S.literal_ok(idx, "Datum", "=", "2024-03-07", "", set()))
        self.assertIsNotNone(S.literal_ok(idx, "s", "like", "%PREMER%", "", {"PREMER SAVKOVIĆ D.O.O."}))
        self.assertEqual(S.missing_names("SELECT \"2024\" FROM T1_all WHERE Richtsatz = 'Witwe'", idx, "", set()),
                         ["value 'Witwe' of Richtsatz"])
        self.assertEqual(S.missing_names("SELECT q FROM T2", idx, "", set()), ["table T2", 'column "q"'])

    def test_value_in_catalog(self):
        ch = {"num": 190423, "tol": 0.0001}
        self.assertIn("Gesamt", S.value_in_catalog(ch, CATALOG, ["Gesamt"], []))
        self.assertIsNone(S.value_in_catalog({"num": 3, "tol": 0}, CATALOG, [], []))  # unattributable small number
        self.assertIsNotNone(S.value_in_catalog({"num": 189537000, "tol": 0.001}, CATALOG, ["Gesamt"], [1000.0]))

    def test_data_view(self):
        self.assertEqual(S.data_view("SELECT a FROM T1_all JOIN T2_long_all JOIN U3_all"), "SELECT a FROM T1 JOIN T2_long JOIN U3")

    def test_top_probes(self):
        q = S.top_probes("SELECT SUM(p) FROM T1 WHERE id = '5039' AND y = 2024")
        self.assertIn("SELECT id FROM T1 WHERE 1=1 AND y = 2024 GROUP BY id ORDER BY SUM(p) DESC LIMIT 5", q)
        self.assertEqual(S.top_probes("SELECT 100.0 * SUM(CASE WHEN a = 'x' THEN v END) FROM T1"), [])


class Tokens(unittest.TestCase):
    def test_naive_traps(self):
        # pt-social: the naive SUM over total rows is wrong; the catalog line of T1 warns, T2's does not.
        session = {"naive:0": "SUM(TOTAL)\n14956\n(1 row)", "naive:1": "SUM(Total)\n1120\n(1 row)",
                   "naive:2": "SUM(TOTAL)\n7478\n(1 row)"}
        text = ("T1  'Quadro 1' · 10 rows · 8 total rows (only in T1_all): grand total 'Total' by Sexo @8–10: "
                "M 4,233 · F 3,245 · T 7,478 in TOTAL (T = M + F: don't add)\nT2  · 4 rows · 5 total rows in T2_all")
        got = [(x["ok"], x["how"]) for x in S.naive_checks("pt-social-report", text, session)]
        self.assertEqual(got, [(True, "the catalog warns"), (False, "wrong: the naive query gives 1120.0"),
                               (True, "the naive query is right")])
        # A text answer (uk-ons): right when the naive query gives it.
        ok = S.naive_checks("uk-ons-labour", "", {"naive:0": "col_A\nFeb-Apr 2026\n(1 row)"})
        self.assertTrue(ok[0]["ok"])
        self.assertFalse(S.naive_checks("uk-ons-labour", "", {"naive:0": "col_A\nApr-Jun 2026\n(1 row)"})[0]["ok"])

    def test_chars_fallback(self):
        self.assertEqual(S.count_tokens(""), 0)
        self.assertEqual(S.count_tokens("abcdefgh", chars_only=True), 2)
        self.assertEqual(S.count_tokens("abcdefghi", chars_only=True), 3)

    def test_tokenizer(self):
        n = S.count_tokens("SELECT \"2024\" FROM T1_all WHERE Richtsatz = 'Gesamt'")
        if S.TOKENIZER.startswith("tiktoken"):
            self.assertTrue(8 <= n <= 20, n)
        else:
            self.assertEqual(n, 13)


if __name__ == "__main__":
    unittest.main()
