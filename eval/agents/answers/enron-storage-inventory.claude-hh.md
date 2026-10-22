# enron-storage-inventory · claude-hh · 2026-10-22T18:19:14+00:00 · claude-opus-5-5[1m]

Run `20261022T181914Z-enron-storage-inventory-claude-hh`

Total storage inventory peaks at the **October 31, 2001** month-end, and that inventory is worth about **$42.5M at the current NYMEX price** and **$46.5M at the winter NYMEX price**.

**1. Peak month-end**

- Total storage reaches **12,427,743** at 10/31/2001 (`Sheet1!G28`, formula `=G26+G10`).
- It builds up from 8,469,805 at 7/31 and falls to 100,595 by 3/31/2002.
- The sheet gives no unit; these are presumably gas volumes in MMBtu/Dth.

**2. End of October 2001 split**

- **Columbia Gas (COH) pipeline storage: 8,285,615 in total.** This is COH FSS 7,338,781 (`G5`) plus COH (AES) FSS 946,834 (`G6`).
- **All pipeline storage: 10,087,219** (`G10`). The rest is Sonat/AGL 1,293,235, Transco WSS 483,019 and Transco ESS 25,350.
- **LDC / virtual storage: 2,340,524** (`G26`).
- Check: 10,087,219 + 2,340,524 = 12,427,743.

**3. Value of the end-of-October inventory**

| Price basis | $/unit | Value |
|---|---|---|
| Current NYMEX | 3.42 (`G30`) | **$42,502,881** (`G32`) |
| Winter NYMEX | 3.74497 (`G34`) | **$46,541,528** (`G36`) |

The winter NYMEX price isn't a quoted market price. The sheet calculates it as a volume-weighted average of the November–February prices (3.70, 3.82, 3.76 and 3.65). On that basis, the winter value is about $4.0M higher than the current-price value.
