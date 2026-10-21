# Agent comparisons

The benchmarks in [docs/evaluation.md](../../docs/evaluation.md) measure whether hedgehog labels
cells correctly. This folder measures what users care about: **does an agent with hedgehog's tools
answer real questions about a real file correctly, quickly and cheaply, compared with a strong
general agent that has no hedgehog?**

## The method

1. **A case** is a file, a prompt and checks ([cases.json](cases.json)). The prompt is what an
   analyst would ask. The reference answers come from SQL over every row of the file (`T<n>_all`),
   checked by hand. [sql.py](sql.py) runs such queries.
2. **The same prompt goes to every agent**, as `File: <path>` plus the questions:
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

```sh
# from the repo root; the whole run under a memory cap (Claude's pandas on 4.5M cells is heavy)
../scripts/run_capped.sh 6G /tmp/agents.log python3 eval/agents/run.py                 # all cases, hedgehog + claude
python3 eval/agents/run.py --cases at-pension --arms hedgehog                            # one case, one agent
python3 eval/agents/run.py --arms hedgehog --hh-model gpt-4o                             # another model
python3 eval/agents/run.py --report                                                      # rebuild RESULTS.md
```

Needs hedgehog-cli installed (`~/.hedgehog-cli/bin`), its API key in `hedgehog/.env` or its
settings, Claude Code logged in, and the test data at `$HEDGEHOG_DATA/irregular`. Each Claude Code
run is capped at `--max-usd` (default $3). Raw outputs go to `runs/` (not committed).

## Adding a case

Pick a file that stresses something different (size, layout, language, totals, time). Write the
prompt first, then compute the reference answers with `sql.py` against `T<n>_all`. Check them
against the source cells, and prefer checks that a number or a phrase can settle. Rerun every case
after each library or agent change, so RESULTS.md always shows where things stand.
