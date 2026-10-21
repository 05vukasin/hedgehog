# Backlog from agent comparisons

Each item comes from a case in [cases.json](cases.json) where an agent using hedgehog got something
wrong, was slow, or needed more steps than it should have. An item is **done** only when the case it
names passes its checks in a fresh run ([RESULTS.md](RESULTS.md)) and the library's gates still hold
(tests, golden check, no benchmark drop).

Status: `open` · `in progress` · `done (run id)`.

## Phase 1: correctness bugs (library)

| ID | Problem (found in) | Fix | Done when | Status |
|---|---|---|---|---|
| B1 | 38 contracts taken for **total rows** because a free-text column says "total" ("Prótese total anca"); 63 more stay in the table but are flagged "exclude before summing" (pt-contracts) | in flat registers, a keyword in a non-label column is not enough: a total row must also add up, or carry the keyword in the row-label column | pt-contracts: `T1` has all 176,512 rows, no false total rows; benchmarks no drop | open |
| B6 | Inline small tables leave out total rows, so the model never saw "Gesamt" and reported the single-recipient row as the total (at-pension) | inline CSV of small tables comes from `T<n>_all` with a `_row_type` column; the catalog names the total row and its values | at-pension 4/4 | open |
| B5 | `MONTH()` error: the model writes MySQL/Postgres SQL (pt-contracts) | SQL errors for unknown functions name the SQLite replacement (`MONTH(x)` → `strftime('%m', x)`, `YEAR`, `DATE_FORMAT`, `EXTRACT`, `TO_CHAR`, `ILIKE`, `TOP n`); tool description says "SQLite" | no dialect errors across the cases | open |

## Phase 2: catalog intelligence (what the agent needs to know before it queries)

| ID | Problem (found in) | Fix | Done when | Status |
|---|---|---|---|---|
| B2 | Same entity under many spellings: `"<tax id> - <name>"` with up to 6 names per ID; the model grouped by text and listed one body twice (pt-contracts) | detect ID-plus-name columns; catalog line with how many IDs have several spellings and the SQL to group by the ID (or a ready `<col>_id` column in the SQL view) | pt-contracts `variants` and `top-body` pass with gpt-4o-mini | open |
| B3 | Two similar price columns; the model picked the effective price (€4.28 bn) instead of the contract price (€11.16 bn) (pt-contracts) | catalog shows each numeric column's total, fill rate and range; similar numeric columns get a one-line "differs from" note | `price-column`, `ad-rg-value`, `cp-value` pass | open |
| B4 | Data-quality facts only a strong model found by exploring: 20 duplicate IDs, 1,087 multi-winner rows, 26,723 rows without a signing date, negative prices (pt-contracts) | catalog "quality" line: duplicates in ID-like columns, empty share of date columns (and which category they belong to), negative values in amount columns, multi-value cells | the hedgehog answer mentions the missing signing dates when counting by month | open |

## Phase 3: structure

| ID | Problem (found in) | Fix | Done when | Status |
|---|---|---|---|---|
| B8 | Daily grids T3–T7 lose their row labels: the pipeline names sit in the left block and go to `Notes` as repeated headers (enron-henry-hub) | side-by-side and stacked blocks that share the row labels of a neighbouring block get those labels as a key column | "which pipeline had the highest single-day receipts in January" is answerable by SQL | open |

## Phase 4: the agent (hedgehog-cli) and the tool pool

| ID | Problem (found in) | Fix | Done when | Status |
|---|---|---|---|---|
| B7 | 12 turns, 11 tool calls and 50k tokens for one number (at-pension) | system prompt: answer from the inline table when it is there; one query per sub-question; stop when answered | at-pension in ≤ 4 turns | open |
| B11 | Judgement slips: "most contracts" ranked by value, a +6 % month called "a jump", a computation left undone (pt-contracts) | system-prompt rules: count vs value, compare against the other periods before calling a change, finish every sub-question | pt-contracts `by-count`, `covid`, `top10-share` pass | open |
| B9 | gpt-4o-mini is the weak link on multi-part questions | run every case with 2–3 candidate models; choose the default by checks passed per dollar | a default model with the best score/cost on all cases | open |
| B10 | Claude Code reads the sheet with its own pandas (3 min) | third arm `claude-hh`: Claude Code with `hedgehog mcp`; then package it as a Claude skill/plugin | claude-hh matches claude's checks in less time and fewer tokens | open |
