# pt-contracts · claude · 2026-10-21T07:03:16+00:00 · claude-opus-5-5[1m]

Run `20261021T070316Z-pt-contracts-claude`

I've answered all four questions below. Before the numbers, three things about the data that affect them:

- **Duplicates:** 20 contracts appeared twice as identical rows. I removed them, leaving 176,492 contracts worth **€11.16 bn** in total (`precoContratual`).
- **Name spellings:** the same body or company is often spelled several ways, e.g. "Infraestruturas de Portugal" vs "…, S. A.". I grouped buyers and suppliers by tax number (NIF). Foreign firms with no tax number are grouped by name.
- **Missing signing dates:** 26,723 contracts have no signing date. These are exactly the simplified direct awards (*Ajuste direto simplificado*), which don't need a written contract. They count in every answer except the monthly one.

## 1. Top 5 public bodies by contract value

| # | Public body | Contracts | Total value |
|---|---|---|---|
| 1 | Infraestruturas de Portugal, S.A. | 660 | €737.7 M |
| 2 | Metro do Porto, S.A. | 6 | €352.4 M |
| 3 | Centro Hospitalar Universitário de Lisboa Norte, EPE | 6,069 | €313.6 M |
| 4 | Metropolitano de Lisboa, EPE | 17 | €284.6 M |
| 5 | Direção-Geral da Saúde | 520 | €272.4 M |

- Metro do Porto and Metropolitano de Lisboa got there with a handful of very large contracts. Metro do Porto's two line-extension works alone were €189 M and €99 M.
- Direção-Geral da Saúde includes €54.5 M for Pfizer COVID-19 vaccines.

## 2. Suppliers with the most contracts

| # | Supplier | Contracts | Value |
|---|---|---|---|
| 1 | B. Braun Medical | 1,458 | €18.5 M |
| 2 | Generis Farmacêutica | 1,231 | €7.7 M |
| 3 | Medtronic Portugal | 1,152 | €38.1 M |
| 4 | MEO (telecoms) | 1,121 | €148.5 M |
| 5 | Roche Farmacêutica | 908 | €84.7 M |
| 6 | Labesfal | 895 | €9.0 M |
| 7 | Overpharma | 872 | €8.4 M |
| 8 | Novartis Farma | 816 | €54.8 M |
| 9 | Fresenius Kabi | 762 | €9.8 M |
| 10 | Merck Sharp & Dohme | 698 | €70.0 M |

Nine of the ten supply hospitals with medicines or medical equipment; MEO is the exception.

**Share of total value going to the top 10:**
- **Top 10 by number of contracts: 4.0%** (€449 M). They win many contracts, but mostly small ones.
- **Top 10 by value: 11.4%** (€1.27 bn). This group is Alberto Couto Alves, Endesa, Acciona, MEO, Uniself, Siemens/Stadler, Galp Power, Pfizer, Ferrovial and Roche.
- About 39,000 different suppliers won at least one contract.
- 1,031 contracts went to consortiums. For the value figures I split each one's value equally among its members; for the contract counts, every member is credited.

## 3. Direct awards vs public tenders

| Procedure | Contracts | Total value | Median contract |
|---|---|---|---|
| **Ajuste Direto (all types)** | **105,062** (59.5%) | **€2.52 bn** (22.6%) | — |
|   – Regime Geral (general) | 78,339 | €2.41 bn | €8,950 |
|   – Simplificado (simplified) | 26,262 | €109.8 M | €675 |
|   – Simplificado, Lei 30/2021 | 461 | €2.0 M | €1,850 |
| **Concurso público** | **17,407** (9.9%) | **€5.48 bn** (49.1%) | €64,546 |

Direct awards are 6 times as many contracts as public tenders, but public tenders carry more than twice the money. The rest of the total is mostly *Consulta Prévia* (prior consultation: 35,053 contracts, €1.14 bn) and purchases under framework agreements (18,667 contracts, €1.23 bn).

## 4. Contracts signed per month (by signing date)

| Jan | Feb | **Mar** | Apr | May | Jun | Jul | Aug | Sep | Oct | Nov | Dec |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 16,152 | 13,219 | **13,981** | 10,682 | 9,833 | 10,196 | 11,655 | 9,964 | 12,452 | 12,708 | 12,992 | 15,935 |

**There was no jump in the total number of contracts.** March was only 6% above February. April then fell 24% and May fell further, which is consistent with the lockdown slowing normal buying. Activity recovered from September and peaked in December, which is typical for year-end budget spending.

**COVID-related buying did jump.** I counted contracts that mention COVID in their subject or cite the emergency COVID decree (Decreto-Lei 10-A/2020):
- February: 20
- March: about 1,050
- April: about 1,230 (the peak)
- After that: about 400–800 a month

In total that is about 16,300 contracts worth €605 M. About 9,700 of them are simplified direct awards with no signing date, so they don't show up in the monthly table at all. Simplified direct awards also only begin to appear in March 2020, by publication date.

So COVID changed what was bought and how, with fast direct awards for emergency supplies, but it didn't raise the overall number of contracts.

The analysis scripts are in `/tmp/ptscripts/`; I didn't change the spreadsheet.
