# de-berlin-grants · claude-hh · 2026-10-22T18:26:46+00:00 · claude-opus-5-5[1m]

Run `20261022T182646Z-de-berlin-grants-claude-hh`

I've finished all the queries.

| Question | Answer |
|---|---|
| **2024 total** | **€2,644,310,862** (about €2.64 bn) across **9,810 grants** |
| **Institutional share of 2024** | **22.9%** (about €606 m). The other 77.1% was project funding. |
| **Top recipient 2024** | **Forschungsverbund Berlin e. V.** (ID `vr_012174`): **€113,083,872** from 8 grants |
| **Year with the highest total** | **2025**, with **€3,695,037,363** (about €3.70 bn) |

**Totals by year:**

| Year | Grants | Total |
|---|---|---|
| 2021 | 14,011 | €2.703 bn |
| 2022 | 10,613 | €2.260 bn |
| 2023 | 11,752 | €3.123 bn |
| 2024 | 9,810 | €2.644 bn |
| 2025 | 9,268 | €3.695 bn |

**How I counted:**
- **Matching spellings of a recipient:** I grouped by the database's recipient ID (`empfaengerid`), not by the name. In 2024, Forschungsverbund Berlin appears under 5 different names, such as "Forschungsverbund Berlin e. V. (FVB)", "… Max-Born-Institut" and "… Leibniz-Institut für Kristallzüchtung". Grouped by name alone, BVG would come out on top.
- **Runners-up:** BVG (Berliner Verkehrsbetriebe) is close behind at €105,105,692 from 138 grants, then Grün Berlin GmbH at €81,491,556 (3 spellings). I kept the BVG-AusBildungs-Campus gGmbH (€49.9 m) and the Grün Berlin Stiftung separate, because each has its own recipient ID and is its own legal entity.
- **Institutional funding** appears in the file as both "institutionelle Förderung" and "Institutionelle Förderung". I counted the two together.
- **Possible duplicates:** 60 rows across all five years match another row on every field (recipient, funder, type, year, purpose, amount), but each has its own grant ID. I counted them as separate grants. Even if they were true duplicates, they would barely change any of the figures above.
