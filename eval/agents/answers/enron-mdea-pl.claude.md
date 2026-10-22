# enron-mdea-pl · claude · 2026-10-21T23:18:05+00:00 · claude-opus-5-5[1m]

Run `20261021T231805Z-enron-mdea-pl-claude`

Over May 1–14, 2001, day-ahead transactions only, EPMI's total net profit was **$44,058.18** and MDEA's total net savings were **$127,882.48**.

MDEA's largest day-ahead savings came on **May 5, 2001: $15,056.52**. May 6 was next at $13,499.09.

| Day | EPMI net profit | MDEA net savings |
|---|---:|---:|
| May 1 | 191.92 | 3,730.38 |
| May 2 | 2,444.08 | 8,201.62 |
| May 3 | 1,907.63 | 7,593.05 |
| May 4 | 1,489.57 | 6,631.15 |
| **May 5** | 4,853.68 | **15,056.52** |
| May 6 | 3,778.06 | 13,499.09 |
| May 7 | 3,301.54 | 12,966.32 |
| May 8 | 4,398.00 | 11,397.00 |
| May 9 | 4,290.12 | 11,645.58 |
| May 10 | 4,156.20 | 11,403.30 |
| May 11 | 3,880.92 | 10,767.78 |
| May 12 | 3,292.14 | 5,442.81 |
| May 13 | 2,160.16 | 3,676.64 |
| May 14 | 3,914.16 | 5,871.24 |
| **Total** | **44,058.18** | **127,882.48** |

How I got these figures:
- **Which rows count:** I used only the day-ahead section, the 24 hourly rows at the top of each sheet (rows 7–30). From May 3 onward each sheet also has a "Real-Time Transactions" section further down, which I left out.
- **Where the numbers come from:** The figures are each sheet's day-ahead totals in row 32: column S is EPMI's profit and column T is MDEA's savings. On every sheet these totals match the sum of the 24 hourly rows.
- **Typed-in totals:** On May 1–2 and May 10–14 the row-32 totals are typed-in numbers rather than formulas. They still agree with the hourly rows, so nothing changes.
