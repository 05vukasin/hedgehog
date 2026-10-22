# enron-storage-inventory · claude · 2026-10-21T23:18:28+00:00 · claude-opus-5-5[1m]

Run `20261021T231828Z-enron-storage-inventory-claude`

I read this from Sheet1, the only sheet with data. Volumes have no units in the file; for NYMEX-priced gas they are most likely MMBtu/Dth.

**1. Highest month-end total: October 31, 2001, at 12,427,743**

| Month-end | Pipeline | LDC / Virtual | Total |
|---|---|---|---|
| Jul 31 | 6,821,693 | 1,648,112 | 8,469,805 |
| Aug 31 | 8,159,748 | 1,957,265 | 10,117,013 |
| Sep 30 | 9,527,204 | 2,248,271 | 11,775,475 |
| **Oct 31** | **10,087,219** | **2,340,524** | **12,427,743** |
| Nov 30 | 8,933,036 | 2,087,589 | 11,020,625 |
| Dec 31 | 6,176,332 | 1,509,813 | 7,686,145 |
| Jan 31 | 3,047,689 | 916,099 | 3,963,788 |
| Feb 28 | 1,096,968 | 384,166 | 1,481,134 |
| Mar 31 | 0 | 100,595 | 100,595 |

Inventory builds through October and is drawn down to about zero by March. Only MichCon still holds gas at March 31 (100,595).

**2. End of October 2001**
- **Columbia Gas (COH) pipeline storage: 8,285,615 in total.** That is COH FSS at 7,338,781 plus COH (AES) FSS at 946,834. This is about 82% of all pipeline storage.
- **LDC / virtual storage: 2,340,524** (the section subtotal). The largest holdings are AGL Peaking (518,000), MichCon (393,164) and Nipsco (390,000).
- Pipeline storage in total is 10,087,219. The Columbia total plus 1,801,604 held on Sonat and Transco for AGL makes up that amount.

**3. Value of the end-of-October inventory (12,427,743)**
- **At the current NYMEX price ($3.42): $42,502,881.06**
- **At the winter NYMEX price (~$3.745, i.e. $3.744970…): $46,541,527.99**
- The winter price adds about **$4.04 million** of value.

I recomputed both values from volume × price and they match the sheet.

**Problems in the file:**
- The headers for January, February and March are dated 2001. In this July-to-March sequence they must be 2002.
- No prices or values are given for July or March. The pricing rows only cover August to February.
- Cell O28 holds an unlabeled 24,151,692, and I couldn't tell what it represents. It is not the October total, so it does not change any of the answers above.
