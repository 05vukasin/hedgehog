# Case study: 176,000 public contracts, two agents

One real spreadsheet, four analyst questions, two terminal agents side by side:

- **hedgehog-cli**: a chat agent whose only tools are hedgehog's (`open_file`, `query`, …),
  running on **gpt-4o-mini**.
- **Claude Code**: a general coding agent running a frontier Claude model, which answers by
  writing and running its own pandas scripts.

The point of the comparison: what a spreadsheet-native tool layer buys in time, tokens and cost,
and where the answers still differ.

## The file

`eu_pt_opendata/001_contratos2020.xlsx`: Portugal's public procurement register for 2020
(base.gov.pt open data). One sheet, **4,565,843 cells**, 39 columns, 176,512 contracts: buyer
(`adjudicante`), winning supplier(s) (`adjudicatarios`), contract price (`precoContratual`),
effective total price (`PrecoTotalEfetivo`), procedure type, signing date, CPV code, location.
The file is 40 MB.

## The questions

Both agents got the same prompt in one message:

1. Which 5 public bodies spent the most on contracts in 2020, and how much each?
2. Which suppliers won the most contracts, and what share of the total value went to the top 10?
3. Compare direct awards (Ajuste Direto) with public tenders (Concurso público): how many
   contracts and total value for each?
4. How many contracts were signed per month in 2020? Was there a jump when COVID started in March?

## Time, tokens, cost

| | hedgehog-cli (gpt-4o-mini) | Claude Code |
|---|---|---|
| Time to the full answer | **1 min 05 s** | 3 min 08 s |
| — of which reading the file | ~45 s (one `open_file`, 4.5M cells, normalized once) | inside its pandas scripts |
| — of which the LLM | 14 s | most of it |
| Tool calls | 6 (5 ok, 1 SQL error it fixed itself) | 9 shell commands (pandas) |
| LLM requests | 4 | — |
| Input / output tokens | **13,863 / 1,404** (83% of input from the prompt cache) | see the Claude Code usage display for this run |
| Approximate LLM cost | **≈ $0.002** (gpt-4o-mini list prices) | far higher (frontier model, longer context) |

The whole 4.5-million-cell file reached gpt-4o-mini as a catalog of about 2,000 tokens: every
column with its type, date range and the values of the small label columns. After that, each
answer was one SQL query that returned a handful of rows. No rows of the sheet were ever pasted
into the prompt.

## Who got it right

The reference numbers below were checked directly with SQL over every row of the file
(`T1_all`).

| Question | Reference | hedgehog-cli | Claude Code |
|---|---|---|---|
| Total value | €11.16 bn (`precoContratual`) | not asked | €11.16 bn ✓ |
| 1. Top body | Infraestruturas de Portugal, written 6 different ways; grouped by tax ID, about €738 M | listed twice under two spellings (€268 M and €122 M) ✗ | grouped by tax ID: €737.7 M ✓ |
| 1. Price column | `precoContratual` (contract price at signing, €11.16 bn) | used `PrecoTotalEfetivo` (effective price so far, €4.28 bn) ✗ | `precoContratual` ✓ |
| 2. "Won the most contracts" | ranked by number of contracts | ranked by value instead ✗; top-10 share not computed ✗ | by count (B. Braun 1,458, Medtronic 1,152 after merging spellings) ✓, share computed ✓ |
| 3. Ajuste Direto, Regime Geral | 78,349 contracts, €2.41 bn | 78,323, €0.69 bn (wrong price column) ~ | 78,339, €2.41 bn ✓ |
| 3. Concurso público | 17,408 contracts, €5.48 bn (49%) | 17,408 ✓, €2.56 bn (wrong price column) ~ | €5.48 bn (49.1%) ✓ |
| 4. Contracts per month | Jan 16,154 · Mar 13,983 · Apr 10,682 · Dec 15,940; 26,723 without a date | Jan 16,147 · Mar 13,979 · Apr 10,681 · Dec 15,938 ✓ (within 0.05%) | Jan 16,152 · Mar 13,981 · Apr 10,682 · Dec 15,935 ✓ |
| 4. COVID jump? | No overall jump: March +6% over February but below January, then a fall in April–May. COVID-related purchases did surge. | "a notable increase … indicating a jump" ✗ | "did not jump; fell in April–May; COVID purchases rose sharply" ✓, with weekly detail |

Claude Code also noticed, without being asked:

- duplicate contract IDs (20);
- contracts with several winners (1,087);
- simplified direct awards that have no signing date (26,723);
- negative prices.

hedgehog-cli did not report any of these.

## Why the answers differ

**The numbers hedgehog returned were exact.** Every SQL result matched the file. The per-month
counts agree with the reference within 0.05%. The small gap comes from a hedgehog bug: 38
contracts whose description contains the word *total* ("Prótese total anca", a hip prosthesis)
were taken for total rows and kept out of `T1`.

**The mistakes were judgement calls by a small model**, made because the catalog did not warn
about the traps:

1. **Same entity, many spellings.** The buyer and supplier columns hold `"<tax id> - <name>"` with
   up to 6 spellings per tax ID. Claude Code checked this and grouped by tax ID. gpt-4o-mini
   grouped by the raw text.
2. **Two price columns.** `precoContratual` and `PrecoTotalEfetivo` look alike. Nothing said
   which one means "spent on contracts".
3. **Reading the question.** "Won the most contracts" means count, not value. The model also
   stopped before computing the top-10 share, and called a 6% rise "a jump".
4. **SQL dialect.** The model first tried MySQL's `MONTH()`. The error message was enough for it
   to switch to `strftime`, at the cost of one extra call.

## What this means

- **Speed and cost.** For questions over a big sheet, a spreadsheet-native tool layer answers
  about **3× faster**, and with a small model for about a fifth of a cent, because the LLM only
  ever sees a catalog and query results. Claude Code spent most of its time loading and exploring
  the sheet with pandas.
- **Depth.** Data-quality checks and careful reading of a question still come from a strong model.
- **Best of both.** A strong model with hedgehog's tools. `hedgehog mcp` already plugs into
  Claude Code and Claude Desktop ([docs/mcp.md](mcp.md)): the same agent would get the 2,000-token
  catalog and instant SQL instead of writing pandas from scratch, and could be packaged as a
  Claude skill for spreadsheet analysis.

## What hedgehog will fix from this run

| Finding | Fix |
|---|---|
| 38 contracts taken for total rows because their description says "total" | total rows in flat registers must also add up; a keyword in a free-text column is not enough |
| One entity under many spellings | catalog: flag `"<id> - <name>"` columns and columns where one ID has several names, and suggest grouping by the ID |
| Two similar price columns | catalog: show each numeric column's total and how often it is filled, so the difference is visible (€11.16 bn vs €4.28 bn) |
| Duplicate IDs, rows without a date | catalog: report duplicate values in ID-like columns and the empty share of date columns |
| `MONTH()` error | SQL errors name the dialect and the replacement (`strftime('%m', col)`) |
| Small-model judgement | try a stronger default model for multi-part questions; rerun this case after the fixes above |

This case is now tracked with three others in [eval/agents](../eval/agents/RESULTS.md): every
change is rerun on the same prompts for hedgehog and Claude Code, and the fixes above are items
B1–B11 in [the backlog](../eval/agents/BACKLOG.md).

Run it yourself:

```console
$ cd testdata/irregular/eu_pt_opendata
$ hhc          # hedgehog-cli; then: @001_contratos2020.xlsx <questions>
```
