# uk-ons-labour · claude · 2026-10-21T23:25:22+00:00 · claude-opus-5-5[1m]

Run `20261021T232522Z-uk-ons-labour-claude`

The latest period in Table 1 (sheet `1`) is **May–Jul 2026** (row 673).

**Headline rates**
- **Employment rate: 75.1%** (75.077%). This is the 16–64 rate, code LF24, which the sheet's own notes define as the headline employment rate.
- **Unemployment rate: 4.9%** (4.905%). This is the 16+ rate, code MGSX, which the notes define as the headline unemployment rate.

**Levels (aged 16 and over, the headline basis)**
- **In employment: about 34.48 million** (34,477,788, code MGRZ)
- **Unemployed: about 1.78 million** (1,778,259, code MGSC)

**Change from the previous non-overlapping period**
The sheet's footnote says to compare against the previous non-overlapping three months, which here is **Feb–Apr 2026**. The unemployment rate there was 4.886%, so it **rose slightly**, from 4.886% to 4.905% (about +0.02 percentage points). At one decimal place both periods show 4.9%, so a published headline would call it essentially unchanged.

**Two things in the file to know about:**
- The sheet header says "United Kingdom (thousands)", but the level cells hold whole-person counts with decimals (for example, 34477788.08). If the values really were in thousands, employment would be about 34 billion. I read them as persons. If your parser applies the "thousands" label, the levels will be 1,000 times too large.
- The rates and levels are unrounded model values, not the rounded figures ONS publishes. That is why the change in the rate only shows up beyond the first decimal place.
