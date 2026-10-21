# Locales: numbers, dates and currencies

How hedgehog reads numbers, dates, currencies, missing values and total labels written in US,
European and other conventions, how it settles ambiguous values, and when to pass a locale.

hedgehog is locale-neutral: no convention is the default. `1,200.50` and `1.200,50` are both
read as 1200.5, `12/31/2024` and `31.12.2024` both as 31 December 2024, and a file may mix
conventions from column to column. Only values that can honestly be read two ways (`1.200`,
`03/04/2024`) need a decision, and that decision is made per column from the other values in it.

Contents: [Numbers](#numbers) · [Dates](#dates) · [Currencies](#currencies) ·
[Missing values and booleans](#missing-values-and-booleans) · [Text](#text) ·
[Total and note keywords](#total-and-note-keywords) · [Ambiguous values](#ambiguous-values) ·
[Locale presets](#locale-presets)

## Numbers

Numbers stored as text are parsed when they match one of these forms; anything else stays text, so
phone numbers, codes and dates are not misread.

| Written | Read as | Notes |
|---|---|---|
| `1,200.50`, `1.200,50` | 1200.5 | both decimal conventions |
| `1 200,50`, `1 200,50` (NBSP), `1'200.50` | 1200.5 | space, no-break space and apostrophe thousands (French, Polish, Swiss) |
| `-500`, `(500)`, `500-` | -500 | accounting negatives |
| `12%`, `8,5 %` | 0.12, 0.085 | stored as fractions |
| `$1,200`, `1.200,00 €`, `EUR 1,200.00`, `1 200 Kč` | 1200 | currency removed and recorded |
| `5 kg`, `12 pcs` | 5, 12 | unit removed and recorded |
| `1,5 mil.`, `5k` | 1500000, 5000 | scale words |
| `1.234*`, `567 (a)` | 1234, 567 | trailing footnote markers |

## Dates

| Written | Examples |
|---|---|
| Numeric | `31.12.2024`, `31.12.2024.`, `31. 12. 2024.`, `2024-12-31` (ISO, optional time `T10:30:00Z`), `2024/12/31`, `12/31/2024`, `31/12/2024`, `31-12-24`, month only `12/2024`, `2024-12` |
| Month names | `31 December 2024`, `December 31st, 2024`, `31-Dec-24`, `1er janvier 2024`, `31 de diciembre de 2024`, `31. März 2024`, `okt 2026` |
| Quarters | `Q3 2024`, `2024-Q3`, `3Q24`, `3rd quarter 2024`, `III kvartal 2024` |
| Weeks | `2024-W05`, `KW 5/2024` |
| Times | trailing `10:30`, `10:30:15`, `2:05 PM`; a leading weekday is ignored |
| Excel serial numbers | a plain number in a column that is otherwise dates |

Month-only, quarter and week values become the first day of the period (`Q3 2024` is 2024-07-01,
`2024-W05` is 2024-01-29).

Month and weekday names are recognized in English, German, French, Spanish, Italian, Dutch,
Portuguese, Polish and Serbian/Croatian/Bosnian/Slovenian (Latin and Cyrillic): full, abbreviated and
genitive forms, with or without diacritics (`März`, `Marz`, `Mär`).

Day/month order for `a/b/yyyy`: a field greater than 12 decides; otherwise the column's order
(below); otherwise a tie-break that is flagged as ambiguous: `/` means month/day, `-` means day/month,
and `.` is always day/month. Two-digit years follow Excel: `00`–`29` are 2000–2029, `30`–`99` are
1930–1999. A bare year (`2024`) is a number, not a date.

## Currencies

All active ISO 4217 codes, plus symbols and words before or after the number: `$`, `€`, `£`, `¥`,
`₹`, `₽`, `₺`, `₩`, `₪`, `zł`, `Kč`, `kr`, `Fr.`, `R$`, `C$`, `A$`, `US$`, `HK$`, `din.`, and more.

- A bare `$` is USD; prefixed dollars (`C$`, `A$`, `HK$`) are distinct.
- Symbols shared by several currencies resolve to the most common one: `kr` to SEK, `¥` to JPY,
  `Rs` to INR.
- Codes that are also English words (`ALL`, `TOP`, `CUP`, `PEN`, `TRY`, ...) count as currencies only
  in a column that has other currency evidence, such as a header `Amount (USD)`.

In the normalized output a column whose values mostly carry a currency gets dtype `currency`, the
values become plain numbers, and the manifest records the code. Mixed currencies are listed in the
table's `warnings`.

## Missing values and booleans

These become empty cells in the normalized output:

- placeholders: `-`, `–`, `—`, `/`, `..`, `...`, `?`, `*`, `_`, `x`, `xx`, `n.p.`;
- words: `n/a`, `NA`, `n.a.`, `n/d`, `null`, `nil`, `none`, `nan`, `missing`, `unknown`,
  `not available`, `not applicable`, `no data`, `tbd`, and Serbian/Croatian equivalents (`nema`,
  `nepoznato`, ...);
- Excel errors: `#N/A`, `#REF!`, `#DIV/0!`, `#VALUE!`, `#NAME?`, `#NUM!`, `#NULL!`, `#SPILL!`,
  `#CALC!`.

Booleans: `yes`/`no`, `y`/`n`, `true`/`false`, `✓`/`✗`, `ja`/`nein`, `oui`/`non`, `si`, `da`/`ne`
(Latin and Cyrillic). `1`/`0` and `x` are not taken as booleans.

## Text

Text is trimmed, invisible characters (no-break spaces, zero-width characters) are removed, and runs of
whitespace become one space. Searches and keyword matching use a normalized key that ignores case,
diacritics and script: `Beograd`, ` BEOGRAD ` and `Београд` all match; `Đorđević` matches
`djordjevic`. Serbian Cyrillic is transliterated to Latin deterministically.

## Total and note keywords

- **Layout** (which rows are totals, notes and sources) recognizes total and note words in English,
  German, French, Spanish, Italian, Dutch, Portuguese, Polish, Nordic languages and
  Serbian/Croatian/Bosnian/Slovenian: `Total`, `Subtotal`, `Grand total`, `Summe`, `Gesamt`,
  `Totaal`, `Razem`, `Ukupno`, `Σ`; `Note`, `Source`, `Quelle`, `Fuente`, `Bron`, `Izvor`, `*`, `¹`.
- **Totals verification** recognizes a typed total by its label: English and
  Serbian/Croatian/Bosnian (`Total`, `Subtotal`, `Grand total`, `Sum`, `Ukupno`, `Svega`,
  `Međuzbir`, `Укупно`, ...); the first word of a German, French, Spanish, Italian, Dutch,
  Portuguese, Polish or Nordic label (`Summe`, `Insgesamt`, `Somme`, `Totale`, `Totaal`, `Razem`);
  and, as the first or last word of a label of up to five words, Czech/Slovak `Celkem`/`Celkom`,
  Finnish `Yhteensä`, Danish `I alt`, Swedish/Norwegian `Totalt`, Hungarian `Összesen`, Russian
  `Всего`/`Итого`, Polish `Ogółem`/`Łącznie`, Estonian `Kokku`, Icelandic `Samtals`, Turkish
  `Toplam`, Greek `Σύνολο`, German `gesamt`/`insgesamt` after a name (`Deutschland gesamt`). Short
  labels with a one-letter typo of `total` (`Toal`, `Totl`, `Grand Totl`) and spaced letters
  (`T o t a l`) count too. Words that are also names or codes (`Somme`, `Sumas`, `SOM`, `Suma …`,
  `Zusammen …`) need the row's numbers to add up. A total written as a formula (`SUM`, `SUBTOTAL`,
  `A+B+C`) is recognized whatever its label.
- **Average and of-which rows** (not data, not sums): `Average`, `Mean`, `Median`, `Ø`,
  `Durchschnitt`, `Mittelwert`, `Moyenne`, `Media`, `Promedio`, `Prosek`, `Średnia`, `Gemiddelde`,
  `Gennemsnit`, `Keskiarvo`, `Átlag`, `Průměr`; `of which`, `thereof`, `davon`, `darunter`, `dont`,
  `di cui`, `de los cuales`, `dos quais`, `od toga`, `w tym`, `z toho`, `waarvan`, `varav`, `heraf`,
  `josta`, `ebből`, `из них` ([concepts](concepts.md#average-and-of-which-rows)).

## Ambiguous values

`1.200` is 1.2 with a decimal point or 1200 with a thousands separator. `03/04/2024` is 4 March or
3 April. hedgehog decides, in this order:

1. **An explicit locale**, if you pass one (`--locale`, `locale=`). It applies to every column.
2. **The column's own evidence.** A value counts as evidence only when it cannot be read the other
   way: `1.200,50`, `3,25` and `1.200.000` mean decimal comma; `1,200.50`, `3.25` and `1,200,000`
   mean decimal point; `25/12/2024` means day first. The majority wins.
3. **The sheet's evidence**, when the column has none; then cross-evidence (a sheet with decimal
   commas writes dates day first; a month-first sheet uses the decimal point).
4. **The currency in the cell**, for numbers: `1.200 RSD` is 1200 because RSD amounts use a decimal
   comma, `$1,200` is 1200. `€` is used with both conventions and gives no evidence.
5. **A fixed tie-break, flagged as a guess:** `.` is the decimal point and `,` a thousands separator
   (`1.200` is 1.2, `1,200` is 1200); dates as described above.

Values read by a guess are counted in the column's `locale.ambiguous` in the manifest, and the table
gets a warning. Here `orders.xlsx` has only ambiguous values:

```python
import openpyxl

wb = openpyxl.Workbook()
ws = wb.active
ws.title = "Orders"
ws.append(["Order", "Shipped", "Weight"])
ws.append(["A-1", "03/04/2024", "1.200"])
ws.append(["A-2", "05/06/2024", "2.500"])
ws.append(["A-3", "07/08/2024", "3.750"])
wb.save("orders.xlsx")
```

```python
import hedgehog

r = hedgehog.normalize("orders.xlsx", "out/")
t = r.table("T1")
for c in t["columns"][1:3]:
    print(c["name"], c["dtype"], c["min"], c["max"], c["locale"])
print(t["warnings"][0])
```

```text
Shipped date 2024-03-04 2024-07-08 {'decimal': None, 'date_order': None, 'ambiguous': 3}
Weight number 1.2 3.75 {'decimal': None, 'date_order': None, 'ambiguous': 3}
column 'Shipped': 3 value(s) read with a guessed decimal separator or day/month order (pass a locale to override)
```

If the file comes from Germany, say so:

```python
import hedgehog

r = hedgehog.normalize("orders.xlsx", "out_de/", locale="de-DE")
for c in r.table("T1")["columns"][1:3]:
    print(c["name"], c["dtype"], c["min"], c["max"], c["locale"])
print(r.table("T1")["warnings"])
```

```text
Shipped date 2024-04-03 2024-08-07 {'decimal': ',', 'date_order': 'dmy', 'ambiguous': 0}
Weight integer 1200 3750 {'decimal': ',', 'date_order': 'dmy', 'ambiguous': 0}
[]
```

When the column has unambiguous values, no locale is needed. `budget.xlsx` mixes German number
formatting, day-first dates and euro amounts:

```python
import openpyxl

wb = openpyxl.Workbook()
ws = wb.active
ws.title = "Budget"
ws.append(["Cost centre", "Date", "Amount"])
ws.append(["Marketing", "03.04.2024", "1.200,50 €"])
ws.append(["Sales", "15.04.2024", "980,00 €"])
ws.append(["IT", "02.05.2024", "2.045,25 €"])
wb.save("budget.xlsx")
```

```python
import hedgehog

r = hedgehog.normalize("budget.xlsx", "out/")
print(r.read("T1"))
amount = r.table("T1")["columns"][2]
print(amount["dtype"], amount["currency"], amount["locale"])
```

```text
  Cost centre       Date   Amount  _src_row
0   Marketing 2024-04-03  1200.50         2
1       Sales 2024-04-15   980.00         3
2          IT 2024-05-02  2045.25         4
currency EUR {'decimal': ',', 'date_order': None, 'ambiguous': 0}
```

## Locale presets

A locale fixes two things for every column: the decimal separator and the day/month order. It does
not change which month names, currencies or keywords are understood; those are always all on.

| Preset | Tags that select it | Decimal | Dates | Example |
|---|---|---|---|---|
| `us` | `us`, `en`, `en-US`, `en-PH` | `.` | month first | `1,234.56`, `12/31/2024` |
| `uk` | `uk`, `gb`, `en-GB`, `en-IE`, other `en-XX` | `.` | day first | `1,234.56`, `31/12/2024` |
| `de` | `de`, `de-DE`, `de-AT` | `,` | day first | `1.234,56`, `31.12.2024` |
| `fr` | `fr`, `fr-FR`, `fr-BE` | `,` | day first | `1 234,56`, `31/12/2024` |
| `es` | `es`, `es-ES` | `,` | day first | `1.234,56`, `31/12/2024` |
| `it` | `it`, `it-IT` | `,` | day first | `1.234,56`, `31/12/2024` |
| `nl` | `nl`, `nl-NL` | `,` | day first | `1.234,56`, `31-12-2024` |
| `pt` | `pt`, `pt-PT`, `pt-BR`, `br` | `,` | day first | `1.234,56`, `31/12/2024` |
| `pl` | `pl`, `pl-PL` | `,` | day first | `1 234,56`, `31.12.2024` |
| `ch` | `ch`, `de-CH`, `fr-CH`, `it-CH` | `.` | day first | `1'234.56`, `31.12.2024` |
| `rs` | `sr`, `sr-Latn-RS`, `hr`, `bs`, `sl`, `mk`, `me` | `,` | day first | `1.234,56`, `31.12.2024.` |
| `iso` | `iso` | `.` | no preference | `1234.56`, `2024-12-31` |

Tags are case-insensitive and `_` works like `-`. Only the language and a two-letter region
matter (`sr-Latn-RS` is `rs`). An unknown language is an error (`unknown locale "xx"`), never a
silent default. The manifest records the preset name in
`options.locale`.

From Rust, the same presets are `hh_core::values::Locale` (`Locale::parse("de-AT")`).

## See also

- [normalized-output.md](normalized-output.md#values-and-dtypes): how parsed values are written
- [concepts.md](concepts.md): where value parsing sits in the pipeline
- [troubleshooting.md](troubleshooting.md#numbers-or-dates-are-read-the-wrong-way): wrong numbers or dates
