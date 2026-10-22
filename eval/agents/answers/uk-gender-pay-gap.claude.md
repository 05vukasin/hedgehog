# uk-gender-pay-gap · claude · 2026-10-21T23:22:35+00:00 · claude-opus-5-5[1m]

Run `20261021T232235Z-uk-gender-pay-gap-claude`

**1. Employers reporting: 11,314.** Each row is a distinct employer (11,314 unique EmployerIds) and every row has a median hourly gap value.

**2. Median of the median hourly gap: 8.4%.** Values run from −122% to +100%.

**3. Gap in favour of men (median gap > 0): 77.5%** (8,770 of 11,314). Another 13.9% (1,571) favour women and 8.6% (973) report exactly 0.

**4. Average median hourly gap by employer size:**

| Size band | Employers | Mean of median gap | Median of median gap |
|---|---|---|---|
| **500 to 999** | 2,901 | **12.06%** | 9.2% |
| 250 to 499 | 4,897 | 11.67% | 9.09% |
| 1000 to 4999 | 2,431 | 10.45% | 7.4% |
| 5000 to 19,999 | 543 | 9.05% | 7.2% |
| Less than 250 | 473 | 8.21% | 4.62% |
| 20,000 or more | 69 | 7.23% | 5.5% |

The **500 to 999** band has the highest average, just ahead of 250 to 499. The gap tends to shrink as employers get bigger.

**5. Late submissions: 764**, going by the dataset's `SubmittedAfterTheDeadline` flag (6.8%).

That flag doesn't always match the dates. If you count rows where `DateSubmitted` falls after `DueDate`, you get 791, not 764:
- 71 rows were submitted after their due date but aren't flagged as late.
- 44 rows are flagged as late but were submitted on or before their due date.

These could be resubmissions or late reports with an accepted reason, but the file doesn't say. I used the official flag. There are two due dates: 31 March 2025 for public sector (1,945 rows) and 5 April 2025 for private sector (9,369 rows).
