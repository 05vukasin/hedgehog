# enron-newco-cashflow · claude-hh · 2026-10-22T18:18:28+00:00 · claude-opus-5-5[1m]

Run `20261022T181828Z-enron-newco-cashflow-claude-hh`

The Summary sheet is labelled "(All values in 000s)". The unit profile is also in thousands: 500 MW × $550/kW = $275M of capital, which the sheet stores as 275,000. All figures below are those values multiplied by 1,000.

**Total net income, all projects (2001–2005 projects)**
- **2005: $259,957,886** (Summary!H26 = 259,957.8862)
- **2020: $1,024,037,237** (Summary!W26 = 1,024,037.23656)

**Total aftertax cash, 2010**
- **$362,129,578** (Summary!M34 = 362,129.577865)

**Generic 500 MW combined-cycle unit** ('New Unit Financial Profile' sheet)
- **NPV at 14%: about $4,254,691** (cell C30 = 4,254.6913). The formula is `=NPV(0.14, C10:V10) - C13`: 20 years of aftertax cash flow, minus the $110M equity investment (40% of $275M).
- **IRR: about 14.49%** (cell C31 = 0.14489). It's calculated on the equity cash flows: −$110M up front, then the yearly aftertax cash flows.

The IRR is just above the 14% discount rate, which is why the NPV is small but positive.
