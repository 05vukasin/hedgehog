---
name: accuracy-loop
description: Use when working on hedgehog's accuracy, library fixes, benchmarks, the scoreboard, or when continuing the staged work (starting/briefing/merging a stage, checking what is done or next). Points to the playbook, the stage ledger and the plan, and says how to resume.
---

# hedgehog accuracy loop

hedgehog is being made a tool with 100 % accuracy for agents, measured model-free. All work is done in
numbered **stages** on their own branches; the lead merges them after the gates.

## Read first (in this order)

0. `docs/OVERVIEW.md` — the whole picture of both projects, if you are new.
1. `docs/STAGES.md` — current numbers, what is done, what is **running** (branch + worktree) and **next**.
2. `docs/AGENT_PLAYBOOK.md` — standing rules, machine limits (memory, btrfs metadata), the gates, the
   stage workflow, the brief and report templates, the commit trailer `Stage: S<n>`.
3. `docs/ACCURACY_PLAN.md` — what "100 %" means (F/S/L/C/M/K), audits, dev/holdout split.
4. `eval/accuracy/README.md` + `RESULTS.md` — the scoreboard and the current miss clusters.
5. `docs/DATA.md` — all test data is already on disk in `../testdata` (sources, licenses, manifests); never re-download.

## Resuming in a fresh chat

1. `git -C <repo> worktree list` and `git log --oneline master..<branch>` for each running stage in STAGES.md:
   is its work committed? Are its capped jobs still running
   (`systemctl --user list-units 'run-*' --state=running`)?
2. If a stage's agent is gone, start a new agent with the same brief (STAGES.md "Running" row + the
   playbook's brief template) telling it to continue from the branch's last commit.
3. When a stage reports: check its gates, merge (playbook "Lead merges"), update STAGES.md.
4. Pick the next stage from STAGES.md "Next"; brief it with the template; at most 2–3 agents at once.

## Never

- LLM calls in the dev loop (official benchmark runs only, with the owner's budget approval).
- Looking at holdout items; auditing without source-cell evidence; hiding a problem in a checker.
- Heavy jobs outside `../scripts/run_capped.sh`; unbounded waits; mass small files (btrfs metadata).
