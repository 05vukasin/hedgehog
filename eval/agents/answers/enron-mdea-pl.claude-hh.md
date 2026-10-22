# enron-mdea-pl · claude-hh · 2026-10-22T17:18:06+00:00 · claude-opus-5-5[1m]

Run `20261022T171806Z-enron-mdea-pl-claude-hh`

Across the 14 day-ahead sections (May 1–14, 2001), EPMI's total net profit was $44,058.18 and MDEA's total net savings were $127,882.48. MDEA's day-ahead savings were largest on **May 5, 2001**, at $15,056.52.

| Day | EPMI net profit | MDEA net savings |
|---|---:|---:|
| May 1 | 191.92 | 3,730.38 |
| May 2 | 2,444.08 | 8,201.62 |
| May 3 | 1,907.63 | 7,593.05 |
| May 4 | 1,489.57 | 6,631.15 |
| **May 5** | **4,853.68** | **15,056.52** |
| May 6 | 3,778.06 | 13,499.09 |
| May 7 | 3,301.54 | 12,966.32 |
| May 8 | 4,398.00 | 11,397.00 |
| May 9 | 4,290.12 | 11,645.58 |
| May 10 | 4,156.20 | 11,403.30 |
| May 11 | 3,880.92 | 10,767.78 |
| May 12 | 3,292.14 | 5,442.81 |
| May 13 | 2,160.16 | 3,676.64 |
| May 14 | 3,914.16 | 5,871.24 |
| **Total** | **$44,058.18** | **$127,882.48** |

The source cell for the May 5 figure is `May5!T32` (`=SUM(T7:T30)`), and EPMI's profit that day is in `May5!S32`. May 6 was second, at $13,499.09.

- **Real-time excluded:** most sheets also have a separate real-time section, which I left out.
- **Totals checked:** each day's hourly rows add up to the sheet's own day-ahead total row.
- **Partial days:** May 1, 12, 13 and 14 have savings in only 12–16 of the 24 hours. The remaining hours are blank, so those days' totals are lower.
