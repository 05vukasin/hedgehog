//! Delimited text (CSV, TSV, `;` or `|` separated) as a one-sheet [`Workbook`] named after the
//! file stem.
//!
//! - Delimiter: `.tsv` → tab; otherwise sniffed among `,` `;` tab `|` on the first records, quotes
//!   respected: the one that splits the most records into the same number (≥ 2) of fields wins.
//!   Ties go to tab, `;`, `|`, then `,` (a `;` that splits as evenly as `,` is the separator of a
//!   decimal-comma file). A single column reads with `,`.
//! - Quoting: RFC 4180 (quoted fields, `""`, delimiters and newlines inside quotes), lenient on
//!   malformed input: text after a closing quote is kept, an unclosed quote runs to the end of
//!   the file. A record ends at `\n`, `\r\n` or a lone `\r`; one record is one row, so a blank
//!   line is an empty row.
//! - Encoding: UTF-8 (BOM stripped) or UTF-16 with a BOM. A file that is not valid UTF-8 is read
//!   as Windows-1252, decided for the whole file at the first invalid field.
//! - Values: a field becomes a [`Value::Number`] only when it is an unambiguous plain number
//!   ([`plain_number`]); everything else stays [`Value::Text`], and the value normalizer parses it
//!   with per-column locale inference (`1.200,50`, `1,200.50`, `12%`, dates) exactly like numbers
//!   stored as text in a workbook. Nothing ambiguous is guessed here. Whitespace-only fields are
//!   empty cells.
//! - Memory: records are streamed; only the cells are kept.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Cursor, Read};
use std::path::Path;

use super::{err, Error};
use crate::model::{Cell, Sheet, Value, Workbook};

/// Bytes read up front for the encoding check and the delimiter sniff.
const HEAD: usize = 64 << 10;
/// Records the delimiter sniff looks at.
const SNIFF_RECORDS: usize = 200;
/// Candidate delimiters in tie-break order.
const DELIMITERS: [u8; 4] = *b"\t;|,";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Utf8,
    Windows1252,
}

/// `max_cells`: reading stops with a limit error past this many cells (checked while reading,
/// so a huge file is refused before it fills memory).
pub(super) fn open(path: &Path, max_cells: u64) -> Result<Workbook, Error> {
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let tsv = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("tsv"));
    let mut head = Vec::new();
    File::open(path).map_err(err)?.take(HEAD as u64).read_to_end(&mut head).map_err(err)?;
    let cells = if utf16_bom(&head).is_some() {
        // Rare (Excel's "Unicode text"); decoded in memory as a whole.
        read_bytes(&std::fs::read(path).map_err(err)?, tsv, max_cells)?
    } else {
        let bom = if head.starts_with(b"\xEF\xBB\xBF") { 3 } else { 0 };
        let open = || -> io::Result<BufReader<File>> {
            let mut r = BufReader::with_capacity(HEAD, File::open(path)?);
            let skip = bom.min(r.fill_buf()?.len());
            r.consume(skip);
            Ok(r)
        };
        read_cells(open, &head[bom..], tsv, max_cells)?
    };
    Ok(Workbook { sheets: vec![Sheet { name, cells, merges: Vec::new(), formats: Vec::new() }] })
}

/// [`open`] on bytes in memory, without the sheet around the cells.
fn read_bytes(bytes: &[u8], tsv: bool, max_cells: u64) -> Result<Vec<Cell>, Error> {
    if let Some(le) = utf16_bom(bytes) {
        let unit = |c: &[u8]| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) };
        let units = bytes[2..].chunks_exact(2).map(unit);
        let text: String = char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)).collect();
        return read_cells(|| Ok(Cursor::new(text.as_bytes())), &text.as_bytes()[..text.len().min(HEAD)], tsv, max_cells);
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    read_cells(|| Ok(Cursor::new(bytes)), &bytes[..bytes.len().min(HEAD)], tsv, max_cells)
}

/// `Some(little endian)` for a UTF-16 byte order mark.
fn utf16_bom(b: &[u8]) -> Option<bool> {
    match b {
        [0xFF, 0xFE, ..] => Some(true),
        [0xFE, 0xFF, ..] => Some(false),
        _ => None,
    }
}

/// Cells of the whole input. `open` gives a fresh reader positioned after any BOM (called twice
/// when the input turns out not to be UTF-8); `head` is its first bytes.
fn read_cells<R: BufRead>(mut open: impl FnMut() -> io::Result<R>, head: &[u8], tsv: bool, max_cells: u64) -> Result<Vec<Cell>, Error> {
    if head.contains(&0) {
        return Err(Error("not a spreadsheet or delimited text file (binary content)".into()));
    }
    let delim = if tsv { b'\t' } else { sniff_delimiter(head) };
    match collect(open().map_err(err)?, delim, Encoding::Utf8, max_cells)? {
        Some(cells) => Ok(cells),
        None => Ok(collect(open().map_err(err)?, delim, Encoding::Windows1252, max_cells)?.unwrap_or_default()),
    }
}

/// `None` when reading as UTF-8 and a field is not valid UTF-8.
fn collect<R: BufRead>(r: R, delim: u8, enc: Encoding, max_cells: u64) -> Result<Option<Vec<Cell>>, Error> {
    let mut parser = Parser::new(r, delim);
    let mut rec = Record::default();
    let mut cells = Vec::new();
    let mut row = 0u32;
    while parser.next_record(&mut rec).map_err(err)? {
        for (col, bytes) in rec.fields().enumerate() {
            let text = match enc {
                Encoding::Utf8 => match std::str::from_utf8(bytes) {
                    Ok(s) => s.to_owned(),
                    Err(_) => return Ok(None),
                },
                Encoding::Windows1252 => bytes.iter().map(|&b| windows_1252(b)).collect(),
            };
            if let Some(value) = typed(text) {
                cells.push(Cell { row, col: col as u32, value: Some(value), formula: None });
            }
        }
        if cells.len() as u64 > max_cells {
            return Err(super::limits::too_many_cells(max_cells));
        }
        row = row.saturating_add(1);
    }
    Ok(Some(cells))
}

/// `None` for a whitespace-only field.
fn typed(text: String) -> Option<Value> {
    if text.trim().is_empty() {
        return None;
    }
    Some(match plain_number(&text) {
        Some(x) => Value::Number(x),
        None => Value::Text(text),
    })
}

/// A number only when no reader could take it for anything else: an optional sign, an integer
/// part without leading zeros (`0` itself is fine), an optional `.` with digits after it, an
/// optional exponent, surrounding spaces. Text, left to the value normalizer: `007`, `1.200,50`,
/// `1,5`, `.5`, `5.`, `12%`, `1.200` (one to three digits, `.`, exactly three digits: `1.200` is
/// 1200 in a decimal-comma file) and more than 15 significant digits (IDs and account numbers
/// that a float would round).
fn plain_number(s: &str) -> Option<f64> {
    let t = s.trim_matches(|c| c == ' ' || c == '\t');
    let b = t.as_bytes();
    let digits = |mut i: usize| {
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        i
    };
    let int_start = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let mut i = digits(int_start);
    let int_len = i - int_start;
    if int_len == 0 || (int_len > 1 && b[int_start] == b'0') {
        return None;
    }
    let mut frac_len = 0;
    if b.get(i) == Some(&b'.') {
        let end = digits(i + 1);
        frac_len = end - i - 1;
        if frac_len == 0 {
            return None;
        }
        i = end;
    }
    let mut exponent = false;
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let start = i + 1 + usize::from(matches!(b.get(i + 1), Some(b'+' | b'-')));
        i = digits(start);
        if i == start {
            return None;
        }
        exponent = true;
    }
    if i != b.len() {
        return None;
    }
    if frac_len == 3 && int_len <= 3 && b[int_start] != b'0' && !exponent {
        return None;
    }
    let mantissa = &t[int_start..int_start + int_len + usize::from(frac_len > 0) + frac_len];
    if mantissa.bytes().filter(u8::is_ascii_digit).skip_while(|&d| d == b'0').count() > 15 {
        return None;
    }
    t.parse::<f64>().ok().filter(|x| x.is_finite())
}

/// The delimiter that splits the most of the first records into the same number of fields.
fn sniff_delimiter(head: &[u8]) -> u8 {
    // A full head usually ends inside a record; drop that partial record.
    let sample = match head.iter().rposition(|&b| b == b'\n') {
        Some(end) if head.len() == HEAD => &head[..=end],
        _ => head,
    };
    let mut best = (b',', 0usize);
    let mut rec = Record::default();
    for delim in DELIMITERS {
        let mut parser = Parser::new(sample, delim);
        let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
        let mut seen = 0;
        // Records with a quote inside a field: quoted fields split by the wrong delimiter
        // (`"Payroll staff;⏎AO/AA;⏎Headcount"` read with `;`, B53).
        let mut stray = 0usize;
        while seen < SNIFF_RECORDS && parser.next_record(&mut rec).unwrap_or(false) {
            seen += 1;
            if rec.len() >= 2 {
                *counts.entry(rec.len()).or_default() += 1;
            }
            stray += rec.fields().any(|f| f.contains(&b'"')) as usize;
        }
        let consistent = counts.values().copied().max().unwrap_or(0).saturating_sub(2 * stray);
        if consistent > best.1 {
            best = (delim, consistent);
        }
    }
    best.0
}

/// Fields of one record: their bytes back to back, and where each ends.
#[derive(Default)]
struct Record {
    buf: Vec<u8>,
    ends: Vec<usize>,
}

impl Record {
    fn len(&self) -> usize {
        self.ends.len()
    }

    fn fields(&self) -> impl Iterator<Item = &[u8]> {
        self.ends.iter().scan(0, |start, &end| {
            let f = &self.buf[*start..end];
            *start = end;
            Some(f)
        })
    }
}

/// Streaming RFC 4180 record parser over raw bytes. Every byte it acts on (delimiter, quote, CR,
/// LF) is ASCII, so it works the same on UTF-8 and single-byte encodings.
struct Parser<R: BufRead> {
    r: R,
    delim: u8,
    /// The last record ended at a `\r`; a `\n` right after it belongs to that line break.
    skip_lf: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    FieldStart,
    Unquoted,
    Quoted,
    /// A `"` inside a quoted field: `""` is a literal quote, anything else closes the quotes.
    QuoteInQuoted,
}

impl<R: BufRead> Parser<R> {
    fn new(r: R, delim: u8) -> Self {
        Parser { r, delim, skip_lf: false }
    }

    /// Reads the next record into `rec`. `false` at the end of the input.
    fn next_record(&mut self, rec: &mut Record) -> io::Result<bool> {
        rec.buf.clear();
        rec.ends.clear();
        let mut state = State::FieldStart;
        let mut started = false;
        loop {
            let chunk = self.r.fill_buf()?;
            if chunk.is_empty() {
                if started {
                    rec.ends.push(rec.buf.len());
                }
                return Ok(started);
            }
            if std::mem::take(&mut self.skip_lf) && chunk[0] == b'\n' {
                self.r.consume(1);
                continue;
            }
            started = true;
            let mut end = None;
            for (i, &b) in chunk.iter().enumerate() {
                match state {
                    State::Quoted => {
                        if b == b'"' {
                            state = State::QuoteInQuoted;
                        } else {
                            rec.buf.push(b);
                        }
                        continue;
                    }
                    State::QuoteInQuoted if b == b'"' => {
                        rec.buf.push(b'"');
                        state = State::Quoted;
                        continue;
                    }
                    _ => {}
                }
                if b == self.delim {
                    rec.ends.push(rec.buf.len());
                    state = State::FieldStart;
                } else if b == b'\n' || b == b'\r' {
                    rec.ends.push(rec.buf.len());
                    self.skip_lf = b == b'\r';
                    end = Some(i + 1);
                    break;
                } else if b == b'"' && state == State::FieldStart {
                    state = State::Quoted;
                } else {
                    rec.buf.push(b);
                    state = State::Unquoted;
                }
            }
            let used = end.unwrap_or(chunk.len());
            self.r.consume(used);
            if end.is_some() {
                return Ok(true);
            }
        }
    }
}

/// Windows-1252 byte → char. 0x80–0x9F hold typographic characters; the five unassigned bytes
/// map to the C1 control of the same value, as WHATWG decoders do. Everything else is Latin-1.
fn windows_1252(b: u8) -> char {
    const C1: [char; 32] = [
        '€', '\u{81}', '\u{201A}', 'ƒ', '\u{201E}', '…', '†', '‡', //
        '\u{2C6}', '‰', 'Š', '\u{2039}', 'Œ', '\u{8D}', 'Ž', '\u{8F}', //
        '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '•', '\u{2013}', '\u{2014}', //
        '\u{2DC}', '™', 'š', '\u{203A}', 'œ', '\u{9D}', 'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9F => C1[(b - 0x80) as usize],
        _ => b as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &[u8]) -> Vec<Cell> {
        read_bytes(text, false, u64::MAX).unwrap()
    }

    fn rows(cells: &[Cell]) -> Vec<Vec<Option<Value>>> {
        let h = cells.iter().map(|c| c.row + 1).max().unwrap_or(0);
        let w = cells.iter().map(|c| c.col + 1).max().unwrap_or(0);
        let mut out = vec![vec![None; w as usize]; h as usize];
        for c in cells {
            out[c.row as usize][c.col as usize] = c.value.clone();
        }
        out
    }

    fn t(s: &str) -> Option<Value> {
        Some(Value::Text(s.into()))
    }

    fn n(x: f64) -> Option<Value> {
        Some(Value::Number(x))
    }

    #[test]
    fn sniffs_the_delimiter() {
        assert_eq!(sniff_delimiter(b"a,b,c\n1,2,3\n4,5,6\n"), b',');
        assert_eq!(sniff_delimiter(b"Name;Amount\nBeograd;1.234,56\nNis;2.000,00\n"), b';');
        assert_eq!(sniff_delimiter(b"a\tb, c\n1\t2,5\n"), b'\t');
        assert_eq!(sniff_delimiter(b"a|b|c\n1|2|3\n"), b'|');
        // Commas inside quotes do not count; the semicolons split every line evenly.
        assert_eq!(sniff_delimiter(b"\"Smith, J\";\"x,y,z\"\n\"Doe, A\";\"p\"\n"), b';');
        // Commas outside quotes, semicolons only inside them.
        assert_eq!(sniff_delimiter(b"\"a;b\",1\n\"c;d;e\",2\n\"f\",3\n"), b',');
        // A title line with no delimiter does not change the vote.
        assert_eq!(sniff_delimiter(b"Report 2024\n\nx;y;z\n1;2;3\n4;5;6\n"), b';');
        assert_eq!(sniff_delimiter(b"one column\nonly\n"), b',');
        assert_eq!(sniff_delimiter(b""), b',');
    }

    #[test]
    fn tsv_extension_means_tab() {
        let cells = read_bytes(b"a;b\tc\n", true, u64::MAX).unwrap();
        assert_eq!(rows(&cells), [[t("a;b"), t("c")]]);
    }

    #[test]
    fn quotes_escapes_and_embedded_newlines() {
        let cells = read(b"name,note\r\n\"Smith, J\",\"say \"\"hi\"\"\"\r\n\"multi\r\nline\",x\r\n");
        assert_eq!(
            rows(&cells),
            [vec![t("name"), t("note")], vec![t("Smith, J"), t("say \"hi\"")], vec![t("multi\r\nline"), t("x")]]
        );
        // A quote inside an unquoted field is literal; text after a closing quote is kept.
        let cells = read(b"a\"b,\"c\"d,e\n");
        assert_eq!(rows(&cells), [[t("a\"b"), t("cd"), t("e")]]);
        // Lone CR line ends; an unclosed quote runs to the end.
        let cells = read(b"a,b\r1,2\r\"open,3\n4");
        assert_eq!(rows(&cells), [vec![t("a"), t("b")], vec![n(1.0), n(2.0)], vec![t("open,3\n4"), None]]);
    }

    #[test]
    fn blank_lines_ragged_rows_and_empty_fields() {
        let cells = read(b"a,b,c\n\n1,,3,4\n5\n ,\"\",x\n");
        assert_eq!(
            rows(&cells),
            [
                vec![t("a"), t("b"), t("c"), None],
                vec![None, None, None, None],
                vec![n(1.0), None, n(3.0), n(4.0)],
                vec![n(5.0), None, None, None],
                vec![None, None, t("x"), None],
            ]
        );
        let positions: Vec<(u32, u32)> = cells.iter().map(|c| (c.row, c.col)).collect();
        assert_eq!(positions, [(0, 0), (0, 1), (0, 2), (2, 0), (2, 2), (2, 3), (3, 0), (4, 2)]);
        assert!(read(b"").is_empty());
    }

    #[test]
    fn bom_and_encodings() {
        let cells = read("\u{FEFF}Град,Износ\nНиш,5\n".as_bytes());
        assert_eq!(rows(&cells), [[t("Град"), t("Износ")], [t("Ниш"), n(5.0)]]);
        // Not UTF-8 → Windows-1252, for the whole file.
        let cells = read(b"Caf\xe9;Prix\nCr\xe8me br\xfbl\xe9e;3,50 \x80\n\x93quoted\x94;1\n");
        assert_eq!(
            rows(&cells),
            [[t("Café"), t("Prix")], [t("Crème brûlée"), t("3,50 €")], [t("\u{201C}quoted\u{201D}"), n(1.0)]]
        );
        let le = "a\tb\n1\t€\n".encode_utf16().flat_map(u16::to_le_bytes);
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain(le).collect();
        assert_eq!(rows(&read(&utf16)), [[t("a"), t("b")], [n(1.0), t("€")]]);
        assert!(read_bytes(b"PK\x03\x04\x00\x00binary", false, u64::MAX).is_err());
    }

    #[test]
    fn only_plain_numbers_are_numbers() {
        for (s, x) in [
            ("0", 0.0),
            ("42", 42.0),
            ("-3.5e2", -350.0),
            ("+7", 7.0),
            ("0.125", 0.125),
            ("3.25", 3.25),
            ("1234.567", 1234.567),
            ("4711.00", 4711.0),
            ("1.5E-3", 0.0015),
            ("1.200e3", 1200.0),
            ("  12 ", 12.0),
            ("123456789012345", 123456789012345.0),
        ] {
            assert_eq!(plain_number(s), Some(x), "{s}");
        }
        for s in [
            "007", "-01", "00.5", "1.200,50", "1,200.50", "1,5", "1.200", "-12.500", "1 200", ".5", "5.", "1e", "1e+",
            "12%", "$5", "1-2", "2024-01-31", "31.12.2024", "inf", "NaN", "1.2.3", "--1", "+", "", "1234567890123456",
            "0.1234567890123456",
        ] {
            assert_eq!(plain_number(s), None, "{s}");
        }
        assert_eq!(rows(&read(b"007,1.200,50,-3.5e2\n")), [[t("007"), t("1.200"), n(50.0), n(-350.0)]]);
        assert_eq!(rows(&read(b"007;1.200,50;-3.5e2\n")), [[t("007"), t("1.200,50"), n(-350.0)]]);
    }

    #[test]
    fn records_across_buffer_boundaries() {
        // A tiny buffer puts quotes, CR LF pairs and delimiters on chunk edges.
        let data = b"a,\"b\"\"c\"\r\n\"x\ny\",2\r\n3,4";
        let mut p = Parser::new(BufReader::with_capacity(1, &data[..]), b',');
        let mut rec = Record::default();
        let mut got = Vec::new();
        while p.next_record(&mut rec).unwrap() {
            got.push(rec.fields().map(|f| String::from_utf8(f.to_vec()).unwrap()).collect::<Vec<_>>());
        }
        assert_eq!(got, [vec!["a", "b\"c"], vec!["x\ny", "2"], vec!["3", "4"]]);
    }
}
