# de-berlin-grants · claude · 2026-10-21T23:28:42+00:00 · claude-opus-5-5[1m]

Run `20261021T232842Z-de-berlin-grants-claude`

**Answers for 2024:**

| Question | Answer |
|---|---|
| Total granted in 2024 | **€2,644,310,862** (about €2.64 bn) across **9,810 grants** |
| Institutional funding share | **€605,891,758, which is 22.9%** of the 2024 money. The other 77.1% is project funding (Projektförderung). |
| Top recipient | **Forschungsverbund Berlin e. V. (FVB): €113,083,872** from 8 grants |
| Year with the highest total | **2025: €3,695,037,363** (about €3.70 bn) |

**Totals by year:**

| Year | Total (€) | Grants |
|---|---|---|
| 2021 | 2,703,000,698 | 14,011 |
| 2022 | 2,259,883,184 | 10,613 |
| 2023 | 3,123,181,707 | 11,752 |
| 2024 | 2,644,310,862 | 9,810 |
| **2025** | **3,695,037,363** | 9,268 |

**How I got these numbers:**
- **Data quality:** All 55,454 rows have a whole-number amount, and none are negative or zero.
- **Funding type spellings:** The type column spells institutional funding two ways ("institutionelle" and "Institutionelle Förderung"). I counted both as institutional.
- **Grant with no type:** One 2024 grant has no funding type (Berlin 2070 gGmbH, €285,900). I counted it as project funding. Leaving it out changes the share by less than 0.01 percentage points.
- **Grouping recipients:** I grouped recipients by their recipient ID (`empfaengerid`), which ties all spellings of a name together. FVB appears under 5 spellings, which include its member institutes: Leibniz-Institut für Kristallzüchtung, Max-Born-Institut and Paul-Drude-Institut. I also checked the names directly and found no FVB grants under a different ID or without an ID.
- **The top spot is close:** If you counted only the grants listed as "Forschungsverbund Berlin e. V." itself, without the member institutes, FVB would have €105.6M. That is only just ahead of the Berliner Verkehrsbetriebe (BVG) at €105.1M from 138 grants. The next largest are Grün Berlin GmbH (€81.5M) and the DFG (€67.1M).
- **Duplicates:** 7 rows are exact duplicates except for their ID, all in 2021–2023. They don't affect the 2024 figures. I left them in the yearly totals because they could be genuine separate grants. Together they come to less than €1.6M.
