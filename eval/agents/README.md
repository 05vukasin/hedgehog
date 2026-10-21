# Agent comparisons

The benchmarks in [docs/evaluation.md](../../docs/evaluation.md) measure whether hedgehog labels
cells correctly. This folder measures what users care about.

**The headline comparison is the weakest harness with the best tool against the best harness with
its own tools:** hedgehog-cli (a simple chat loop on a small, cheap model whose only tools are
hedgehog's) versus Claude Code (the strongest general agent harness on a frontier model, reading the
file with its own Python). When hedgehog-cli matches Claude Code, the tool is doing the work. The
third arm, Claude Code with hedgehog's plugin, is extra information, not the headline.

The question: **does an agent with hedgehog's tools
answer real questions about a real file correctly, quickly and cheaply, compared with a strong
general agent that has no hedgehog?**

## The method

1. **A case** is a file, a prompt and checks ([cases.json](cases.json)). The prompt is what an
   analyst would ask. The reference answers come from SQL over every row of the file (`T<n>_all`),
   checked by hand. [sql.py](sql.py) runs such queries.
2. **The same prompt goes to every agent**, as `File: <path>` (or `Files: a, b` for a multi-file case)
   plus the questions:
   - `hedgehog`: hedgehog-cli, one-shot, with its configured model;
   - `claude`: Claude Code with shell and file tools only (it writes its own pandas);
   - `claude-hh`: Claude Code with the [hedgehog plugin](../../integrations/claude-code): its
     `spreadsheets` skill (`--plugin-dir`) and `hedgehog mcp` (the binary under test, `--hedgehog`),
     plus the same shell and file tools. Its skill-listing budget is raised (`--skill-budget`, default
     80000) so the skill keeps its description when many user skills are installed.
3. **Every run is recorded** in [results.jsonl](results.jsonl): time, turns, tokens, cost, tool calls
   and errors, the answer, and the automatic checks. [RESULTS.md](RESULTS.md) shows the latest run per
   case and agent; the full answers are in [answers/](answers/).
4. **Judgement checks** (did it merge name variants, did it read the question right) are graded by
   a person: `run.py --grade <run id> covid=1 variants=0 --note "..."`.
5. **Every failure becomes a backlog item** ([BACKLOG.md](BACKLOG.md)) that names the case it came
   from. An item is done when that case passes in a fresh run.

## Running

**Claude Code runs once per case.** Its answer does not depend on hedgehog, so the `claude` arm is
recorded once per (file, prompt) and reused in every report; the runner skips cases that already
have one (`--rerun-claude` forces a new run, e.g. for a new Claude model). Only new or changed cases
cost a Claude run. The default arm is `hedgehog`; `claude-hh` depends on hedgehog and is rerun only
when needed.


```sh
# from the repo root; the whole run under a memory cap (Claude's pandas on 4.5M cells is heavy)
../scripts/run_capped.sh 6G /tmp/agents.log python3 eval/agents/run.py --repeat 3       # all cases, hedgehog-cli (Claude answers are reused)
python3 eval/agents/run.py --cases at-pension --arms hedgehog                            # one case, one agent
python3 eval/agents/run.py --arms hedgehog --hh-model gpt-4o                             # another model
python3 eval/agents/run.py --report                                                      # rebuild RESULTS.md
```

Needs hedgehog-cli installed (`~/.hedgehog-cli/bin`), its API key in `hedgehog/.env` or its
settings, Claude Code logged in, and the test data at `$HEDGEHOG_DATA` (`irregular/` by default, `clean/`
for cases with `"root": "clean"`). Each Claude Code
run is capped at `--max-usd` (default $3). Raw outputs go to `runs/` (not committed).

## The cases

30 cases: finance (budgets, P&L, cash flow, trading), sales, HR/payroll, ops/inventory, public
statistics, registers and energy; US and EU number/date formats; 200 cells to 4.5M cells; four
multi-file cases (`files`); questions of lookup, top-N, group-by with name variants, shares, change
over time, totals vs details, unit traps (thousands, %, currency) and two that the file cannot answer
(us-nyc-payroll: NYPD; at-cpi: a 2026 annual rate). Titles say where hedgehog's catalog is known to be
wrong for the file; those cases measure it.

| Case | Domain | Size | What it stresses | Checks (auto / judge) |
|---|---|---|---|---:|
| pt-contracts | register: procurement | 4.5M cells | Portugal's 2020 procurement register (176k contracts, 4.5M cells) | 9 / 4 |
| at-pension | public statistics: social | 244 cells | Austrian pension equalization supplement (small table, mixed measures, total row) | 4 / 0 |
| enron-henry-hub | energy: gas flows | 5k cells, 7 tables | Enron Henry Hub gas flows (7 tables, daily grids, labels only in the first table) | 8 / 0 |
| de-truck-toll | public statistics: transport | 144k cells, 9 sheets | German truck-toll mileage index (daily series 2008–2026, 9 sheets, statistical report) | 4 / 1 |
| enron-om-2plants | finance: budget | 2 × ~640 cells (2 files) | Two Enron peaker plants' draft 2001 O&M budgets (multi-file; monthly columns, subtotals and totals) | 5 / 0 |
| enron-newco-cashflow | finance: cash flow | ~870 cells, 3 sheets | Enron 'Newco' development cash-flow summary (values in $000s, wide year columns, label-less second sheet) | 5 / 0 |
| enron-deals-pl | finance: trading P&L | ~230 cells | Enron gas desk deal log (18 deals; trader name variants; catalog wrongly calls the 'Sumas' rows totals) | 5 / 1 |
| enron-mdea-pl | energy: trading P&L | 12k cells, 14 sheets, 32 tables | Enron/MDEA profit-sharing report (14 daily sheets, stacked day-ahead and real-time blocks; layout shifts from May 10, catalog loses the May 12–14 profit columns) | 4 / 0 |
| enron-storage-inventory | ops: inventory | ~290 cells | NewPower gas storage inventory valuation (two stacked blocks with subtotals, month-end columns, price rows below) | 6 / 0 |
| at-salzburg-budget | finance: public budget | 67k cells | State of Salzburg 2019 operating budget (3,206 budget lines, expenses and revenues in one amount column) | 7 / 0 |
| rs-pancevo-budget | finance: public budget | 506 + 17k cells (2 files) | Pančevo 2020 budget: revenues file + expenditures file (multi-file, Cyrillic headers, several funding sources) | 6 / 1 |
| us-superstore | sales | 216k cells, 3 sheets | Superstore orders 2021–2024 (10k order lines, US formats, Returns on a second sheet) | 6 / 0 |
| ms-financial-sample | sales | 11k cells | Microsoft financial sample (700 sales rows; 2013 covers only Sep–Dec) | 4 / 1 |
| nw-orders | sales/ops | 11.6k + 10.8k cells (2 files) | Northwind orders + order lines (multi-file CSV; revenue needs a join across the two files) | 4 / 0 |
| us-chicago-salaries | HR: payroll | 199k cells | City of Chicago employees and salaries (32k rows; hourly staff have no annual salary) | 6 / 1 |
| us-nyc-payroll | HR: payroll | 1.6M cells | NYC citywide payroll extract, FY2014–2025 (100k person-year rows, 2 agencies; NYPD question not answerable) | 5 / 1 |
| uk-gender-pay-gap | HR: pay | 290k cells | UK gender pay gap reports 2024/25 (11k employers; catalog flags 3 'TOTAL …' company names as totals and reads 25 values like 40.805 as 40805) | 5 / 0 |
| pt-social-report | HR: workforce | 3.4k cells, 26 tables | Portuguese municipality's 2016 social report (one sheet, 26 stacked tables, M/F/T rows, merged headers) | 7 / 0 |
| at-cpi | public statistics | 3.6k cells | Austrian consumer price index since 1990 (monthly rows interleaved with 'Ø' annual-average rows; 2026 incomplete) | 5 / 1 |
| wb-gdp-wide | public statistics | 16k cells, 66 year columns | World Bank GDP per capita, 1960–2025 wide years (countries mixed with aggregates; catalog invents a 'group' column and calls Somalia a total) | 6 / 0 |
| eu-electricity-prices | energy / public statistics | 332k cells | Eurostat household electricity prices (SDMX long format, codes for band/tax/currency, half-years 2019–2026) | 4 / 0 |
| de-gdp | public statistics | 63k cells, 56 sheets | Destatis national accounts, first results (56 sheets, 154 tables: each sheet stacks levels, % changes and index; units in a dropped header row) | 6 / 0 |
| uk-ons-labour | public statistics | 191k cells, 27 sheets | ONS labour market summary (27 sheets, notes-heavy; headline rates defined in footnotes; catalog uses 'Date of publication' as a column name) | 5 / 1 |
| fr-insee-pop | public statistics | 100k cells, 50 sheets | INSEE population by département, sex and age (49 yearly sheets; Ensemble/Hommes/Femmes blocks side by side; catalog takes the Somme département for a total row) | 7 / 0 |
| at-tourism-2months | public statistics | 2 × 6k cells (2 files) | Austrian tourism by country of origin, July and August 2026 (multi-file; 10 sheets per file; Germany total above its 7 regions) | 6 / 0 |
| rs-geodetic-licenses | register: licences | 26k cells | Serbian register of geodetic licences (3.6k people, Cyrillic, empty employer cells) | 7 / 0 |
| uk-beis-spending | register: spending | 45k cells | UK BEIS payments over £25k, March 2018 (4.3k transactions, DD/MM/YYYY dates, refunds as negatives) | 7 / 0 |
| de-berlin-grants | register: grants | 553k cells | Berlin grants database 2021–2025 (55k grants, ';' CSV, recipient spelling variants with an ID; catalog takes 'Zusammen …'/'SUMA …' recipients for totals) | 5 / 0 |
| pt-tax-benefits | register: public finance | 1.2M cells (5.5M-cell IRC range), 9 sheets | Portugal's 2023 tax-benefit beneficiaries (8 sheets, one per tax; IRC sheet 63k × 87 = 5.5M-cell range; long legal column names) | 5 / 0 |
| owid-energy | energy | 1.0M filled cells (3.0M-cell range) | Our World in Data energy dataset (23k country-years × 130 columns; countries mixed with regional aggregates) | 5 / 0 |

## Adding a case

Pick a file that stresses something different (size, layout, language, totals, time). Write the
prompt first, then compute the reference answers with `sql.py` against `T<n>_all`. Check them
against the source cells (`hedgehog cells`) and with a second query (a total row, or a sum of the
parts), and prefer checks that a number or a phrase can settle. Rerun every case after each library
or agent change, so RESULTS.md always shows where things stand.

Every `num` check stores how its reference was computed, so it can be reproduced:
`ref_sql` (a query on the case file, or a list of `{"file", "sql"}` summed across files),
`ref_cells` (source cells, for files whose catalog is wrong) or `ref_py` (a Python expression over
the raw CSV rows, for values hedgehog misreads). [check_cases.py](check_cases.py) validates the
schema, opens every file and recomputes every reference:

```sh
../scripts/run_capped.sh 4G /tmp/check.log python3 -I eval/agents/check_cases.py   # all cases (pt-contracts needs 4G)
python3 -I eval/agents/check_cases.py --schema-only                                 # no hedgehog calls
```
