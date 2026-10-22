# Agent comparison: latest results

## Headline: hedgehog-cli vs Claude Code

The question this folder answers: how much does the **tool** give? **hedgehog-cli** is a simple chat
loop on a small, cheap model whose only tools are hedgehog's. **Claude Code** is the strongest
general harness, on a frontier model, reading the file with its own Python. Same file, same prompt.

| Case | hedgehog-cli checks (worst) | Claude Code checks | Time: hedgehog / Claude | Cost: hedgehog / Claude | Input tokens: hedgehog / Claude |
|---|---:|---:|---:|---:|---:|
| pt-contracts | 8.7/13 (8) | 13.0/13 | 17 s / 101 s | $0.0058 / $0.58 | 22k / 370k |
| at-pension | 4.0/4 (4) | 4.0/4 | 3 s / 17 s | $0.0018 / $0.29 | 10k / 163k |
| enron-henry-hub | 7.7/8 (7) | 8.0/8 | 8 s / 30 s | $0.0066 / $0.43 | 45k / 285k |
| de-truck-toll | 4.0/5 (4) | 5.0/5 | 6 s / 18 s | $0.0033 / $0.32 | 21k / 171k |
| enron-om-2plants | 5.0/5 (5) | 3.0/5 | 7 s / 18 s | $0.0029 / $0.38 | 14k / 133k |
| enron-newco-cashflow | 4.3/5 (4) | 5.0/5 | 13 s / 14 s | $0.0096 / $0.33 | 53k / 130k |
| enron-deals-pl | 3.7/6 (3) | 6.0/6 | 8 s / 21 s | $0.0066 / $0.31 | 37k / 208k |
| enron-mdea-pl | 1.3/4 (0) | 4.0/4 | 7 s / 42 s | $0.0051 / $0.39 | 33k / 262k |
| enron-storage-inventory | 2.7/6 (2) | 6.0/6 | 24 s / 21 s | $0.0199 / $0.29 | 131k / 122k |
| at-salzburg-budget | 7.0/7 (7) | 5.0/7 | 8 s / 43 s | $0.0037 / $0.35 | 22k / 248k |
| rs-pancevo-budget | 6.0/7 (6) | 7.0/7 | 8 s / 26 s | $0.0040 / $0.36 | 21k / 255k |
| us-superstore | 5.0/6 (5) | 6.0/6 | 11 s / 37 s | $0.0055 / $0.33 | 34k / 247k |
| ms-financial-sample | 4.0/5 (4) | 5.0/5 | 9 s / 20 s | $0.0044 / $0.29 | 29k / 203k |
| nw-orders | 4.0/4 (4) | 4.0/4 | 10 s / 18 s | $0.0049 / $0.27 | 29k / 161k |
| us-chicago-salaries | 5.0/7 (5) | 7.0/7 | 6 s / 25 s | $0.0030 / $0.32 | 16k / 167k |
| us-nyc-payroll | 4.0/6 (2) | 6.0/6 | 10 s / 30 s | $0.0050 / $0.32 | 26k / 247k |
| uk-gender-pay-gap | 5.0/5 (5) | 5.0/5 | 11 s / 31 s | $0.0055 / $0.30 | 34k / 206k |
| pt-social-report | 5.7/7 (5) | 7.0/7 | 16 s / 20 s | $0.0124 / $0.41 | 72k / 217k |
| at-cpi | 3.0/6 (2) | 6.0/6 | 17 s / 23 s | $0.0108 / $0.32 | 74k / 209k |
| wb-gdp-wide | 5.3/6 (4) | 6.0/6 | 19 s / 20 s | $0.0088 / $0.26 | 49k / 121k |
| eu-electricity-prices | 3.0/4 (2) | 4.0/4 | 9 s / 22 s | $0.0041 / $0.29 | 22k / 165k |
| de-gdp | 5.3/6 (5) | 6.0/6 | 23 s / 30 s | $0.0214 / $0.36 | 129k / 216k |
| uk-ons-labour | 4.3/6 (4) | 6.0/6 | 9 s / 39 s | $0.0073 / $0.38 | 41k / 301k |
| fr-insee-pop | 7.0/7 (7) | 7.0/7 | 15 s / 21 s | $0.0096 / $0.41 | 59k / 244k |
| at-tourism-2months | 3.3/6 (1) | 6.0/6 | 22 s / 33 s | $0.0152 / $0.37 | 95k / 258k |
| rs-geodetic-licenses | 6.7/7 (6) | 7.0/7 | 8 s / 32 s | $0.0032 / $0.34 | 19k / 250k |
| uk-beis-spending | 6.7/7 (6) | 6.0/7 | 9 s / 46 s | $0.0040 / $0.36 | 26k / 251k |
| de-berlin-grants | 5.0/5 (5) | 5.0/5 | 9 s / 57 s | $0.0038 / $0.38 | 24k / 296k |
| pt-tax-benefits | 4.3/5 (4) | 5.0/5 | 8 s / 106 s | $0.0037 / $0.46 | 17k / 329k |
| owid-energy | 4.0/5 (4) | 5.0/5 | 9 s / 13 s | $0.0050 / $0.24 | 31k / 115k |
| **all cases** | **145.0/180 (130)** | **175.0/180** | 338 s / 971 s | $0.21 / $10.45 | 1237k / 6551k |

Models: Claude Code: claude-opus-5-5[1m], hedgehog-cli: gpt-4.1-mini. hedgehog-cli is 2.9× faster and 51× cheaper; it passes 81 % of the checks vs 97 % for Claude Code.
`claude-hh` (Claude Code plus hedgehog's plugin) is in the table below: the same tool in the best harness.

## Tool swap: same loop, same model, only the tools change

A minimal loop ([swap/agent.py](swap/agent.py): one system prompt plus a one-line tool intro, temperature 0.2,
at most 20 model calls) on one small model; only the tool set differs. Automatic checks only (judgement
checks are left out). Mean = mean over the runs, worst = each case's worst run, summed over cases.

| Tool set | Model | Cases | Checks mean | Checks worst | Time | Input / output tokens | Cost | $ / pass | Tokens / pass | Tool calls (errors) |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| hedgehog-cli (reference) | gpt-4.1-mini | 30 | 145.0/168 (86 %) | 130/168 | 338 s | 1,237k / 26.1k | $0.207 | $0.0014 | 8.7k | 228 (22) |
| Claude Code (reference) | claude-opus-5-5[1m] | 30 | 163.0/168 (97 %) | 163/168 | 971 s | 6,551k / 68.6k | $10.452 | $0.0641 | 40.6k | – |
| **hedgehog MCP** | gpt-4.1-mini-2025-04-14 | 30 | 143.3/168 (85 %) | 133/168 | 329 s | 602k / 23.6k | $0.139 | $0.0010 | 4.4k | 230 (19) |

Per case, automatic checks: mean (worst) / n.

| Case | hedgehog-cli (reference) | Claude Code (reference) | hedgehog MCP |
|---|---:|---:|---:|
| pt-contracts | 8.7 (8) / 9 | 9.0 (9) / 9 | 8.7 (8) / 9 |
| at-pension | 4.0 (4) / 4 | 4.0 (4) / 4 | 2.7 (2) / 4 |
| enron-henry-hub | 7.7 (7) / 8 | 8.0 (8) / 8 | 8.0 (8) / 8 |
| de-truck-toll | 4.0 (4) / 4 | 4.0 (4) / 4 | 4.0 (4) / 4 |
| enron-om-2plants | 5.0 (5) / 5 | 3.0 (3) / 5 | 5.0 (5) / 5 |
| enron-newco-cashflow | 4.3 (4) / 5 | 5.0 (5) / 5 | 3.3 (2) / 5 |
| enron-deals-pl | 3.7 (3) / 5 | 5.0 (5) / 5 | 4.0 (4) / 5 |
| enron-mdea-pl | 1.3 (0) / 4 | 4.0 (4) / 4 | 0.0 (0) / 4 |
| enron-storage-inventory | 2.7 (2) / 6 | 6.0 (6) / 6 | 2.7 (2) / 6 |
| at-salzburg-budget | 7.0 (7) / 7 | 5.0 (5) / 7 | 7.0 (7) / 7 |
| rs-pancevo-budget | 6.0 (6) / 6 | 6.0 (6) / 6 | 6.0 (6) / 6 |
| us-superstore | 5.0 (5) / 6 | 6.0 (6) / 6 | 4.3 (4) / 6 |
| ms-financial-sample | 4.0 (4) / 4 | 4.0 (4) / 4 | 4.0 (4) / 4 |
| nw-orders | 4.0 (4) / 4 | 4.0 (4) / 4 | 3.3 (3) / 4 |
| us-chicago-salaries | 5.0 (5) / 6 | 6.0 (6) / 6 | 5.0 (5) / 6 |
| us-nyc-payroll | 4.0 (2) / 5 | 5.0 (5) / 5 | 5.0 (5) / 5 |
| uk-gender-pay-gap | 5.0 (5) / 5 | 5.0 (5) / 5 | 4.3 (4) / 5 |
| pt-social-report | 5.7 (5) / 7 | 7.0 (7) / 7 | 5.7 (5) / 7 |
| at-cpi | 3.0 (2) / 5 | 5.0 (5) / 5 | 3.7 (3) / 5 |
| wb-gdp-wide | 5.3 (4) / 6 | 6.0 (6) / 6 | 5.7 (5) / 6 |
| eu-electricity-prices | 3.0 (2) / 4 | 4.0 (4) / 4 | 4.0 (4) / 4 |
| de-gdp | 5.3 (5) / 6 | 6.0 (6) / 6 | 5.3 (4) / 6 |
| uk-ons-labour | 4.3 (4) / 5 | 5.0 (5) / 5 | 4.7 (4) / 5 |
| fr-insee-pop | 7.0 (7) / 7 | 7.0 (7) / 7 | 7.0 (7) / 7 |
| at-tourism-2months | 3.3 (1) / 6 | 6.0 (6) / 6 | 2.0 (1) / 6 |
| rs-geodetic-licenses | 6.7 (6) / 7 | 7.0 (7) / 7 | 6.3 (6) / 7 |
| uk-beis-spending | 6.7 (6) / 7 | 6.0 (6) / 7 | 6.7 (6) / 7 |
| de-berlin-grants | 5.0 (5) / 5 | 5.0 (5) / 5 | 5.0 (5) / 5 |
| pt-tax-benefits | 4.3 (4) / 5 | 5.0 (5) / 5 | 5.0 (5) / 5 |
| owid-energy | 4.0 (4) / 5 | 5.0 (5) / 5 | 5.0 (5) / 5 |

## All runs

Generated by `eval/agents/run.py --report` from `results.jsonl` (every run is kept there); method in
[README.md](README.md), open work in [BACKLOG.md](BACKLOG.md). ✓/✗ = automatic check against the
reference answer, (✓)/(✗) = graded by a person, ? = not graded yet. With repeated runs (`--repeat`),
numbers are means; Checks shows the mean and the worst run, and the check table shows how many runs passed.

| Case | Agent | Date | Model | Runs | Time | Input / output tokens | Cost (USD) | Tools (errors) | Checks (worst) |
|---|---|---|---|---:|---:|---:|---:|---:|---:|
| pt-contracts | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 17 s | 22,047 / 1,673 | 0.0058 | 6 (1) | 8.7/13 (8) |
| pt-contracts | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 101 s | 370,341 / 6,913 | 0.58 | – | 13/13 |
| pt-contracts | claude-hh | 2026-10-21 | claude-opus-5-5[1m] | 1 | 67 s | 212,293 / 4,074 | 0.34 | – | 9/13 |
| pt-contracts | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 27 s | 9,248 / 1,343 | 0.0042 | 6 (0) | 8.7/13 (8) |
| at-pension | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 3 s | 10,393 / 161 | 0.0018 | 1 (0) | 4.0/4 (4) |
| at-pension | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 17 s | 162,596 / 1,101 | 0.29 | – | 4/4 |
| at-pension | claude-hh | 2026-10-21 | claude-opus-5-5[1m] | 1 | 18 s | 132,222 / 885 | 0.21 | – | 4/4 |
| at-pension | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 5 s | 7,737 / 370 | 0.0020 | 3 (0) | 2.7/4 (2) |
| enron-henry-hub | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 8 s | 44,563 / 643 | 0.0066 | 4 (0) | 7.7/8 (7) |
| enron-henry-hub | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 30 s | 284,741 / 2,298 | 0.43 | – | 8/8 |
| enron-henry-hub | claude-hh | 2026-10-21 | claude-opus-5-5[1m] | 1 | 11 s | 143,072 / 947 | 0.25 | – | 8/8 |
| enron-henry-hub | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 8 s | 27,700 / 572 | 0.0048 | 4 (0) | 8.0/8 (8) |
| de-truck-toll | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 6 s | 20,923 / 358 | 0.0033 | 3 (0) | 4.0/5 (4) |
| de-truck-toll | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 18 s | 170,773 / 1,693 | 0.32 | – | 5/5 |
| de-truck-toll | claude-hh | 2026-10-21 | claude-opus-5-5[1m] | 1 | 15 s | 167,817 / 909 | 0.22 | – | 4/5 |
| de-truck-toll | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 7 s | 13,873 / 435 | 0.0031 | 4 (0) | 4.0/5 (4) |
| enron-om-2plants | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 7 s | 14,192 / 670 | 0.0029 | 7 (0) | 5.0/5 (5) |
| enron-om-2plants | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 18 s | 133,049 / 1,699 | 0.38 | – | 3/5 |
| enron-om-2plants | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 10 s | 13,146 / 1,161 | 0.0043 | 13 (0) | 5.0/5 (5) |
| enron-newco-cashflow | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 13 s | 52,768 / 835 | 0.0096 | 10 (2) | 4.3/5 (4) |
| enron-newco-cashflow | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 14 s | 130,475 / 964 | 0.33 | – | 5/5 |
| enron-newco-cashflow | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 12 s | 23,401 / 887 | 0.0047 | 9 (0) | 3.3/5 (2) |
| enron-deals-pl | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 8 s | 36,566 / 478 | 0.0066 | 5 (0) | 3.7/6 (3) |
| enron-deals-pl | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 21 s | 207,502 / 2,004 | 0.31 | – | 6/6 |
| enron-deals-pl | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 9 s | 10,415 / 674 | 0.0033 | 9 (4) | 4.0/6 (4) |
| enron-mdea-pl | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 7 s | 32,757 / 431 | 0.0051 | 3 (0) | 1.3/4 (0) |
| enron-mdea-pl | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 42 s | 261,995 / 2,976 | 0.39 | – | 4/4 |
| enron-mdea-pl | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 31 s | 105,450 / 1,938 | 0.0167 | 17 (0) | 0.0/4 (0) |
| enron-storage-inventory | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 24 s | 131,176 / 1,844 | 0.0199 | 13 (2) | 2.7/6 (2) |
| enron-storage-inventory | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 21 s | 122,461 / 1,886 | 0.29 | – | 6/6 |
| enron-storage-inventory | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 12 s | 25,669 / 1,147 | 0.0062 | 10 (1) | 2.7/6 (2) |
| at-salzburg-budget | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 8 s | 22,385 / 559 | 0.0037 | 4 (0) | 7.0/7 (7) |
| at-salzburg-budget | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 43 s | 248,022 / 2,895 | 0.35 | – | 5/7 |
| at-salzburg-budget | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 12 s | 19,677 / 751 | 0.0048 | 7 (0) | 7.0/7 (7) |
| rs-pancevo-budget | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 8 s | 21,467 / 690 | 0.0040 | 8 (0) | 6.0/7 (6) |
| rs-pancevo-budget | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 26 s | 255,282 / 2,455 | 0.36 | – | 7/7 |
| rs-pancevo-budget | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 11 s | 23,194 / 902 | 0.0052 | 9 (0) | 6.0/7 (6) |
| us-superstore | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 11 s | 34,153 / 658 | 0.0055 | 7 (0) | 5.0/6 (5) |
| us-superstore | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 37 s | 247,386 / 2,385 | 0.33 | – | 6/6 |
| us-superstore | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 7 s | 8,345 / 642 | 0.0028 | 7 (1) | 4.3/6 (4) |
| ms-financial-sample | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 9 s | 28,964 / 527 | 0.0044 | 6 (0) | 4.0/5 (4) |
| ms-financial-sample | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 20 s | 202,736 / 1,805 | 0.29 | – | 5/5 |
| ms-financial-sample | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 5 s | 6,263 / 452 | 0.0020 | 7 (0) | 4.0/5 (4) |
| nw-orders | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 10 s | 29,438 / 500 | 0.0049 | 7 (1) | 4.0/4 (4) |
| nw-orders | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 18 s | 161,418 / 1,560 | 0.27 | – | 4/4 |
| nw-orders | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 9 s | 14,139 / 488 | 0.0034 | 6 (1) | 3.3/4 (3) |
| us-chicago-salaries | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 6 s | 16,237 / 462 | 0.0030 | 6 (0) | 5.0/7 (5) |
| us-chicago-salaries | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 25 s | 166,513 / 2,538 | 0.32 | – | 7/7 |
| us-chicago-salaries | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 5 s | 5,181 / 406 | 0.0021 | 6 (0) | 5.0/7 (5) |
| us-nyc-payroll | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 10 s | 26,044 / 905 | 0.0050 | 9 (3) | 4.0/6 (2) |
| us-nyc-payroll | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 30 s | 246,822 / 2,521 | 0.32 | – | 6/6 |
| us-nyc-payroll | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 13 s | 15,099 / 621 | 0.0040 | 7 (1) | 5.0/6 (5) |
| uk-gender-pay-gap | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 11 s | 33,563 / 638 | 0.0055 | 7 (1) | 5.0/5 (5) |
| uk-gender-pay-gap | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 31 s | 205,520 / 2,255 | 0.30 | – | 5/5 |
| uk-gender-pay-gap | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 8 s | 8,626 / 619 | 0.0029 | 8 (1) | 4.3/5 (4) |
| pt-social-report | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 16 s | 72,202 / 1,612 | 0.0124 | 12 (1) | 5.7/7 (5) |
| pt-social-report | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 20 s | 217,181 / 2,025 | 0.41 | – | 7/7 |
| pt-social-report | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 9 s | 26,586 / 677 | 0.0059 | 8 (1) | 5.7/7 (5) |
| at-cpi | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 17 s | 73,744 / 1,094 | 0.0108 | 11 (0) | 3.0/6 (2) |
| at-cpi | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 23 s | 209,246 / 1,821 | 0.32 | – | 6/6 |
| at-cpi | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 8 s | 11,575 / 680 | 0.0032 | 6 (0) | 3.7/6 (3) |
| wb-gdp-wide | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 19 s | 48,692 / 1,862 | 0.0088 | 10 (0) | 5.3/6 (4) |
| wb-gdp-wide | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 20 s | 121,449 / 1,351 | 0.26 | – | 6/6 |
| wb-gdp-wide | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 9 s | 10,436 / 888 | 0.0035 | 7 (0) | 5.7/6 (5) |
| eu-electricity-prices | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 9 s | 22,450 / 880 | 0.0041 | 4 (0) | 3.0/4 (2) |
| eu-electricity-prices | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 22 s | 165,007 / 1,522 | 0.29 | – | 4/4 |
| eu-electricity-prices | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 10 s | 11,338 / 814 | 0.0037 | 6 (0) | 4.0/4 (4) |
| de-gdp | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 23 s | 128,855 / 1,792 | 0.0214 | 21 (7) | 5.3/6 (5) |
| de-gdp | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 30 s | 216,493 / 1,944 | 0.36 | – | 6/6 |
| de-gdp | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 13 s | 44,427 / 1,165 | 0.0097 | 13 (6) | 5.3/6 (4) |
| uk-ons-labour | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 9 s | 40,590 / 814 | 0.0073 | 4 (1) | 4.3/6 (4) |
| uk-ons-labour | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 39 s | 301,107 / 2,245 | 0.38 | – | 6/6 |
| uk-ons-labour | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 9 s | 26,508 / 597 | 0.0051 | 5 (1) | 4.7/6 (4) |
| fr-insee-pop | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 15 s | 59,460 / 1,171 | 0.0096 | 10 (0) | 7.0/7 (7) |
| fr-insee-pop | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 21 s | 244,094 / 1,953 | 0.41 | – | 7/7 |
| fr-insee-pop | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 18 s | 43,052 / 1,379 | 0.0093 | 12 (0) | 7.0/7 (7) |
| at-tourism-2months | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 22 s | 95,002 / 2,025 | 0.0152 | 21 (1) | 3.3/6 (1) |
| at-tourism-2months | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 33 s | 257,701 / 2,324 | 0.37 | – | 6/6 |
| at-tourism-2months | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 14 s | 48,123 / 1,254 | 0.0094 | 13 (1) | 2.0/6 (1) |
| rs-geodetic-licenses | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 8 s | 19,414 / 554 | 0.0032 | 5 (0) | 6.7/7 (6) |
| rs-geodetic-licenses | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 32 s | 249,953 / 2,094 | 0.34 | – | 7/7 |
| rs-geodetic-licenses | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 9 s | 10,857 / 619 | 0.0032 | 6 (0) | 6.3/7 (6) |
| uk-beis-spending | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 9 s | 26,318 / 455 | 0.0040 | 6 (0) | 6.7/7 (6) |
| uk-beis-spending | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 46 s | 250,912 / 3,070 | 0.36 | – | 6/7 |
| uk-beis-spending | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 6 s | 5,465 / 480 | 0.0017 | 5 (0) | 6.7/7 (6) |
| de-berlin-grants | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 9 s | 24,366 / 529 | 0.0038 | 5 (0) | 5.0/5 (5) |
| de-berlin-grants | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 57 s | 296,433 / 3,792 | 0.38 | – | 5/5 |
| de-berlin-grants | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 7 s | 5,407 / 433 | 0.0020 | 5 (0) | 5.0/5 (5) |
| pt-tax-benefits | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 8 s | 17,302 / 776 | 0.0037 | 6 (0) | 4.3/5 (4) |
| pt-tax-benefits | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 106 s | 329,381 / 3,172 | 0.46 | – | 5/5 |
| pt-tax-benefits | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 18 s | 11,342 / 674 | 0.0031 | 6 (0) | 5.0/5 (5) |
| owid-energy | hedgehog | 2026-10-22 | gpt-4.1-mini | 3 | 9 s | 30,599 / 505 | 0.0050 | 5 (1) | 4.0/5 (4) |
| owid-energy | claude | 2026-10-21 | claude-opus-5-5[1m] | 1 | 13 s | 114,647 / 1,304 | 0.24 | – | 5/5 |
| owid-energy | swap-hedgehog | 2026-10-22 | gpt-4.1-mini-2025-04-14 | 3 | 8 s | 9,723 / 487 | 0.0024 | 6 (0) | 5.0/5 (5) |

## pt-contracts: Portugal's 2020 procurement register (176k contracts, 4.5M cells)

File: `eu_pt_opendata/001_contratos2020.xlsx`

> Which 5 public bodies spent the most on contracts in 2020, and how much each?
> Which suppliers won the most contracts, and what share of the total value went to the top 10?
> Compare direct awards (Ajuste Direto) with public tenders (Concurso público): how many contracts and total value for each?
> How many contracts were signed per month in 2020? Was there a jump when COVID started in March?

| Check | hedgehog | claude | claude-hh | swap-hedgehog |
|---|---|---|---|---|
| top body: Infraestruturas de Portugal, €737.7 M (6 spellings merged by tax ID) | 3/3 | ✓ | ✓ | 3/3 |
| Metro do Porto €352.4 M | 3/3 | ✓ | ✓ | 3/3 |
| does not list the same body twice under different spellings | ? | (✓) | ? | ? |
| uses the contract price (precoContratual), total €11.16 bn | ? | (✓) | ? | ? |
| ranks suppliers by number of contracts (Medtronic ~1,140–1,152, Generis ~1,130–1,231) | ? | (✓) | ? | ? |
| gives the top-10 suppliers' share of total value: ≈ 3.6–3.9 % for the top 10 by number of contracts (the question's reading) or ≈ 9–13 % for the top 10 by value | 2/3 | ✓ | ✓ | 2/3 |
| Ajuste Direto Regime Geral: 78,349 contracts (±0.2 %) | 3/3 | ✓ | ✓ | 3/3 |
| Ajuste Direto Regime Geral: €2.41 bn | 3/3 | ✓ | ✓ | 3/3 |
| Concurso público: 17,408 contracts | 3/3 | ✓ | ✓ | 3/3 |
| Concurso público: €5.48 bn | 3/3 | ✓ | ✓ | 3/3 |
| January: 16,154 contracts (±0.2 %) | 3/3 | ✓ | ✓ | 3/3 |
| April: 10,682 contracts (±0.2 %) | 3/3 | ✓ | ✓ | 3/3 |
| says there was no overall jump in March (counts fell in April–May); noting that COVID-related purchases rose is a plus, not required (the question does not ask for it) | ? | (✓) | ? | ? |

- **claude** (`20261021T070316Z-pt-contracts-claude`): grouped by tax ID, removed 20 duplicates, explained 26,723 undated simplified awards, both top-10 shares (4.0 % by count, 11.4 % by value), COVID: no overall jump but COVID purchases surged

Answers (latest run): [hedgehog](answers/pt-contracts.hedgehog.md), [claude](answers/pt-contracts.claude.md), [claude-hh](answers/pt-contracts.claude-hh.md), [swap-hedgehog](answers/pt-contracts.swap-hedgehog.md)
Backlog items from this case: B1, B2, B3, B4, B5, B9

## at-pension: Austrian pension equalization supplement (small table, mixed measures, total row)

File: `eu_at_opendata/003_pen_02_02.xlsx`

> How many people received the equalization supplement (Ausgleichszulage) in total in 2024 and in 2025? What was the average monthly amount for single and for married recipients in 2025?

| Check | hedgehog | claude | claude-hh | swap-hedgehog |
|---|---|---|---|---|
| total recipients 2024: 190,423 (the 'Gesamt' row) | 3/3 | ✓ | ✓ | 1/3 |
| total recipients 2025: 189,537 | 3/3 | ✓ | ✓ | 1/3 |
| average 2025, single: €403.19 | 3/3 | ✓ | ✓ | 3/3 |
| average 2025, married: €594.20 | 3/3 | ✓ | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/at-pension.hedgehog.md), [claude](answers/at-pension.claude.md), [claude-hh](answers/at-pension.claude-hh.md), [swap-hedgehog](answers/at-pension.swap-hedgehog.md)
Backlog items from this case: B6, B7

## enron-henry-hub: Enron Henry Hub gas flows (7 tables, daily grids, labels only in the first table)

File: `enron/andrea_ring__3__HHmonthlyavg.xlsx`

> This is a Henry Hub natural gas flow report. Which 3 pipelines had the highest month-to-date average receipts in February 2002, and how much each? What were total receipts and total deliveries, and what is the net flow? On which day in January 2002 were total receipts highest, and how much?

| Check | hedgehog | claude | claude-hh | swap-hedgehog |
|---|---|---|---|---|
| SEA ROBIN 123,367 | 3/3 | ✓ | ✓ | 3/3 |
| NGPL 105,538 | 3/3 | ✓ | ✓ | 3/3 |
| MAINLINE 100,733 | 3/3 | ✓ | ✓ | 3/3 |
| total receipts 424,772 | 3/3 | ✓ | ✓ | 3/3 |
| total deliveries 214,246 (shown as −214,246) | 3/3 | ✓ | ✓ | 3/3 |
| net flow +210,526 | 2/3 | ✓ | ✓ | 3/3 |
| highest January receipts: 606,356 | 3/3 | ✓ | ✓ | 3/3 |
| on 23 January 2002 | 3/3 | ✓ | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/enron-henry-hub.hedgehog.md), [claude](answers/enron-henry-hub.claude.md), [claude-hh](answers/enron-henry-hub.claude-hh.md), [swap-hedgehog](answers/enron-henry-hub.swap-hedgehog.md)
Backlog items from this case: B8

## de-truck-toll: German truck-toll mileage index (daily series 2008–2026, 9 sheets, statistical report)

File: `eu_de_destatis/statistischer-bericht-lkw-maut-fahrleistungsindex-5421901.xlsx`

> Using the daily truck-toll mileage index for Germany: what was the highest unadjusted daily value in 2023, and on which date? What was the average calendar- and seasonally-adjusted (KSB) index in 2023 and in 2024, and did it go up or down?

| Check | hedgehog | claude | claude-hh | swap-hedgehog |
|---|---|---|---|---|
| highest unadjusted 2023 value: 153.0 | 3/3 | ✓ | ✓ | 3/3 |
| on 31 May 2023 | 3/3 | ✓ | ✓ | 3/3 |
| KSB average 2023: 95.61 | 3/3 | ✓ | ✓ | 3/3 |
| KSB average 2024: 94.92 | 3/3 | ✓ | ✓ | 3/3 |
| says it went down | ? | (✓) | ? | ? |

- **claude** (`20261021T070453Z-de-truck-toll-claude`): also noted the index base (2021=100) and that Destatis calls the daily adjusted series experimental

Answers (latest run): [hedgehog](answers/de-truck-toll.hedgehog.md), [claude](answers/de-truck-toll.claude.md), [claude-hh](answers/de-truck-toll.claude-hh.md), [swap-hedgehog](answers/de-truck-toll.swap-hedgehog.md)

## enron-om-2plants: Two Enron peaker plants' draft 2001 O&M budgets (multi-file; monthly columns, subtotals and totals)

Files: `enron/benjamin_rogers__1115__Gleason 01 O&M Budget.xlsx`, `enron/benjamin_rogers__1117__Wheatland 01 O&M Budget.xlsx`

> These are the draft 2001 operations & maintenance budgets of two Enron power plants, Gleason and Wheatland.
> What is each plant's total 2001 budget (all expenses, including owner's expenses and taxes), and what is the total for both plants together?
> Which plant budgets more for variable O&M, and how much?
> In which month is Gleason's total expense budget highest, and how much is it?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| Gleason total expenses 2001: $2,869,475.84 | 3/3 | ✗ | 3/3 |
| Wheatland total expenses 2001: $4,452,830 | 3/3 | ✗ | 3/3 |
| both plants: $7,322,305.84 | 3/3 | ✓ | 3/3 |
| Wheatland variable O&M $2,200,688 (Gleason $901,100) | 3/3 | ✓ | 3/3 |
| Gleason's highest month: May 2001, $260,734.24 | 3/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/enron-om-2plants.hedgehog.md), [claude](answers/enron-om-2plants.claude.md), [swap-hedgehog](answers/enron-om-2plants.swap-hedgehog.md)

## enron-newco-cashflow: Enron 'Newco' development cash-flow summary (values in $000s, wide year columns, label-less second sheet)

File: `enron/benjamin_rogers__1058__newco development cash flow.xlsx`

> This is a development financial summary for 'Newco', a power development company.
> What is the total net income of all projects in 2005 and in 2020, in dollars?
> What is the total aftertax cash in 2010, in dollars?
> For the generic 500 MW combined-cycle unit, what are the NPV at 14% (in dollars) and the IRR?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| net income 2005: $259.96 M (259,957.9 in $000s) | 3/3 | ✓ | 2/3 |
| net income 2020: $1.024 bn | 3/3 | ✓ | 1/3 |
| aftertax cash 2010: $362.13 M | 3/3 | ✓ | 1/3 |
| NPV @ 14%: $4.25 M (4,254.7 in $000s) | 1/3 | ✓ | 3/3 |
| IRR 14.5 % | 3/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/enron-newco-cashflow.hedgehog.md), [claude](answers/enron-newco-cashflow.claude.md), [swap-hedgehog](answers/enron-newco-cashflow.swap-hedgehog.md)

## enron-deals-pl: Enron gas desk deal log (18 deals; trader name variants; catalog wrongly calls the 'Sumas' rows totals)

File: `enron/barry_tycholiz__889__June 1 P&L.xlsx`

> This is a log of gas deals done since April 1, 2001.
> What is the total origination across all deals?
> Which trader generated the most origination, and how much?
> How much origination came from physical deals and how much from financial deals?
> Which delivery location brought the most origination, and how much?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| total origination $135,666.48 (all 18 rows, including the 'Sumas' ones) | 3/3 | ✓ | 3/3 |
| McKay $78,653.38 ('Mc Kay' + 'McKay' merged) | 0/3 | ✓ | 0/3 |
| physical $66,089.60 | 3/3 | ✓ | 3/3 |
| financial $69,576.88 | 3/3 | ✓ | 3/3 |
| Sumas $77,603.38 | 2/3 | ✓ | 3/3 |
| treats 'Mc Kay' and 'McKay' as one trader | ? | (✓) | ? |

- **claude** (`20261021T231721Z-enron-deals-pl-claude`): merged 'Mc Kay' + 'McKay' = $78,653.38 and says so

Answers (latest run): [hedgehog](answers/enron-deals-pl.hedgehog.md), [claude](answers/enron-deals-pl.claude.md), [swap-hedgehog](answers/enron-deals-pl.swap-hedgehog.md)

## enron-mdea-pl: Enron/MDEA profit-sharing report (14 daily sheets, stacked day-ahead and real-time blocks; layout shifts from May 10, catalog loses the May 12–14 profit columns)

File: `enron/craig_dean__4381__MDEA P&L Report.xlsx`

> This is the MDEA profit-sharing report for May 1–14, 2001, one sheet per day.
> For the day-ahead transactions only: what is EPMI's total net profit and MDEA's total net savings over the 14 days?
> On which day were MDEA's day-ahead savings largest, and how much were they?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| EPMI day-ahead profit: $44,058.18 (sum of S32 on the 14 sheets) | 1/3 | ✓ | 0/3 |
| MDEA day-ahead savings: $127,882.48 (sum of T32) | 1/3 | ✓ | 0/3 |
| May 5, 2001 | 1/3 | ✓ | 0/3 |
| MDEA savings on May 5: $15,056.52 | 1/3 | ✓ | 0/3 |

Answers (latest run): [hedgehog](answers/enron-mdea-pl.hedgehog.md), [claude](answers/enron-mdea-pl.claude.md), [swap-hedgehog](answers/enron-mdea-pl.swap-hedgehog.md)

## enron-storage-inventory: NewPower gas storage inventory valuation (two stacked blocks with subtotals, month-end columns, price rows below)

File: `enron/barry_tycholiz__846__NPC Inventory Valuation.xlsx`

> This is NewPower's gas storage inventory valuation by month-end (pipeline storage and LDC/virtual storage).
> At which month-end is total storage inventory highest, and how much is it?
> At the end of October 2001, how much is held in Columbia Gas (COH) pipeline storage in total, and how much in LDC storage?
> What is the end-of-October inventory worth at the current NYMEX price and at the winter NYMEX price?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| peak total volumes 12,427,743 | 0/3 | ✓ | 1/3 |
| end of October 2001 | 3/3 | ✓ | 3/3 |
| COH pipeline storage end of October: 8,285,615 (COH + COH (AES)) | 0/3 | ✓ | 1/3 |
| LDC storage end of October: 2,340,524 | 0/3 | ✓ | 1/3 |
| value at current NYMEX ($3.42): $42,502,881 | 2/3 | ✓ | 1/3 |
| value at winter NYMEX ($3.745): $46,541,528 | 3/3 | ✓ | 1/3 |

Answers (latest run): [hedgehog](answers/enron-storage-inventory.hedgehog.md), [claude](answers/enron-storage-inventory.claude.md), [swap-hedgehog](answers/enron-storage-inventory.swap-hedgehog.md)

## at-salzburg-budget: State of Salzburg 2019 operating budget (3,206 budget lines, expenses and revenues in one amount column)

File: `eu_at_opendata/013_budget-Operating-statement-szg-position-2019.xls`

> This is the 2019 operating budget (Ergebnishaushalt) of the Austrian state of Salzburg.
> What are total planned expenses and total planned revenues, and does the budget show a surplus or a deficit (how much)?
> Which function area (Haushaltsgruppe) has the largest expenses, how much, and what share of all expenses is that?
> How much is planned for personnel expenses?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| expenses €2,641.70 M | 3/3 | ✓ | 3/3 |
| revenues €2,771.27 M | 3/3 | ✓ | 3/3 |
| surplus €129.57 M | 3/3 | ✓ | 3/3 |
| calls it a surplus | 3/3 | ✓ | 3/3 |
| Gesundheit (health) €775.41 M | 3/3 | ✗ | 3/3 |
| health ≈ 29.4 % of expenses | 3/3 | ✗ | 3/3 |
| personnel expenses €894.67 M | 3/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/at-salzburg-budget.hedgehog.md), [claude](answers/at-salzburg-budget.claude.md), [swap-hedgehog](answers/at-salzburg-budget.swap-hedgehog.md)

## rs-pancevo-budget: Pančevo 2020 budget: revenues file + expenditures file (multi-file, Cyrillic headers, several funding sources)

Files: `data_gov_rs/pancevo_budzet_2020_prihodi.xlsx`, `data_gov_rs/pancevo_budzet_2020_rashodi.xlsx` (under `clean/`)

> These two files are the 2020 budget of the city of Pančevo (Serbia): planned revenues and receipts, and planned expenditures.
> What are total planned revenues and receipts, and total planned expenditures from all sources? Is the budget balanced?
> How much of the revenue side comes from taxes, and what share of total revenues and receipts is that?
> How much of the revenue side is unallocated surplus carried over from earlier years?
> Which function gets the most money (all sources), and how much?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| revenues and receipts RSD 5,804,251,672 | 3/3 | ✓ | 3/3 |
| expenditures all sources RSD 5,804,251,672 (budget 4.81 bn + other 0.99 bn) | 3/3 | ✓ | 3/3 |
| says revenues and expenditures are equal (balanced), using all sources | ? | (✓) | ? |
| taxes RSD 3,888,600,000 | 3/3 | ✓ | 3/3 |
| taxes ≈ 67.0 % of revenues | 3/3 | ✓ | 3/3 |
| carried-over surplus RSD 946,971,409 | 3/3 | ✓ | 3/3 |
| road transport (Друмски саобраћај) RSD 1,071,519,412 | 3/3 | ✓ | 3/3 |

- **claude** (`20261021T231940Z-rs-pancevo-budget-claude`): equal to the dinar; notes the balance relies on the carried-over surplus

Answers (latest run): [hedgehog](answers/rs-pancevo-budget.hedgehog.md), [claude](answers/rs-pancevo-budget.claude.md), [swap-hedgehog](answers/rs-pancevo-budget.swap-hedgehog.md)

## us-superstore: Superstore orders 2021–2024 (10k order lines, US formats, Returns on a second sheet)

File: `superstore/sample_superstore.xls` (under `clean/`)

> These are Superstore order lines for 2021–2024, with a separate Returns sheet.
> What were total sales, and which region sold the most (how much)?
> By what percentage did sales grow from 2023 to 2024?
> Which sub-category lost the most money, and how much?
> What share of orders was returned, and how much in sales did those returned orders represent?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| total sales $2,326,534.36 | 3/3 | ✓ | 3/3 |
| West $739,813.61 | 3/3 | ✓ | 3/3 |
| 2024 vs 2023: +21.4 % | 3/3 | ✓ | 3/3 |
| Tables lost $17,753.21 | 3/3 | ✓ | 3/3 |
| 296 of 5,111 orders returned = 5.8 % | 3/3 | ✓ | 1/3 |
| sales of returned orders $180,504.28 | 0/3 | ✓ | 0/3 |

Answers (latest run): [hedgehog](answers/us-superstore.hedgehog.md), [claude](answers/us-superstore.claude.md), [swap-hedgehog](answers/us-superstore.swap-hedgehog.md)

## ms-financial-sample: Microsoft financial sample (700 sales rows; 2013 covers only Sep–Dec)

File: `microsoft/financial_sample.xlsx` (under `clean/`)

> This is a sales and profit table by segment, country, product and month.
> Which country was the most profitable, and what was its profit?
> Which customer segment loses money overall, and how much?
> What was the overall profit margin (profit as a share of net sales)?
> Which product sold the most units, and how many?
> Did profit grow from 2013 to 2014?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| France $3,781,020.78 | 3/3 | ✓ | 3/3 |
| Enterprise −$614,545.63 | 3/3 | ✓ | 3/3 |
| margin 14.2 % | 3/3 | ✓ | 3/3 |
| Paseo 338,239.5 units | 3/3 | ✓ | 3/3 |
| notes that 2013 covers only September–December, so the yearly totals are not comparable (same months: $3.88 M → $5.44 M) | ? | (✓) | ? |

- **claude** (`20261021T232042Z-ms-financial-sample-claude`): 2013 covers only Sep–Dec; same months $3.88 M → $5.44 M (+40 %)

Answers (latest run): [hedgehog](answers/ms-financial-sample.hedgehog.md), [claude](answers/ms-financial-sample.claude.md), [swap-hedgehog](answers/ms-financial-sample.swap-hedgehog.md)

## nw-orders: Northwind orders + order lines (multi-file CSV; revenue needs a join across the two files)

Files: `northwind/orders.csv`, `northwind/order_details.csv` (under `clean/`)

> These are Northwind's orders and order lines (unit price, quantity, discount).
> How many orders were placed in 1997, and what was their net revenue (unit price × quantity, less discount)?
> Which employee (employee ID) brought in the most 1997 revenue, and how much?
> How many orders were shipped after their required date?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| 408 orders | 3/3 | ✓ | 3/3 |
| net revenue $617,085.20 (orderID 10400–10807 = the 1997 orders) | 3/3 | ✓ | 1/3 |
| employee 4: $128,809.79 | 3/3 | ✓ | 3/3 |
| 37 orders shipped late | 3/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/nw-orders.hedgehog.md), [claude](answers/nw-orders.claude.md), [swap-hedgehog](answers/nw-orders.swap-hedgehog.md)

## us-chicago-salaries: City of Chicago employees and salaries (32k rows; hourly staff have no annual salary)

File: `chicago_open_data/employee_salaries.csv` (under `clean/`)

> This is the list of City of Chicago employees with their salaries.
> Which department has the most employees, and how many?
> How many employees are paid hourly and how many are salaried?
> What is the average annual salary of a Police Officer (job title)?
> Who is the highest-paid employee, and what is the salary?
> What is the total annual salary bill, and what does that figure leave out?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| Chicago Police Department 12,221 | 3/3 | ✓ | 3/3 |
| 7,129 hourly | 3/3 | ✓ | 3/3 |
| 24,893 salaried | 3/3 | ✓ | 3/3 |
| Police Officer average $110,466.50 | 0/3 | ✓ | 0/3 |
| Michael J. McMurray (Commissioner of Aviation) | 3/3 | ✓ | 3/3 |
| $360,504 | 3/3 | ✓ | 3/3 |
| total $2.89 bn covers salaried staff only; hourly workers (7,129) have only an hourly rate | ? | (✓) | ? |

- **claude** (`20261021T232130Z-us-chicago-salaries-claude`): leaves out all 7,129 hourly workers (rate only; ~$0.69 bn more), plus overtime/benefits

Answers (latest run): [hedgehog](answers/us-chicago-salaries.hedgehog.md), [claude](answers/us-chicago-salaries.claude.md), [swap-hedgehog](answers/us-chicago-salaries.swap-hedgehog.md)

## us-nyc-payroll: NYC citywide payroll extract, FY2014–2025 (100k person-year rows, 2 agencies; NYPD question not answerable)

File: `nyc_open_data/citywide_payroll_100k.csv` (under `clean/`)

> This is an extract of New York City's citywide payroll (one row per employee and fiscal year).
> For the Administration for Children's Services: how much overtime was paid in fiscal year 2025, and in which fiscal year was overtime highest (how much)?
> What was the average base salary of a Child Protective Specialist in FY2015 and in FY2025, and by what percentage did it rise?
> How much overtime did the NYPD pay in FY2025?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| ACS overtime FY2025 $52.45 M | 3/3 | ✓ | 3/3 |
| highest: FY2019, $61.34 M | 3/3 | ✓ | 3/3 |
| CPS FY2015 $50,800.58 | 2/3 | ✓ | 3/3 |
| CPS FY2025 $66,449.96 | 2/3 | ✓ | 3/3 |
| +30.8 % | 2/3 | ✓ | 3/3 |
| says the file has no NYPD rows (only ACS and the Office of Administrative Trials and Hearings), so it cannot answer | ? | (✓) | ? |

- **claude** (`20261021T232201Z-us-nyc-payroll-claude`): file has no NYPD rows (only ACS and Admin Trials and Hearings), cannot answer

Answers (latest run): [hedgehog](answers/us-nyc-payroll.hedgehog.md), [claude](answers/us-nyc-payroll.claude.md), [swap-hedgehog](answers/us-nyc-payroll.swap-hedgehog.md)

## uk-gender-pay-gap: UK gender pay gap reports 2024/25 (11k employers; catalog flags 3 'TOTAL …' company names as totals and reads 25 values like 40.805 as 40805)

File: `uk_gov/gender_pay_gap_2024_25.csv` (under `clean/`)

> These are the UK gender pay gap reports for 2024/25, one row per employer.
> How many employers reported?
> Across employers, what is the median of the median hourly pay gap?
> What share of employers report a median hourly gap in favour of men?
> By employer size band, what is the average median hourly gap, and which band has the highest?
> How many reports were submitted after the deadline?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| 11,314 employers (none of them is a total row) | 3/3 | ✓ | 3/3 |
| median of medians 8.4 % | 3/3 | ✓ | 3/3 |
| 8,770 of 11,314 = 77.5 % | 3/3 | ✓ | 1/3 |
| highest band: 500 to 999, average 12.06 % (hedgehog's misread values give 42.06) | 3/3 | ✓ | 3/3 |
| 764 late reports | 3/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/uk-gender-pay-gap.hedgehog.md), [claude](answers/uk-gender-pay-gap.claude.md), [swap-hedgehog](answers/uk-gender-pay-gap.swap-hedgehog.md)

## pt-social-report: Portuguese municipality's 2016 social report (one sheet, 26 stacked tables, M/F/T rows, merged headers)

File: `eu_pt_opendata/012_balanco-social-2016.xls`

> This is a Portuguese municipality's 2016 'Balanço Social' (annual HR report).
> How many workers were there at 31 December 2016, and how many of them were women (what share)?
> Which job group (cargo/carreira) is the largest, and how many workers does it have?
> What were total personnel costs in 2016, and what share of that was base pay?
> How many service providers (prestadores de serviços) were there?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| 7,478 workers | 3/3 | ✓ | 3/3 |
| 3,245 women | 3/3 | ✓ | 3/3 |
| 43.4 % | 3/3 | ✓ | 3/3 |
| Técnico Superior 2,148 (just above Assistente Operacional 2,138) | 0/3 | ✓ | 0/3 |
| personnel costs €216,259,993.16 | 3/3 | ✓ | 3/3 |
| base pay 62.6 % | 2/3 | ✓ | 2/3 |
| 560 service providers | 3/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/pt-social-report.hedgehog.md), [claude](answers/pt-social-report.claude.md), [swap-hedgehog](answers/pt-social-report.swap-hedgehog.md)

## at-cpi: Austrian consumer price index since 1990 (monthly rows interleaved with 'Ø' annual-average rows; 2026 incomplete)

File: `eu_at_statistik_austria/005_2_Verbraucherpreisindizes_ab_1990.ods`

> This is Austria's consumer price index (VPI).
> What was the annual inflation rate in 2022 and in 2023?
> Which month had the highest year-on-year inflation, and what was it?
> By how much did the index (base 2020 = 100) rise from the 2020 annual average to the 2025 annual average?
> What was the annual inflation rate for 2026?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| 2022: 8.6 % (the official annual average, not the 8.53 mean of monthly rates) | 2/3 | ✓ | 1/3 |
| 2023: 7.8 % | 1/3 | ✓ | 1/3 |
| peak 11.2 % | 2/3 | ✓ | 3/3 |
| January 2023 | 1/3 | ✓ | 3/3 |
| +28.2 points = +28.2 % (100 → 128.2; base 2020 = 100, so points and percent coincide) | 3/3 | ✓ | 3/3 |
| says there is no annual 2026 figure yet (monthly data only to September 2026); may quote the latest month (3.6 %) or the year-to-date average | ? | (✓) | ? |

- **claude** (`20261021T232322Z-at-cpi-claude`): no annual 2026 figure yet; data to September 2026 (3.6 %), partial-year mean ~3.0 % flagged as not official

Answers (latest run): [hedgehog](answers/at-cpi.hedgehog.md), [claude](answers/at-cpi.claude.md), [swap-hedgehog](answers/at-cpi.swap-hedgehog.md)

## wb-gdp-wide: World Bank GDP per capita, 1960–2025 wide years (countries mixed with aggregates; catalog invents a 'group' column and calls Somalia a total)

File: `world_bank/wdi_gdp_per_capita_wide_years.csv` (under `clean/`)

> This is the World Bank's GDP per capita (current US$) for countries and regional/income aggregates, 1960–2025.
> Which country (not an aggregate) had the highest GDP per capita in 2024, and how much?
> What was Serbia's GDP per capita in 2010 and in 2024, and by what percentage did it grow?
> What was Poland's GDP per capita in 2024 as a percentage of the European Union average?
> What was Somalia's GDP per capita in 2024?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| Monaco $288,001.57 | 3/3 | ✓ | 3/3 |
| Serbia 2010 $5,970.93 | 3/3 | ✓ | 3/3 |
| Serbia 2024 $13,677.78 | 3/3 | ✓ | 3/3 |
| +129.1 % | 3/3 | ✓ | 3/3 |
| Poland = 58.0 % of the EU | 2/3 | ✓ | 2/3 |
| Somalia $629.54 | 2/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/wb-gdp-wide.hedgehog.md), [claude](answers/wb-gdp-wide.claude.md), [swap-hedgehog](answers/wb-gdp-wide.swap-hedgehog.md)

## eu-electricity-prices: Eurostat household electricity prices (SDMX long format, codes for band/tax/currency, half-years 2019–2026)

File: `eurostat/nrg_pc_204_electricity_prices_households.csv` (under `clean/`)

> This is Eurostat's household electricity price dataset.
> For households consuming 2,500–4,999 kWh a year, prices including all taxes, in EUR per kWh:
> which country had the highest price in the latest half-year, and what was it?
> What was Germany's price then, and how much did it change compared with the second half of 2022 (in %)?
> What was the EU average in the second half of 2025?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| Ireland 0.3871 EUR/kWh (2026-S1) | 3/3 | ✓ | 3/3 |
| Germany 0.3618 EUR/kWh | 3/3 | ✓ | 3/3 |
| +7.8 % vs 2022-S2 (0.3357) | 2/3 | ✓ | 3/3 |
| EU27 2025-S2: 0.2896 EUR/kWh | 1/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/eu-electricity-prices.hedgehog.md), [claude](answers/eu-electricity-prices.claude.md), [swap-hedgehog](answers/eu-electricity-prices.swap-hedgehog.md)

## de-gdp: Destatis national accounts, first results (56 sheets, 154 tables: each sheet stacks levels, % changes and index; units in a dropped header row)

File: `eu_de_destatis/inlandsprodukt-erste-ergebnisse-xlsx-2180110.xlsx`

> These are Germany's national accounts, first results (Destatis).
> What was nominal GDP in 2025, in euros, and the price-adjusted (real) GDP growth rate versus 2024?
> What was the general government deficit in 2025, in euros and as a percentage of GDP?
> How many people lived in Germany in 2025, and what was the unemployment rate?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| €4,470.48 bn | 3/3 | ✓ | 3/3 |
| real growth +0.2 % (calendar-adjusted +0.3 %) | 3/3 | ✓ | 2/3 |
| deficit €107.42 bn | 3/3 | ✓ | 3/3 |
| 2.4 % of GDP | 1/3 | ✓ | 2/3 |
| 83.53 million (83,531 thousand) | 3/3 | ✓ | 3/3 |
| unemployment rate 3.5 % | 3/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/de-gdp.hedgehog.md), [claude](answers/de-gdp.claude.md), [swap-hedgehog](answers/de-gdp.swap-hedgehog.md)

## uk-ons-labour: ONS labour market summary (27 sheets, notes-heavy; headline rates defined in footnotes; catalog uses 'Date of publication' as a column name)

File: `uk_ons/a01_labour_market_summary_sep2026.xls`

> This is the ONS labour market summary (September 2026 release).
> For the latest period in Table 1: what are the headline employment rate and the headline unemployment rate?
> How many people were in employment and how many were unemployed?
> Compared with the previous non-overlapping three-month period, did the unemployment rate rise or fall?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| latest period May–Jul 2026 | 2/3 | ✓ | 2/3 |
| headline employment rate (16–64) 75.1 % | 3/3 | ✓ | 3/3 |
| headline unemployment rate (16+) 4.9 % | 3/3 | ✓ | 3/3 |
| 34.48 million in employment (16+) | 3/3 | ✓ | 3/3 |
| 1.78 million unemployed | 2/3 | ✓ | 3/3 |
| compares with Feb–Apr 2026 (4.89 %): about unchanged / up 0.02 points | ? | (✓) | ? |

- **claude** (`20261021T232522Z-uk-ons-labour-claude`): Feb–Apr 2026 4.886 % → 4.905 %: up ~0.02 points, essentially unchanged at one decimal

Answers (latest run): [hedgehog](answers/uk-ons-labour.hedgehog.md), [claude](answers/uk-ons-labour.claude.md), [swap-hedgehog](answers/uk-ons-labour.swap-hedgehog.md)

## fr-insee-pop: INSEE population by département, sex and age (49 yearly sheets; Ensemble/Hommes/Femmes blocks side by side; catalog takes the Somme département for a total row)

File: `eu_fr_insee/002_estim-pop-dep-sexe-gca-1975-2023.xls`

> These are INSEE population estimates by département, sex and age group, one sheet per year.
> What was the population of metropolitan France, and of France including the overseas departments, in 2023?
> How did the population of Paris change between 1990 and 2023 (number and %)?
> What was the population of the Somme département in 2023?
> What share of metropolitan France's 2023 population was aged 75 or over?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| metropolitan France 65,834,837 | 3/3 | ✓ | 3/3 |
| with DOM 68,042,591 | 3/3 | ✓ | 3/3 |
| Paris 2023: 2,102,650 | 3/3 | ✓ | 3/3 |
| Paris 1990: 2,150,147 | 3/3 | ✓ | 3/3 |
| −47,497 = −2.2 % | 3/3 | ✓ | 3/3 |
| Somme 564,067 | 3/3 | ✓ | 3/3 |
| 75+ = 10.2 % | 3/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/fr-insee-pop.hedgehog.md), [claude](answers/fr-insee-pop.claude.md), [swap-hedgehog](answers/fr-insee-pop.swap-hedgehog.md)

## at-tourism-2months: Austrian tourism by country of origin, July and August 2026 (multi-file; 10 sheets per file; Germany total above its 7 regions)

Files: `eu_at_statistik_austria/012_TourismusJuli2026HerkunftEndg.ods`, `eu_at_statistik_austria/010_TourismusAugust2026HerkunftEndg.ods`

> These two files are Statistik Austria's tourism figures by guests' country of origin for July 2026 and August 2026 (Austria and each federal state).
> How many overnight stays were there in Austria in July and in August, and in both months together?
> How many overnight stays in August came from German guests?
> Which federal state had the most overnight stays in August, and how many?
> In which month did guests stay longer on average?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| July 19,564,196 | 2/3 | ✓ | 1/3 |
| August 21,442,710 | 2/3 | ✓ | 1/3 |
| both months 41,006,906 | 1/3 | ✓ | 0/3 |
| German guests in August 8,530,796 (the 'Deutschland gesamt' row, not one region) | 3/3 | ✓ | 3/3 |
| Tirol 6,473,672 | 0/3 | ✓ | 0/3 |
| August: 3.4 nights (July 3.3) | 2/3 | ✓ | 1/3 |

Answers (latest run): [hedgehog](answers/at-tourism-2months.hedgehog.md), [claude](answers/at-tourism-2months.claude.md), [swap-hedgehog](answers/at-tourism-2months.swap-hedgehog.md)

## rs-geodetic-licenses: Serbian register of geodetic licences (3.6k people, Cyrillic, empty employer cells)

File: `data_gov_rs/rgz_registar_geodetskih_licenci.ods` (under `clean/`)

> This is the Serbian register of geodetic professionals and their licences.
> How many people are in the register, and how many hold a first-order licence, a second-order licence, or no licence?
> What share of registered people hold any licence?
> Which company employs the most registered people, and how many?
> How many registered people have no employer listed?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| 3,605 people | 3/3 | ✓ | 3/3 |
| first-order 875 | 3/3 | ✓ | 3/3 |
| second-order 655 | 3/3 | ✓ | 3/3 |
| no licence 2,075 | 3/3 | ✓ | 3/3 |
| 42.4 % licensed | 3/3 | ✓ | 3/3 |
| PREMER SAVKOVIĆ DOO Lazarevac: 46 | 2/3 | ✓ | 1/3 |
| 1,261 without an employer | 3/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/rs-geodetic-licenses.hedgehog.md), [claude](answers/rs-geodetic-licenses.claude.md), [swap-hedgehog](answers/rs-geodetic-licenses.swap-hedgehog.md)

## uk-beis-spending: UK BEIS payments over £25k, March 2018 (4.3k transactions, DD/MM/YYYY dates, refunds as negatives)

File: `uk_gov/beis_spending_over_25k_2018_03.csv` (under `clean/`)

> This is the UK Department for Business, Energy and Industrial Strategy's list of payments over £25,000 for March 2018.
> What is the total paid, and how many payment lines are negative (credits)?
> Which supplier received the most, and what share of the total is that?
> How much went to the research councils in total?
> On which day was the most paid, and how much?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| £1,930,341,058.77 | 3/3 | ✓ | 3/3 |
| 49 negative lines | 3/3 | ✓ | 3/3 |
| Post Office Ltd £699.82 M | 3/3 | ✓ | 3/3 |
| 36.3 % of the total | 3/3 | ✓ | 3/3 |
| research councils £342.57 M (6 councils) | 2/3 | ✗ | 2/3 |
| 7 March 2018 | 3/3 | ✓ | 3/3 |
| £401.11 M on 7 March | 3/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/uk-beis-spending.hedgehog.md), [claude](answers/uk-beis-spending.claude.md), [swap-hedgehog](answers/uk-beis-spending.swap-hedgehog.md)

## de-berlin-grants: Berlin grants database 2021–2025 (55k grants, ';' CSV, recipient spelling variants with an ID; catalog takes 'Zusammen …'/'SUMA …' recipients for totals)

File: `govdata_de/berlin_zuwendungsdatenbank.csv` (under `clean/`)

> This is the State of Berlin's grants database (Zuwendungsdatenbank), 2021–2025.
> How much was granted in 2024 in total, in how many grants?
> What share of the 2024 money was institutional funding (as opposed to project funding)?
> Which recipient received the most in 2024 (count all spellings of the same recipient together), and how much?
> Which year had the highest total, and how much?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| €2,644,310,862 | 3/3 | ✓ | 3/3 |
| 9,810 grants | 3/3 | ✓ | 3/3 |
| 22.9 % institutional (both spellings of 'institutionelle Förderung') | 3/3 | ✓ | 3/3 |
| Forschungsverbund Berlin €113.08 M (5 spellings, one empfaengerid) | 3/3 | ✓ | 3/3 |
| 2025: €3.695 bn | 3/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/de-berlin-grants.hedgehog.md), [claude](answers/de-berlin-grants.claude.md), [swap-hedgehog](answers/de-berlin-grants.swap-hedgehog.md)

## pt-tax-benefits: Portugal's 2023 tax-benefit beneficiaries (8 sheets, one per tax; IRC sheet 63k × 87 = 5.5M-cell range; long legal column names)

File: `eu_pt_opendata/008_BF_1_2023_beneficiarios_por_tipo_de_imposto_e_beneficio.xlsx`

> This is the Portuguese tax authority's list of beneficiaries of tax benefits in 2023, one sheet per tax.
> How many entities received corporate income tax (IRC) benefits?
> What was the total VAT refunded (restituição do IVA), and which entity received the largest VAT refund (how much)?
> What share of the VAT refunds went to the armed and security forces category?
> How many fire-brigade associations received a VAT refund?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| 63,209 IRC beneficiaries | 3/3 | ✓ | 3/3 |
| VAT refunds €123,994,570.29 | 3/3 | ✓ | 3/3 |
| Estado Maior Força Aérea €37,534,892.43 | 1/3 | ✓ | 3/3 |
| armed and security forces 43.3 % | 3/3 | ✓ | 3/3 |
| 414 fire-brigade associations | 3/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/pt-tax-benefits.hedgehog.md), [claude](answers/pt-tax-benefits.claude.md), [swap-hedgehog](answers/pt-tax-benefits.swap-hedgehog.md)

## owid-energy: Our World in Data energy dataset (23k country-years × 130 columns; countries mixed with regional aggregates)

File: `owid/owid_energy_data.csv` (under `clean/`)

> This is Our World in Data's energy dataset (countries and regional aggregates, by year).
> What share of Germany's electricity came from renewables in 2015 and in 2024?
> Which country generated the most solar electricity in 2024, how many TWh, and what share of world solar generation is that?
> By what percentage did world wind generation grow from 2015 to 2024?

| Check | hedgehog | claude | swap-hedgehog |
|---|---|---|---|
| Germany 2015: 29.4 % | 3/3 | ✓ | 3/3 |
| Germany 2024: 58.6 % | 3/3 | ✓ | 3/3 |
| China 839.04 TWh | 3/3 | ✓ | 3/3 |
| 39.2 % of world solar (2,142.65 TWh) | 0/3 | ✓ | 3/3 |
| world wind +202 % (829.6 → 2,509.5 TWh) | 3/3 | ✓ | 3/3 |

Answers (latest run): [hedgehog](answers/owid-energy.hedgehog.md), [claude](answers/owid-energy.claude.md), [swap-hedgehog](answers/owid-energy.swap-hedgehog.md)
