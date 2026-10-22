# us-superstore · claude · 2026-10-21T23:20:20+00:00 · claude-opus-5-5[1m]

Run `20261021T232020Z-us-superstore-claude`

Here are the answers. They come from the **Orders** sheet (10,194 order lines) and the **Returns** sheet.

| Question | Answer |
|---|---|
| **Total sales** | **$2,326,534.35** |
| **Top region** | **West, $739,813.61** (31.8% of sales). Next were East at $691,828, Central at $503,171 and South at $391,722. |
| **Growth from 2023 to 2024** | **+21.4%**, from $613,933.58 to $745,567.53 |
| **Biggest loss-making sub-category** | **Tables, −$17,753.21** profit. Only two other sub-categories lost money: Bookcases (−$3,632) and Supplies (−$1,171). |
| **Share of orders returned** | **5.8%**: 296 of 5,111 distinct orders |
| **Sales on returned orders** | **$180,504.28**, or 7.8% of total sales |

How I counted:
- **Returns:** The Returns sheet has one row per order line (800 rows), which comes to 296 unique order IDs. I counted the return share by distinct orders, not by lines. The $180,504 is the total of every line on those 296 orders. All 800 return rows match an order line, so the two sheets agree.
- **Data check:** There are no missing values. Two pairs of lines are identical apart from their Row ID (rows 391/392 and 1699/1700). That looks like an order with the same item entered twice, not a broken record, so I kept them. Removing them would change total sales by only about $380.
- **Years:** I grouped sales by Order Date.
