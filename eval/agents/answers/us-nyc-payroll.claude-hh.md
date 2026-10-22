# us-nyc-payroll · claude-hh · 2026-10-22T18:22:27+00:00 · claude-opus-5-5[1m]

Run `20261022T182227Z-us-nyc-payroll-claude-hh`

Two of the three questions can be answered from this file. The third can't, because the extract has no NYPD rows.

**1. Administration for Children's Services overtime**
- **FY2025:** ACS paid **$52,445,049.66** in overtime, across 8,246 payroll rows.
- **Highest year:** overtime peaked in **FY2019 at $61,342,038.11**. FY2018 was close behind at $60.58M.
- **Trend:** overtime fell to $24.08M in FY2021, rose back to about $53–54M in FY2023 and FY2024, and dipped slightly in FY2025.

**2. Child Protective Specialist average base salary** (all paid per year)

| Fiscal year | Employee rows | Average base salary |
|---|---|---|
| FY2015 | 2,628 | **$50,800.58** |
| FY2025 | 2,958 | **$66,449.96** |

That is a rise of about **30.8%**. These figures cover only the "Child Protective Specialist" title, not the separate "Child Protective Specialist Supervisor" title. Supervisors averaged $73,192 in FY2015 and $95,591 in FY2025.

**3. NYPD overtime in FY2025: not in this file.** The extract only contains two agencies: Admin for Children's Services (FY2014–2025) and Admin Trials and Hearings (FY2019–2025). You'd need the full citywide payroll dataset or a different extract to get the NYPD figure.

Overtime is the sum of `total_ot_paid` per agency and year. The file has 229 negative overtime entries, probably payroll adjustments, and I left them in the sums.
