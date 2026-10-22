# us-chicago-salaries · claude · 2026-10-21T23:21:30+00:00 · claude-opus-5-5[1m]

Run `20261021T232130Z-us-chicago-salaries-claude`

I read all 32,022 employee rows in `employee_salaries.csv` and calculated the answers in Python.

**1. Department with the most employees:** The Chicago Police Department, with **12,221** employees (38% of the list). Next are Fire (4,774), Aviation (2,104), Streets & Sanitation (2,078) and Water Management (2,038).

**2. Hourly vs. salaried:** **24,893 salaried** and **7,129 hourly**. Of the hourly workers, about 6,150 work full time (mostly 40 hours a week) and about 980 work part time (mostly 20 hours a week).

**3. Average Police Officer salary:** **$110,467** across the 7,903 people whose title is exactly "POLICE OFFICER". The median is $111,252 and the range is $63,636 to $127,146. There are 14 other titles that start with "POLICE OFFICER", such as "(ASSIGNED AS DETECTIVE)" or "/ FLD TRNG OFFICER". If you count all of them (9,715 people), the average rises to **$114,774**.

**4. Highest-paid employee:** **Michael J. Mc Murray**, Commissioner of Aviation, at **$360,504** a year. That is about $100K more than the next highest, Fire Commissioner Annette M. Nance Holt at $257,268. Even annualized, no hourly worker comes close: the top hourly rate is $80/hr.

**5. Total annual salary bill:** **$2,893,689,009 (about $2.89 billion)**, adding up the `annual_salary` column. That figure leaves out:
- **All 7,129 hourly workers.** Their `annual_salary` field is blank; they only have an hourly rate and typical weekly hours. Rate × hours × 52 weeks comes to roughly **$0.69 billion** more, which brings the full-year payroll to about **$3.59 billion**. That estimate assumes they work their typical hours all 52 weeks.
- **Overtime, bonuses, benefits, pensions and other employer costs.** The file only has base pay rates, not what was actually paid out. That matters most for Police and Fire, where overtime is large.
- **Turnover during the year.** The file is a snapshot of current employees, so people who left earlier in the year and vacant positions aren't counted.

**Data quirks to be aware of:**
- **11 pairs of identical rows**, 8 of them in Police; examples are "GARCIA, DAVID" and "RODRIGUEZ, JOSE J". There's no employee ID column, so I can't tell whether these are duplicates or different people with common names. I counted every row. They would change the totals by about $1M at most.
- **One employee (Albert Barrera, Aviation) has a blank full/part-time field.**
