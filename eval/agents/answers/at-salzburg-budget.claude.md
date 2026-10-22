# at-salzburg-budget · claude · 2026-10-21T23:19:13+00:00 · claude-opus-5-5[1m]

Run `20261021T231913Z-at-salzburg-budget-claude`

The budget plans **€2,722.6 M in expenses and €2,690.3 M in revenues, a deficit of about €32.3 M.** One catch first: the file's `Aufwand_Ertrag` column is wrong for some rows, and reading it as-is gives the opposite answer.

**The labelling problem.** I classified rows by the official account code (`MVAG_EH_3Steller`) instead: codes starting 21x are revenues and 22x are expenses. 86 rows have expense codes but are labelled "Ertrag" (revenue) in that column. They add up to €80.9 M, mostly depreciation (AfA, €79.2 M), plus write-offs of unpaid receivables and additions to provisions. These are real costs that don't involve cash leaving that year, so they belong on the expense side. If you sum by the column as it stands, you get expenses of €2,641.7 M, revenues of €2,771.3 M and a false **surplus of €129.6 M**.

**1. Totals (corrected)**

| | € |
|---|---|
| Revenues | 2,690,345,500 |
| Expenses | 2,722,624,100 |
| **Net result** | **−32,278,600 (deficit)** |

**2. Expenses by function area (Haushaltsgruppe)**

The largest is **Group 5, Gesundheit (health): €780.1 M, 28.7% of all expenses.**

| Group | € M | Share |
|---|---|---|
| 5 Gesundheit (health) | 780.1 | 28.7% |
| 2 Unterricht, Erziehung, Sport, Wissenschaft (education, sport, science) | 591.1 | 21.7% |
| 4 Soziale Wohlfahrt & Wohnbauförderung (social welfare and housing subsidies) | 580.0 | 21.3% |
| 0 Vertretungskörper & allg. Verwaltung (legislature and general administration) | 291.7 | 10.7% |
| 6 Straßen- & Wasserbau, Verkehr (roads, water works, transport) | 168.7 | 6.2% |
| 9 Finanzwirtschaft (public finance) | 165.4 | 6.1% |
| 7, 3, 1, 8 (all others combined) | 145.6 | 5.3% |

**3. Personnel expenses (Personalaufwand, code 221)**

**€894.7 M**, which is about 32.9% of total expenses. Of that, €730.4 M is pay and allowances and €160.3 M is social-security contributions. Only €31,400 of the personnel rows was mislabelled, so this figure is almost the same either way.

For reference, the file has 3,206 line items with no negative amounts. I converted it to CSV with LibreOffice to read it, saved at `/tmp/szg/`.
