# at-salzburg-budget · claude-hh · 2026-10-22T18:20:11+00:00 · claude-opus-5-5[1m]

Run `20261022T182011Z-at-salzburg-budget-claude-hh`

The budget plans a **deficit of about €32.3 million**. You only get that if you correct one flag in the file. If you sum the file's own `Aufwand_Ertrag` (expense/revenue) column as it is, you get a €129.6 million surplus instead, and that figure is wrong.

**Why the flag is wrong.** 89 rows worth €80.9 million are marked "Ertrag" (revenue), but their account category is an expense type (Sachaufwand, Personalaufwand, Finanzaufwand). They are clearly expenses:
- 62 rows of AfA (depreciation), €79.2 million in total. The first one is at Ergebnishaushalt!U8.
- Write-offs of receivables (€0.9 million and €0.76 million) and additions to provisions.

So I sorted every row by its account category (`MVAG_EH_3Steller`) instead of the flag. The file has no total row of its own, so I couldn't check these sums against an official figure.

### 1. Totals (planned 2019, column LVA_2019)
| | € |
|---|---|
| **Total expenses** | **2,722,624,100** (≈ €2.72 bn) |
| **Total revenues** | **2,690,345,500** (≈ €2.69 bn) |
| **Result** | **Deficit of 32,278,600** (≈ €32.3 m) |

The revenues are made up of:
- operating revenues: €1,917.9 m
- transfer revenues: €723.7 m
- financial revenues: €48.8 m

If you use the raw flag instead, expenses come to €2,641.7 m and revenues to €2,771.3 m.

### 2. Largest function area
**Gesundheit (Haushaltsgruppe 5, Health)** has the most expenses: **€780.1 million, 28.7% of all expenses.** It is followed by:
- Unterricht, Erziehung, Sport und Wissenschaft (education, sport and science): €591.1 m (21.7%)
- Soziale Wohlfahrt und Wohnbauförderung (social welfare and housing): €580.0 m (21.3%)

With the raw flag, Health is still first, at €775.4 m and 29.4%.

### 3. Personnel expenses
**€894.7 million** (894,703,300), about 32.9% of total expenses. Of this:
- salaries and allowances: €730.4 m
- statutory and voluntary social costs: €160.3 m
- other personnel costs: €4.0 m

Four tiny personnel rows (€31,400) carry the "Ertrag" flag. Leaving them out gives €894,671,900.
