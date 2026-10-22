# enron-storage-inventory · hedgehog · 2026-10-22T06:20:39+00:00 · gpt-4.1-mini

Run `20261022T062039Z-enron-storage-inventory-hedgehog`

1. The total storage inventory is highest at the month-end of October 31, 2001, with a total of 10,087,219 units in pipeline storage (subtotal from T1_all, "Pipeline Storage / Type" = 'Sub-Total', column "Inventory as of: / 2001-10-31").

2. At the end of October 2001, Columbia Gas (COH) pipeline storage holds 7,338,781 units (from T1, filtered by "Pipeline Storage / Pipeline" = 'Columbia Gas' and "Pipeline Storage / LDC" = 'COH', column "Inventory as of: / 2001-10-31").
   The LDC storage in Columbia Gas at the same date sums to 158,946 units (46,546 + 101,990 + 15,410) from T2, filtered by "LDC / Virtual Storage / Description" = 'Col. Gas'.

3. The end-of-October inventory worth at the current NYMEX price is 46,541,528 (from T2_all, "LDC / Virtual Storage / Type" = 'Value', _row_type = 'total', column "Inventory as of: / 2001-10-31").
   The file does not contain a separate value for the winter NYMEX price at the end of October 2001.
