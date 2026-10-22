# Locales: numbers, dates and currencies

How hedgehog reads numbers, dates, currencies, missing values and total labels written in US,
European and other conventions, how it settles ambiguous values, and when to pass a locale.

hedgehog is locale-neutral: no convention is the default. `1,200.50` and `1.200,50` are both
read as 1200.5, `12/31/2024` and `31.12.2024` both as 31 December 2024, and a file may mix
conventions from column to column. Only values that can honestly be read two ways (`1.200`,
`03/04/2024`) need a decision, and that decision is made per column from the other values in it.

Contents: [Numbers](#numbers) · [Dates](#dates) · [Currencies](#currencies) ·
[Missing values and booleans](#missing-values-and-booleans) ·
[Footnote marks, censored and suppressed values](#footnote-marks-censored-and-suppressed-values) · [Text](#text) ·
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
| `1.234*`, `567 (a)`, `123¹`, `12 (r)` | 1234, 567, 123, 12 | trailing footnote marks, kept per column ([below](#footnote-marks-censored-and-suppressed-values)) |
| `7 p`, `12.3 e`, `5 bep`, `20560.32e` | 7, 12.3, 5, 20560.32 | statistical flag letters ([below](#footnote-marks-censored-and-suppressed-values)) |

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

- placeholders: `-`, `–`, `—`, `/`, `..`, `...`, `…`, `?`, `*`, `_`, `x`, `xx`, `n.p.`, and in table
  columns the bracketed codes `(X)`, `[x]`, `(NA)`, `[NA]`, `[z]` (US Census, ONS);
- `:` (Eurostat's "not available"), also with flag letters: `: c`, `:z`, `: bu`;
- words: `n/a`, `NA`, `n.a.`, `n/d`, `null`, `nil`, `none`, `nan`, `missing`, `unknown`,
  `not available`, `not applicable`, `no data`, `tbd`, German `k.A.` / `keine Angabe` / `n.v.` /
  `n.b.`, Dutch `niet beschikbaar`, French `n.c.` / `non disponible` / `non communiqué`, Spanish
  `s.d.` / `sin datos` / `no disponible`, Italian `non disponibile`, Polish `b.d.` / `brak danych`,
  and Serbian/Croatian equivalents (`nema`, `nepoznato`, ...);
- Excel errors: `#N/A`, `#REF!`, `#DIV/0!`, `#VALUE!`, `#NAME?`, `#NUM!`, `#NULL!`, `#SPILL!`,
  `#CALC!`.

Booleans: `yes`/`no`, `y`/`n`, `true`/`false`, `✓`/`✗`, `ja`/`nein`, `oui`/`non`, `si`, `da`/`ne`
(Latin and Cyrillic). `1`/`0` and `x` are not taken as booleans.

## Footnote marks, censored and suppressed values

Statistical tables mark numbers and leave some cells without a number on purpose. hedgehog reads
the number, keeps the mark, and does not let these cells turn a numeric column into text.

- **Footnote marks** after a number: `*`, `†`, `‡`, superscript digits (`123¹`, `12¹²`), one or two
  letters or digits in parentheses (`12 (r)`, `6077.61 (p)`, `567 (a)`), and statistical **flag
  letters** after a space: `b` break in series, `c` confidential, `d` definition differs,
  `e` estimated, `f` forecast, `n` not significant, `p` provisional, `r` revised, `s` estimate,
  `u` low reliability, `z` not applicable, alone or several in alphabetical order as Eurostat
  writes them (`12.3 bep`). Glued to the digits only `e`, `p` and `r` (`20560.32e`). Units win over
  flags (`5 s` is 5 seconds), and letter runs that are words (`100 pen`) are not flags. The mark is
  removed from the value and kept: the column's `footnote_marks` in the manifest lists each mark
  with how many values carried it and their cells, and the table counts
  `footnote_marks_stripped` ([normalized output](normalized-output.md#values-and-dtypes)).
- **Footnote marks on text labels** (`Delta²`, `Germany *`, `Gamma†`, `Norway (1)`) are removed
  and kept in the label column's `footnote_marks` the same way, when at most half of the column's
  labels carry one. **Unit exponents are not marks**: a `²` / `³` right after a length unit (`km²`,
  `m²`, `cm²`, `m³`, `Mio. m³`, `kg/m³`, `ft²`, Cyrillic `км²`) stays in headers and labels, and
  subscripts (`CO₂`, `H₂O`), Roman numerals (`Fund II`) and plain numbers (`Area 51`,
  `Formula 1`) are part of the name.
- **Censored values** (`<5`, `> 1000`, `≤ 0,5`, US Census `(Z)` for less than half the unit), **suppressed values** (`c`, `[c]`, `(c)`, `(D)`,
  `(S)`, `F`, `[low]`, `confidential`, `suppressed`, `withheld`, `vertraulich`, `confidentiel`,
  `poverljivo`, ...) and **pointers to notes** (`see note`, `see footnote`, `siehe Fußnote`,
  `voir note`, ...) stay text in the output (an upper bound is not a value), but they do not count
  against a numeric column: the column is numeric when its numbers reach 90% of the other
  non-missing values and outnumber these cells (or there is nothing else). The table's `warnings`
  list them (`column 'Count' (integer): 2 censored, suppressed or see-note value(s) kept as text:
  E3 '<5', E5 'c'`).
- **A few words among many numbers** (`approx.` twice in a column of 12 numbers) also keep the column
  numeric: at most three distinct texts without digits, numbers at least 75% of the values and three
  times as many as these texts. The texts stay text and are listed in `warnings`.

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
   When the text values alone do not decide, the column's **numbers stored as numbers** (typed
   cells in a workbook; plain fields like `12.3` or `500` in a CSV) do: among whole numbers an
   `x.xxx` groups thousands (`4.405` next to `3178` and `2411` is 4405), among fractions it is read
   the way that gives the size of the other values (`40.805` next to `12.3` and `8.4` is 40.805, not
   40805). Plain integers written as text (`500`) also support the thousands reading; integers
   with leading zeros (`00123`, IDs) do not.
3. **The sheet's evidence**, when the column has none; then cross-evidence (a sheet with decimal
   commas writes dates day first; a month-first sheet uses the decimal point; dates written
   day-first with dots, `31.12.2024`, mean a decimal comma when no number decides).
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
