# MiMoTable end to end: hedgehog-cli + gpt-4.1-mini

The question here is how well an agent built on hedgehog answers MiMoTable's spreadsheet questions with a
small, cheap model. This is not the model-free suite in [RESULTS.md](RESULTS.md). Script:
[`agent_mimotable.py`](agent_mimotable.py), whose docstring defines the subset, gold keys and matcher.
Per-item answers stay in `../scratch-archive/mimo-agent/` and are not committed.

**Setup.** Run on 2026-10-23. hedgehog master `9bf9c01` (the five fix-public library fixes) built as the
`hedgehog` CLI and as the Node addon that hedgehog-cli embeds (hedgehog-cli `31dae39`, single-file binary).
Each item is a one-shot `hedgehog-cli -m gpt-4.1-mini -o json "Table Name: <file>\nQuestion: <question>"`
run in a folder that holds only that item's spreadsheet(s), with a 180 s timeout and one item at a time.
Cost is estimated from the reported token counts at list prices ($0.40, $0.10 cached, $1.60 per 1M tokens).

**Subset.** 150 English questions from a fixed seed (`mimotable-agent-2026-10-23`), stratified by task class
(from `meta_operation_list`: lookup / compare / calculate / reasoning) and by MiMoTable's table type.
Each question's gold value comes from the annotator's free-text answer. Out of 641 English questions:

- 94 are dropped because they are Visualize or Edit questions or have Reasoning answers that are not
  checkable values;
- 446 have a gold value the script can extract (quoted phrases, numbers the question does not already
  contain, dates, or a short "… is X" phrase);
- I checked every sampled key by hand against its narrative. 27 items were dropped and replaced from the
  same stratum (`AUDIT_EXCLUDE`, with the reason for each). In 17 items the key was cut down to the asked
  value, dropping supporting numbers and sheet names (`KEY_KEEP`).

The subset therefore leans towards questions whose answer the annotator quoted or wrote as a number. When
the asked entity was unquoted and only its supporting value could be extracted, the item was dropped.

**Matcher.** A run is correct when every key atom is in the final answer:

- numbers match within 0.5 % relative, or after rounding to the decimals shown;
- two whole numbers must match exactly;
- percent and fraction count as the same, as do scale words (million, ten thousand) and number words;
- the sign is ignored ("a decrease of 41.67 %");
- dates match on month and day, and on the year when both have one;
- phrases are compared normalised, at word boundaries;
- lists are treated as sets and every element must be present.

A phrase that is only partly present (60 % of its words or more) is marked "unscored" and left out of the
accuracy.

## Results

| | HH_VERIFY on (default) | HH_VERIFY=0 |
|---|---|---|
| **Accuracy** (scored items) | **121 / 149 = 81.2 %** | 120 / 149 = 80.5 % |
| unscored | 1 | 1 |
| lookup (37) | 94.6 % | 91.9 % |
| compare (13) | 84.6 % | 76.9 % |
| calculate (89 scored) | 78.7 % | 79.8 % |
| reasoning (10) | 50.0 % | 50.0 % |
| single file, single sheet, simple header (72) | 86.1 % | 87.5 % |
| single file, single sheet, complicate header (32) | 90.6 % | 87.5 % |
| single file, multiple sheets, simple header (5) | 80.0 % | 80.0 % |
| single file, multiple sheets, complicate header (13) | 61.5 % | 53.8 % |
| multiple tables (16) | 62.5 % | 62.5 % |
| multiple files, simple header (4) | 75.0 % | 75.0 % |
| multiple files, complicate header (7) | 71.4 % | 71.4 % |
| simple / medium / hard table (72 / 41 / 36) | 86.1 / 87.8 / 63.9 % | 87.5 / 85.4 / 61.1 % |
| cost per item, mean | $0.0024 | $0.0026 |
| cost, total (150 items) | $0.36 | $0.39 |
| input tokens per item, mean (median) | 12,309 (9,822), 76 % cached | 13,834 (9,739) |
| output tokens per item, mean | 161 | 164 |
| wall time per item, mean (median) | 4.5 s (3.8 s) | 4.5 s (3.6 s) |
| tool calls / turns per item, mean | 2.6 / 3.4 | 2.9 / 3.6 |
| errors / timeouts | 0 / 0 (1 run stopped by the repeat guard) | 0 / 0 |

**Verification** (`verification` object of `-o json`):

- 149 of 150 answers came back with `ok: true` (the 150th run was stopped by the repeat guard before it
  wrote an answer), 0 were salvaged, and 17 runs had corrective rounds (18 rounds in all);
- the verifier makes no measurable difference here: 147 of 150 verdicts are the same in both arms, and the
  3 that differ (2 for verify, 1 against) are runs without a corrective round, so they are model run-to-run
  noise;
- in the 17 runs with correctives, the verdict is the same as in the HH_VERIFY=0 run of the same item
  (12 correct, 5 wrong);
- `ok: true` does not mean the answer is right: 27 of the 28 wrong answers passed the check, since a
  number taken from the wrong column is still a number found in the data;
- one corrective hurt: in en-0597 the first draft said 32 days, which is correct. The checker could not find
  32 in any tool output, the model re-queried with an `AND … OR …` precedence bug and answered 6.

### Why the 28 wrong answers are wrong (verify arm, checked by hand)

| Cause | Items | Notes |
|---|---|---|
| Wrong gold answer in MiMoTable | 8 | The model is right on the data: the sum of an 8000 column with one text cell is 112,000, not 120,000; weekend overtime 9,488 > weekday 8,687; growth 120 → 174.95 is +45.8 %, not −41.67 %; a "5.64 million" gold that is 564,000 × 10,000 yuan; average unreceived 1,301.6, not 1,435.24; … |
| Library: structure handed to the model is wrong | 9 | "March 25" read as the date 2025-03-01 (two items); a header row missed after a key/value line, so columns are `col_B…` (two items on one file); a section row merged into the header; a first data row taken as the header and the table split in two; a horizontal key/value form read as a table; a sub-header ("This Year") not grouped under its section; an Excel date serial (44310) left as a number |
| Model reasoning | 8 | Summed a figure repeated across daily sheets; listed the parts but did not add them; ignored a filter; named one of two tied answers (two items); a precedence bug after a corrective; … |
| Ambiguous question or header | 2 | Duplicate `Ratio` columns; "most production plans" read as rows rather than quantities |
| Harness | 1 | Stopped by the repeat guard before answering |

The matcher made no wrong calls on the failures: after two matcher fixes, every wrong verdict reflects a
wrong or incomplete answer. The fixes were number words ("Four months") and exact matching for whole
numbers. Without the second fix, 1022 had matched 1023 within 0.5 %. In 70 correct answers with many
numbers or a long text I found no false positive. One case the matcher does accept: a sum of 3,501.25
against a gold of 3,500.25 (0.03 %).

If the 8 faulty gold answers are left out, accuracy is **121 / 141 = 85.8 %**. If the model's answer to
each of them is counted as correct, it is 129 / 149 = 86.6 %.

## Next to the paper

Li et al., *MiMoTable* (COLING 2025, arXiv 2412.11711), Table 6, accuracy in %. The paper judges answers
with GPT-4o (Correct / Incorrect / Uncertain), puts the whole workbook as markdown in the prompt
(GPT-4o-TXT), or uses a code interpreter (GPT-4o-CI). It does not report results by table type.

| Setting | English | Lookup | Compare | Calculate | Reasoning | Simple / Medium / Hard table |
|---|---|---|---|---|---|---|
| Claude-3.5-Sonnet (text) | 79.0 | 89.0 | 79.7 | 76.1 | 63.3 | 81.3 / 75.5 / 72.1 |
| GPT-4o-CI (code interpreter) | 70.8 | 81.0 | 73.1 | 70.6 | 45.8 | 81.7 / 67.1 / 50.8 |
| GPT-4o-TXT (markdown) | 69.3 | 85.1 | 73.1 | 64.5 | 53.9 | 73.8 / 66.2 / 62.1 |
| Llama-3.1-70B-Instruct | 56.6 | 82.0 | 57.7 | 48.8 | 45.1 | 64.0 / 51.6 / 51.3 |
| **hedgehog-cli + gpt-4.1-mini (this run, 150-item subset)** | **81.2** | **94.6** | **84.6** | **78.7** | **50.0** | **86.1 / 87.8 / 63.9** |

The paper's figures are as extracted from the arXiv HTML. In that table, Lookup equals the question
difficulty [1,2) column in every row, and Reasoning equals [3,4]. These two columns may be mislabelled
there, so treat them as approximate.

**Comparability. The numbers are not directly comparable:**

- *Subset.* Ours is 150 English questions with a checkable gold value, against the paper's 1,636
  questions in both languages without Visualize or Edit. Our selection drops open-ended answers and answers
  whose entity the annotator did not quote, and those are likely harder. Our task classes are also not the
  paper's: the paper scores a question under every operation it has, while we put it in one class
  (Reasoning > Calculate > Compare > Lookup).
- *Judge.* We use a strict, deterministic value matcher. The paper uses a GPT-4o judge that accepts an
  answer "containing the key information", which tends to be more lenient. Neither one corrects the 8
  faulty gold answers.
- *Model and tools.* Our model is gpt-4.1-mini, which is newer and cheaper than every model in the paper.
  It uses hedgehog's tools (catalog, SQL query, cells), not the whole table in the prompt.
- *Sample size.* With 149 items, the 95 % interval around 81 % is about ±6 points. Per-class cells of 10–13
  items say little.

The reading the data supports: on checkable MiMoTable questions, hedgehog-cli with a small model is in the
range the paper reports for the strongest text model (Claude-3.5-Sonnet, 79 % English). It does this with
about 10k input tokens per question, three-quarters of them cached, at $0.0024 and 4.5 s per question.
Hard tables (63.9 %), several sheets with complicated headers (61.5 %), and multiple tables (62.5 %) are
where it loses. Nine of the 28 wrong answers come from structure the library got wrong, so these are the
library's targets: day-month text read as month-year, header rows missed after key/value lines, section
rows merged into headers, Excel date serials, horizontal forms.

## Reproduce

```sh
S=../scripts/run_capped.sh; L=../scratch-archive/mimo-agent/logs
python3 -I eval/public/agent_mimotable.py sample --show          # subset and keys (the audit view)
$S 4G $L/run-verify.log python3 -I eval/public/agent_mimotable.py run --max-usd 0.7       # resumable
$S 4G $L/run-noverify.log python3 -I eval/public/agent_mimotable.py run --arm noverify --max-usd 0.6
python3 -I eval/public/agent_mimotable.py score [--arm noverify] [--show]
```

The OpenAI key is read from `hedgehog/.env` through `eval/agents/run.py`'s `dotenv()`, and only `*_API_KEY`
entries are passed to hedgehog-cli. Both arms together cost $0.74.
