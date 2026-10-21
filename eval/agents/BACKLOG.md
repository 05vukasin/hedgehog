# Backlog from agent comparisons

Each item comes from a case in [cases.json](cases.json) where an agent using hedgehog got something
wrong, was slow, or needed more steps than it should have. An item is **done** only when the case it
names passes its checks in a fresh run ([RESULTS.md](RESULTS.md)) and the library's gates still hold
(tests, golden check, no benchmark drop).

Status: `open` · `in progress` · `done (run id)`.

## Phase 1: correctness bugs (library)

| ID | Problem (found in) | Fix | Done when | Status |
|---|---|---|---|---|
| B1 | 38 contracts taken for **total rows** because a free-text column says "total" ("Prótese total anca"); 63 more stay in the table but are flagged "exclude before summing" (pt-contracts) | in flat registers, a keyword in a non-label column is not enough: a total row must also add up, or carry the keyword in the row-label column | pt-contracts: `T1` has all 176,512 rows, no false total rows; benchmarks no drop | in progress (agent: totals) |
| B6 | Small tables: the model reported the single-recipient row as the total in one run, and read the wrong cells for the 2025 averages in the baseline (€287.27 instead of €403.19). The table has 12 rows but 19 columns, so it is not inlined (limit 12 columns), and total rows are never inlined | inline small tables by a cell budget (e.g. ≤ 300 cells) instead of a column limit, from `T<n>_all` with `_row_type`; the catalog names each total row with its values | at-pension 4/4 in 3 runs out of 3 | in progress (agent: catalog) |
| B12 | 8 of 15 tool calls failed: the model queried a `"Total"` column, but total rows are rows (`_row_type = 'total'`, label in `col_C`) (enron-henry-hub) | catalog says how to select total rows (`SELECT … FROM T1_all WHERE _row_type = 'total'`); a "no such column" error whose name matches a total-row label says so | enron-henry-hub with 0 tool errors | SQL part done (pending rerun): `no such column: "Total"` → "is a row label, not a column: total rows are in T1_all with _row_type = 'total' (label in col_C): SELECT * FROM T1_all WHERE _row_type = 'total'"; a data-row label ("SEA ROBIN") names its column; catalog line: in progress (agent: catalog) |
| B5 | `MONTH()` error: the model writes MySQL/Postgres SQL (pt-contracts) | SQL errors for unknown functions name the SQLite replacement (`MONTH(x)` → `strftime('%m', x)`, `YEAR`, `DATE_FORMAT`, `EXTRACT`, `TO_CHAR`, `ILIKE`, `TOP n`); tool description says "SQLite" | no dialect errors across the cases | done (pending rerun): unknown functions, `EXTRACT`, `TOP n`, `ILIKE`, `::`, `FETCH FIRST`, `INTERVAL` errors name the SQLite way (`agent/src/dialect.rs`); query tool description says SQLite with strftime examples |

## Phase 2: catalog intelligence (what the agent needs to know before it queries)

| ID | Problem (found in) | Fix | Done when | Status |
|---|---|---|---|---|
| B2 | Same entity under many spellings: `"<tax id> - <name>"` with up to 6 names per ID; the model grouped by text and listed one body twice (pt-contracts) | detect ID-plus-name columns; catalog line with how many IDs have several spellings and the SQL to group by the ID (or a ready `<col>_id` column in the SQL view) | pt-contracts `variants` and `top-body` pass with gpt-4o-mini | done (pending rerun): `adjudicante_id`/`_name` (and `adjudicatarios`, `CPV`, `NUTs`) in T1/T1_all, catalog + describe_table line (`agent/src/entity.rs`); `GROUP BY adjudicante_id` gives 503933813 €737.7 M, 503278602 €352.4 M |
| B3 | Two similar price columns; the model picked the effective price (€4.28 bn) instead of the contract price (€11.16 bn) (pt-contracts) | catalog shows each numeric column's total, fill rate and range; similar numeric columns get a one-line "differs from" note | `price-column`, `ad-rg-value`, `cp-value` pass | in progress (agent: catalog) |
| B4 | Data-quality facts only a strong model found by exploring: 20 duplicate IDs, 1,087 multi-winner rows, 26,723 rows without a signing date, negative prices (pt-contracts) | catalog "quality" line: duplicates in ID-like columns, empty share of date columns (and which category they belong to), negative values in amount columns, multi-value cells | the hedgehog answer mentions the missing signing dates when counting by month | in progress (agent: catalog) |

## Phase 3: structure

| ID | Problem (found in) | Fix | Done when | Status |
|---|---|---|---|---|
| B8 | Daily grids T3–T7 lose their row labels: the pipeline names sit in the left block and go to `Notes` as repeated headers (enron-henry-hub) | side-by-side and stacked blocks that share the row labels of a neighbouring block get those labels as a key column | "which pipeline had the highest single-day receipts in January" is answerable by SQL | in progress (agent: layout) |

## Phase 4: the agent (hedgehog-cli) and the tool pool

| ID | Problem (found in) | Fix | Done when | Status |
|---|---|---|---|---|
| B7 | 12 turns, 11 tool calls and 50k tokens for one number (at-pension) | system prompt: answer from the inline table when it is there; one query per sub-question; stop when answered | at-pension in ≤ 4 turns | in progress (agent: hedgehog-cli) |
| B11 | Judgement slips: "most contracts" ranked by value, a +6 % month called "a jump", a computation left undone, placeholder rows ("Another body … not named") (pt-contracts); net flow computed as receipts − (−deliveries) (enron-henry-hub) | system-prompt rules: count vs value, compare against the other periods before calling a change, finish every sub-question, never invent placeholder rows; catalog notes sign conventions (a block of negative values) | pt-contracts `by-count`, `covid`, `top10-share` pass | in progress (agent: hedgehog-cli + catalog) |
| B9 | gpt-4o-mini is the weak link on multi-part questions | run every case with 2–3 candidate models; choose the default by checks passed per dollar | a default model with the best score/cost on all cases | open |
| B13 | gpt-4o-mini answers vary between runs: pt-contracts scored 4/13 in the manual run and 9/13 in the baseline (eval) | run each hedgehog case 3 times and report the mean and the worst run; a fix counts only if the worst run passes | RESULTS.md shows mean / worst for hedgehog | done in the runner (`--repeat 3`, `--out-dir`); used from the next run on |
| B10 | Claude Code reads the sheet with its own pandas (3 min) | third arm `claude-hh`: Claude Code with `hedgehog mcp`; then package it as a Claude skill/plugin | claude-hh matches claude's checks in less time and fewer tokens | in progress (agent: Claude plugin) |
