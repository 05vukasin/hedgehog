# enron-deals-pl · claude-hh · 2026-10-22T18:18:48+00:00 · claude-opus-5-5[1m]

Run `20261022T181848Z-enron-deals-pl-claude-hh`

Total origination is **$135,666.48** across the 18 deals. That matches the sheet's own total in `Sheet1!L22` (=SUM(L3:L21)).

**Top trader: Mc Kay, with $78,653.38** (5 deals, 58% of the total). Two of those rows spell the name "McKay" and I counted them as the same trader.

| Trader | Deals | Origination |
|---|---|---|
| Mc Kay | 5 | $78,653.38 |
| Ermis | 8 | $37,145.50 |
| Reitmeyer | 1 | $18,250.00 |
| South | 1 | $1,617.60 |
| Chad | 2 | $0 |
| Quigly | 1 | $0 |

**Physical vs. financial:**
- **Financial:** $69,576.88 from 11 deals (51%)
- **Physical:** $66,089.60 from 7 deals (49%)

**Top delivery location: Sumas, with $77,603.38** (5 deals). Most of that comes from Mc Kay's OSM physical deal ($45,172) and two Mega financial deals ($27,395.55 and $5,035.83).

| Location | Deals | Origination |
|---|---|---|
| Sumas | 5 | $77,603.38 |
| CIG | 9 | $55,395.50 |
| NWR | 1 | $1,617.60 |
| AECO | 2 | $1,050.00 |
| NYMEX | 1 | $0 |

Things to be aware of:
- The title says "since April 1, 2001", but every deal in the log is dated between May 8 and May 29, 2001.
- Each deal's origination is calculated in the sheet as origination per MMBtu × days × volume per day. Two Mega deals have "Monthly" instead of a numeric trader price, and one Ermis deal has no prices at all. Their origination values still come from that per-MMBtu figure.
- Four deals show zero origination: the NYMEX deal and three Sumas deals (Chad's two and Mega's Sep–Oct deal).
