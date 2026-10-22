# Information scoreboard (model-free)

hedgehog is a tool call inside an agent. Its main job is to give the agent a short, correct summary
of the file (the `open_file` catalog) and cheap exact access (`query`, `cells`, …), so that the
agent spends few tokens. A frontier model will always reason better than a small one. What hedgehog
controls is the information it hands over, and this folder measures that information without a model.

The agent comparisons in [eval/agents](../agents/README.md) measure answers, so they mix two
things: what the catalog says and how well a model reads it. This scoreboard measures only the
first. For every automatic check of the 30 cases in [cases.json](../agents/cases.json), it asks
whether the catalog alone, or the catalog plus one query, gives the reference answer.

Results: [REPORT.md](REPORT.md) (generated) and [results.json](results.json).

## Categories

Every `num`, `pct_range` and `text_any` check is put into one of four categories. `judge` checks are
out of scope because they have no reference.

| Cat | Meaning | Decided by |
|---|---|---|
| **a** | **in the catalog** | the reference value is in the catalog text, within the check's `tol` (or inside its `pct_range`), on a line that names it: the line contains one of the reference query's string literals or the check's `near` words, or the number has at least 4 significant digits. Numbers are read the way the answer checks read them (`run.numbers`). CSV inline rows are read field by field. Units count: when the reference multiplies by a literal (`* 1000`, `* 1e9`), the raw value counts. For a `text_any` answer, the phrase must be in the catalog and the number it belongs to (`pair`) must be **a**. |
| **b** | **one query away** | the reference query (or the probe, see below) uses only tables, columns and literal values that the catalog names, and running it reproduces the value |
| **c** | **reachable with more work** | the value is in hedgehog's tables, but the query needs a name the catalog does not show, so the agent needs `describe_table`, a lookup query or several queries |
| **d** | **wrong or missing** | the reference or natural query fails or gives another value on hedgehog's tables; or the data rows view (`T<n>`) gives an aggregate other than `T<n>_all`, which means rows the catalog calls totals are data; or a referenced source cell is in no table (lost) |

**Sufficiency** = (a + b) / checks. **Information per 1k tokens** = (a + b) / (catalog tokens / 1000).

### When the catalog "names" something

The SQL is tokenized with a small SQLite tokenizer (`sql_tokens`, `sql_refs`). Aliases (`AS x`, a
word right after an expression or a table) are not columns. String arguments of functions
(`strftime('%m', …)`, `REPLACE(col, ' ', '')`) are not filter values, and neither are literals
compared with `strftime(…)`, because they follow from the SQL, not the data.

- **Tables**: `T<n>` needs its head line, or a run line that covers it (`T2..T10 same columns as
  T1 · …`). `T<n>_all` needs either the name itself or the SQL tail range (`T1_all..T7_all`). `T<n>_long` and `U<n>` need their names. `f<k>.T<n>` also needs the
  `FILES` line of a multi-file session.
- **Columns**: a quoted name in the catalog, a `name[type]` item of a columns line, an item of a
  brief `N cols:` list, a field of an inline CSV header, a `<col> values:` line, a name inside a
  range `"2011".."2025"` or `"Thu / 2002-01-31".."Tue / 2002-01-01"` (same shape, inside the
  range), a shared-prefix group `solar_{electricity, share_elec}`, a group of header levels
  (`"In jeweiligen Preisen / {Ursprungswerte / {Mrd. EUR, %}, …}"`, `"{a, b} / % des BIP"`, members
  separated by `; ` when one has a comma: `expand_braces`), or a reference to an earlier table's
  listed columns (`Jahr + T27's "Einnahmen / insgesamt".."Finanzierungs-saldo", each + " / % des
  Bruttoinlandsprodukts"`). View columns (`_row_type`,
  `_src_row`, `period`, `value`, `sheet`, `variant`, …) count when the catalog mentions them.
  Unquoted names are matched case-insensitively, as SQLite does.
- **String literals** (`col = 'x'`, `IN (…)`, `LIKE`), in this order:
  1. *in the catalog*: quoted (`"x"` or `'x'`) or a field of an inline row; a non-numeric literal
     of 4 or more characters may also appear anywhere as a whole word;
  2. *in the question*: a non-numeric literal that is a whole word of the case prompt (the agent has
     it from the user);
  3. *a date or period* (`2018-03-07`, `2026-S1`, `2023%`) whose year is inside a range the catalog
     states and whose shape the catalog shows;
  4. *in the first 5 rows of a ranking query on its column*: for a reference
     `SELECT <expr> FROM T WHERE … col = 'x' …`, score.py runs the query an agent would run for
     "which col has the most/least": the condition is dropped, then the rows are ranked by `<expr>`
     (grouped when `<expr>` aggregates), both ways, `LIMIT 5`. A value in those rows counts for
     every check of the case. A `LIKE` pattern counts when it matches such a value.
- **Numbers compared with a column**: `0`, a year inside a stated range, a number shown in the
  catalog (`'Gesamt'@9` shows 9), or a value from a ranking query.

### Probes and alternative queries

Some references are not SQL over hedgehog's tables, or filter on a code that the question does not
give. For these, `PROBES` in score.py holds a hand-written natural query, which is scored exactly
like a reference:

- `ref_py` checks (uk-gender-pay-gap, values read from the raw CSV) and `ref_cells` checks
  (enron-mdea-pl, pt-social-report, uk-ons-labour) get the query an agent would write. If a probe
  fails, the referenced cells are looked up in every table with a second `hedgehog mcp` session
  (`scan_cells`). The check is **c** if every cell is in some table and **d** if one is in none.
- `text_any` checks (a day, a name, a period) get the ranking query that answers them. `pair`
  names the number check the answer belongs to.
- `alt`: a natural alternative when the reference filters on a code (wb-gdp-wide ISO codes,
  fr-insee-pop département numbers, nw-orders order-ID lists). It counts only when the reference
  itself is **c**, and only if it reproduces the value.

Every probe was written once against the case's question and is kept in the script, so a rerun is
deterministic.

## Traps

The checks measure naming; they cannot see a line whose job is to prevent a wrong answer.
`trap_facts` lists those warnings per catalog as facts (`kind:table[:detail]`): columns not to add
up (`do not sum:` items and `[rate]` / `[avg]` / `[index]` column items, also inside brace groups),
total rows that stay in the data (exclude before summing), grand totals by label, signs, mixed and
look-alike measures, scales, COUNT(DISTINCT …) keys, empty dates, duplicates, formulas without a
value and boolean filters. A run line (`T2..T10 …`) counts for each of its tables. results.json
keeps them per case, so a change to the catalog can be checked for lost traps by comparing two
runs. `TRAP_CHECKS` holds case-specific patterns the catalog must show; the report lists them. The
first one is B105: at-tourism-2months must name `'Insgesamt'` as the grand total and flag
`'Österreich gesamt'` (guests from Austria) as the total of one group, not of the table.

## Tokens

Catalog tokens are counted with **tiktoken `o200k_base`** when it can be imported. Otherwise score.py
uses ⌈characters / 4⌉, and the report says which one it used. tiktoken is not a dependency of the
repo. Install it in an isolated venv:

```sh
python3 -m venv /path/to/venv && /path/to/venv/bin/pip install tiktoken
```

Per-kind tokens are summed line by line, each line with its newline. The overall figure is the token
count of the whole catalog text. A multi-file case counts every file's catalog.

## Ablation (what each line kind is worth)

`line_kinds` gives every catalog line a kind. The producers are agent/src/catalog.rs, facts.rs,
entity.rs, views.rs, types.rs, unions.rs and multi.rs. The kinds are FILE, TIME, SHEETS, table head
line, columns, units, column notes, entity, share, values, mixed measures, long view, do not sum,
table status, total rows, signs, inline rows, numbers, quality, FORMULAS, TOTALS, NOTES, WARNINGS,
the SQL / long / union view tails, FILES/JOIN KEYS, cut, and timing. Three parts of lines are
ablated the same way: `types` (the `[…]` after each column name), `table_cols` (the brief `N cols:`
lists) and `table_totals` (the total-row labels on table lines).

For each kind, its lines are removed from every catalog and all checks are scored again. The cached
query results are reused, so no hedgehog call and no model is needed. *Checks lost* counts the
checks that were a or b and are not any more.

What this ablation can and cannot see:

- It measures **naming**: whether the agent can find the table, column, value or number. It does
  not see lines whose job is to prevent a wrong answer (do not sum, signs, mixed measures, total
  rows that stay in T1, quality, different measures). A kind that loses 0 checks here may still win
  checks with a model. The optional LLM run below tests that.
- Kinds are removed one at a time, so two kinds that name the same thing (the `long` line and the
  `SQL long view` tail) each look free.
- The parts (`types`, `table_cols`, `table_totals`) overlap with their line kinds, so the shares do
  not add up to 100 %.

### LLM check of three low-value kinds (2026-10-22)

The ablation found three kinds with the most tokens and no lost check: `types` (3,274 tokens),
`table_totals` (3,290) and `quality` (1,376), together 15 % of the catalog tokens. To test them with
a model, `HH_HIDE_KINDS` (a test-only switch in [swap/tools.py](../agents/swap/tools.py)) removes
these kinds from `open_file` in the `swap-hedgehog` arm. Setup: gpt-4.1-mini, 3 runs per case, 6
cases with the most tokens of these kinds (pt-contracts is left out for memory), binary v0.4.0.

| Case | Full catalog | Without types, table_totals, quality |
|---|---:|---:|
| enron-henry-hub | 18/24 | 23/24 |
| us-superstore | 14/18 | 13/18 |
| uk-gender-pay-gap | 14/15 | 13/15 |
| pt-social-report | 17/21 | 18/21 |
| at-tourism-2months | 6/18 | 8/18 |
| owid-energy | 15/15 | 15/15 |
| **Total** | **84/111 (76 %)** | **90/111 (81 %)** |
| Input tokens (18 runs) | 399k | 252k |
| Cost | $0.084 | $0.062 |

Hiding the three kinds lost nothing and used 37 % fewer input tokens. Most of the token saving is
pt-social-report (167k → 27k), where the model made fewer exploratory calls. With 3 runs per case,
the +6 checks are within noise. This supports shortening these kinds (B104), but it does not prove
that they are useless: their trap warnings (do not sum, empty dates) matter in cases outside this
sample. at-tourism-2months scores 100 % here and 6–8/18 with the model, because the model reads
`'Österreich gesamt'` (guests from Austria) as Austria's total. This scoreboard cannot see that kind
of trap (B105).

Command (run under `scripts/run_capped.sh 4G …`, with `SWAP_DIR` on disk):
`HH_HIDE_KINDS=types,table_totals,quality python3 -I eval/agents/run.py --arms swap-hedgehog --repeat 3
--cases … --hedgehog <bin> --out-dir <dir>`.

### After B100–B105 (2026-10-22)

The catalog changes that followed (shorter column items, total-row labels once, brief catalogs by
detail levels, runs of repeated tables, B102/B103/B105, inline cap; see the CHANGELOG) moved the
scoreboard from 159/168 (95 %) at 53,867 tokens to 168/168 (100 %) at 42,360 tokens (−21 %, 3.97
sufficient checks per 1k tokens), with no case lower and no trap fact lost against the v0.4.0
catalogs (compared with `trap_facts` on both runs' cached sessions).

## Running

```sh
# from the repo root; tier 1 (one hedgehog session per case, ~1 GB, pt-contracts needs 4G)
../scripts/run_capped.sh 4G /tmp/info.log env HEDGEHOG_BIN=target/release/hedgehog \
  /path/to/venv/bin/python -I eval/info/score.py --cache /some/disk/dir
python3 -I eval/info/score.py --offline --cache /some/disk/dir     # re-score from the cached sessions
python3 -I eval/info/score.py --cases de-gdp,owid-energy --no-ablation
python3 -I eval/info/test_score.py                                  # unit tests (no hedgehog)
```

`--cache` keeps every session's raw tool output as JSON: the catalogs, reference and probe results,
data-view and ranking queries, and cell scans. The default is `eval/info/cache/`, which is ignored
by git. A run with `--cases` writes results.json for those cases only and leaves REPORT.md as it is.

## Limits

- A value in the catalog counts even when it is only a range bound (`annual_salary … 40008..360504`
  gives the top salary). The value is there, but the agent may not know that it is the answer.
- A ranking query that shows a value counts as "one query", even though the agent may need a second
  query to use it in a filter. The rule is limited to the first 5 rows so that a value does not
  qualify by chance.
- "Contradictions" are found only through query results: the data view against `_all` for
  aggregates, probes against the raw file, and cell scans. A wrong sentence in the catalog that no
  query exposes is not detected.
