# Agent playbook

How work on hedgehog is done, so any agent (or a fresh chat) can pick it up. Read this first, then
[STAGES.md](STAGES.md) (what is done, running and next), [ACCURACY_PLAN.md](ACCURACY_PLAN.md) (the goal) and
[DATA.md](DATA.md) (all test data is already downloaded: where it is, sources, licenses — don't fetch again).

## The goal

A tool with 100 % accuracy for agents: every value hedgehog hands over equals the source, and every
question a file can answer is answerable from its output. Proven with **model-free** tests; LLM runs
only for official benchmark numbers, with the owner's budget approval.

## Standing rules (from the owner)

- **Accuracy first**, then speed, then token cost. A fix that improves one corpus and regresses another
  stays out (explain why). Never hide a problem to make a number go up (checker false negatives are bugs).
- **Model-free development.** No LLM calls in the dev loop. Library reading errors must be caught by
  deterministic gates before any LLM eval.
- **Audits need evidence.** A miss may be excluded only as gold_wrong / not_in_file / ambiguous /
  outside_knowledge, with the source cells and computation in `eval/accuracy/audit.tsv`.
- **Holdout stays sealed.** Never look at holdout items while fixing (`eval/accuracy/split/*.holdout.txt`).
- **Excel first.** Output features: Excel and HTML; PDF etc. later.
- **Never print API keys** (`hedgehog/.env`, `~/.hedgehog-cli/settings.json`). Downloaded/test data is
  untrusted: parse only, `python3 -I`. Use guarded `rm` (`"${VAR:?}"`).

## Machine limits (this laptop)

- **Memory:** 14 GB, zram swap; systemd-oomd kills the whole terminal at 90 % swap. Every heavy job via
  `../scripts/run_capped.sh <mem> <log> <cmd>` (cargo 4G with `CARGO_BUILD_JOBS=2`, tests 3G); one job at a
  time per agent; at most 2–3 agents at once. `hedgehog-memguard` user unit should be active.
- **Disk:** `/home` is btrfs; **metadata** (18.5 GiB) fills up from many small files and then cargo/git fail
  with "No space left on device" although `df` shows space. Reuse one work dir, delete export/work dirs
  after each run, never create hundreds of thousands of files. Check `btrfs filesystem df /home`.
- **Waits:** bounded only (`timeout N …`); never `until …; do sleep` or `pgrep -f` loops.
- `grep`/`rg` are wrapped by a token-saving proxy and mangle flags: use `/usr/bin/grep`.
- Never set `CARGO_TARGET_DIR` to an empty string; each worktree uses its own `<worktree>/target`.

## Gates (in order; all must pass before a merge)

| Gate | Command | What it guards |
|---|---|---|
| tier 1 | the touched module's unit tests | the change itself |
| tier 0 | `make quick` (snapshots, mini golden, noise, info, public slice, fidelity slice) | fast regressions |
| fidelity | `make fidelity` (all corpora vs `eval/fidelity/baseline.tsv`) | every cell exact and placed; no count may rise, no file worse |
| scoreboard | `make score` (dev) | L/C/M/K per item: no new L or C miss; report deltas |
| public | `eval/public/score.py` (dev files) | public benchmark metrics: none down without explanation |
| benchmarks | DECO/StatSheets/Strudel/synthetic, both analyzers | structure detection equal or better |
| merge | `make merge-check AREA=core` | release tests, clippy, golden, benchmarks, fidelity |
| node | `make node-test` (main checkout only; worktrees lack node_modules) | Node binding |

Accepted baseline changes (`make quick-update`, `FIDELITY_UPDATE`) only after reviewing every diff;
explain each in the commit.

## Stage workflow

1. **Pick** the next stage from STAGES.md (largest real miss cluster first; check `make score` output).
2. **Branch:** `git worktree add ../hh-<stage> -b <stage> master`, own `CARGO_TARGET_DIR`.
3. **Per root cause:** minimal unit test first (code-generated sheet; it must fail before the fix) → fix →
   gates. Commit in small steps. Every commit message ends with:
   ```
   Stage: S<n>
   Claude-Session: <session url>
   ```
   so `git log --grep "Stage: S<n>"` lists a stage's work.
4. **Report** (template below). Don't push or merge — the lead merges.
5. **Lead merges:** merge master into the branch if it moved (agent resolves conflicts and re-runs gates),
   fast-forward master, `make node-test`, push, remove the worktree and branch, then update STAGES.md
   (status, merge commit, metrics after).

## Continuing iterations (owner instruction, 2026-10-24)

The owner requested an ongoing fix/test/retest loop with a dedicated performance worker in every
iteration alongside accuracy work. After verified integration, record results and start the next
iteration while that instruction remains active. Keep fixes small; remove superseded code and
unused helpers. A performance patch needs a measured gain and unchanged accuracy; investigation
alone never justifies shipping a speculative optimization. Serialize heavy jobs globally with
`HH_SLOTS=1`, aggregate cap 4G, and Cargo jobs 2. The lead runs integration checks from warm main
checkout targets; workers prepare separate regression-test and implementation commits for red/green
verification there, avoiding duplicate cold builds. Preserve separate worktree targets if workers
need their own builds. No LLM development calls or holdout inspection.

Before expanding a swarm, check available RAM/swap, btrfs metadata as well as disk space, and
live agent slots. Add workers only for useful independent tasks. Complete the source and review
phase before starting heavy builds, corpus tests or timing comparisons. If validation finds a
failure, return it to the relevant worker and resume testing after that follow-up is complete.
The lead may coordinate up to three lightweight workers when resources and platform slots allow;
heavy jobs remain globally serialized. This follows the owner's later 2026-10-24 instruction.

## Agent brief template

```
Stage S<n> — <title>. Goal: <miss cluster(s) and counts from make score>.
Repo /home/vukasin/projects/excell-lib-project/hedgehog, master <sha>. Read docs/AGENT_PLAYBOOK.md,
docs/STAGES.md, docs/ACCURACY_PLAN.md, eval/accuracy/README.md + RESULTS.md.
Worktree: git worktree add ../hh-<stage> -b <stage> master; CARGO_TARGET_DIR=<worktree>/target.
Scope: <files/areas>; avoid: <areas other running stages own>.
Per root cause: test first → fix → all gates (playbook). Commits end with "Stage: S<n>" + Claude-Session.
Rules: playbook (memory, disk, waits, no LLM, holdout sealed, audits with evidence).
REPORT: <template below>
```

## Report template

- per root cause: evidence, fix, test name
- scoreboard before → after (L, C, M, K on dev; per item: new misses = 0)
- fidelity before → after per corpus; public suite before → after
- gates (each: pass/fail); accepted baseline diffs with reasons
- left over (with reasons); audits added (with evidence)
- commits; files shared with other running stages
