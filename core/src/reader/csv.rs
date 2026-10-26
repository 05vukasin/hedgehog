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
//! - Encoding: UTF-8 (BOM stripped) or UTF-16 with a BOM. A file that is not valid UTF-8 (decided
//!   for the whole file at the first invalid field) is read as Windows-1250 when its bytes read as
//!   Central European words ([`single_byte_encoding`]: `Ra\xE8un` is `Račun`, not `Raèun`),
//!   otherwise as Windows-1252.
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
    Windows1250,
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
        None => {
            let enc = single_byte_encoding(open().map_err(err)?).map_err(err)?;
            Ok(collect(open().map_err(err)?, delim, enc, max_cells)?.unwrap_or_default())
        }
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
            let value = match enc {
                // B136: a number or a blank field is never copied into a string.
                Encoding::Utf8 => match std::str::from_utf8(bytes) {
                    Ok(s) => typed_str(s),
                    Err(_) => return Ok(None),
                },
                Encoding::Windows1250 => typed(bytes.iter().map(|&b| windows_1250(b)).collect()),
                Encoding::Windows1252 => typed(bytes.iter().map(|&b| windows_1252(b)).collect()),
            };
            if let Some(value) = value {
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

/// [`typed`] of a borrowed field: the text is copied only when it stays text.
fn typed_str(text: &str) -> Option<Value> {
    if text.trim().is_empty() {
        return None;
    }
    Some(match plain_number(text) {
        Some(x) => Value::Number(x),
        None => Value::Text(text.to_owned()),
    })
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
        while seen < SNIFF_RECORDS && parser.next_record(&mut rec).unwrap_or(false) {
            seen += 1;
            if rec.len() >= 2 {
                *counts.entry(rec.len()).or_default() += 1;
            }
        }
        // Quotes out of place: quoted fields split by the wrong delimiter (`"Payroll staff;⏎AO/AA;⏎
        // Headcount"` read with `;`, B53).
        let consistent = counts.values().copied().max().unwrap_or(0).saturating_sub(2 * parser.stray);
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
    /// Quotes out of place so far: inside an unquoted field, or text right after a closing quote
    /// (the signs of quoted fields split by the wrong delimiter).
    stray: usize,
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
        Parser { r, delim, skip_lf: false, stray: 0 }
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
            let mut i = 0;
            while i < chunk.len() {
                // B136: runs of plain bytes are copied at once (same states, same bytes).
                match state {
                    State::Quoted => {
                        match memchr::memchr(b'"', &chunk[i..]) {
                            Some(k) => {
                                rec.buf.extend_from_slice(&chunk[i..i + k]);
                                state = State::QuoteInQuoted;
                                i += k + 1;
                            }
                            None => {
                                rec.buf.extend_from_slice(&chunk[i..]);
                                i = chunk.len();
                            }
                        }
                        continue;
                    }
                    State::FieldStart | State::Unquoted => {
                        let run = chunk[i..].iter().position(|&b| b == self.delim || b == b'\n' || b == b'\r' || b == b'"').unwrap_or(chunk.len() - i);
                        if run > 0 {
                            rec.buf.extend_from_slice(&chunk[i..i + run]);
                            state = State::Unquoted;
                            i += run;
                            continue;
                        }
                    }
                    State::QuoteInQuoted => {}
                }
                let b = chunk[i];
                i += 1;
                match state {
                    State::QuoteInQuoted if b == b'"' => {
                        rec.buf.push(b'"');
                        state = State::Quoted;
                        continue;
                    }
                    State::QuoteInQuoted if b != self.delim && b != b'\n' && b != b'\r' => self.stray += 1,
                    _ => {}
                }
                if b == self.delim {
                    rec.ends.push(rec.buf.len());
                    state = State::FieldStart;
                } else if b == b'\n' || b == b'\r' {
                    rec.ends.push(rec.buf.len());
                    self.skip_lf = b == b'\r';
                    end = Some(i);
                    break;
                } else if b == b'"' && state == State::FieldStart {
                    state = State::Quoted;
                } else {
                    self.stray += usize::from(b == b'"');
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

/// Windows-1250 byte → char (Central European: Polish, Czech, Slovak, Slovene, Croatian,
/// Serbian Latin, Hungarian, Romanian). Unassigned bytes map to the C1 control of the same value.
fn windows_1250(b: u8) -> char {
    const HIGH: [char; 128] = [
        '€', '\u{81}', '\u{201A}', '\u{83}', '\u{201E}', '…', '†', '‡', //
        '\u{88}', '‰', 'Š', '\u{2039}', 'Ś', 'Ť', 'Ž', 'Ź', //
        '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '•', '\u{2013}', '\u{2014}', //
        '\u{98}', '™', 'š', '\u{203A}', 'ś', 'ť', 'ž', 'ź', //
        '\u{A0}', 'ˇ', '˘', 'Ł', '¤', 'Ą', '¦', '§', '¨', '©', 'Ş', '«', '¬', '\u{AD}', '®', 'Ż', //
        '°', '±', '˛', 'ł', '´', 'µ', '¶', '·', '¸', 'ą', 'ş', '»', 'Ľ', '˝', 'ľ', 'ż', //
        'Ŕ', 'Á', 'Â', 'Ă', 'Ä', 'Ĺ', 'Ć', 'Ç', 'Č', 'É', 'Ę', 'Ë', 'Ě', 'Í', 'Î', 'Ď', //
        'Đ', 'Ń', 'Ň', 'Ó', 'Ô', 'Ő', 'Ö', '×', 'Ř', 'Ů', 'Ú', 'Ű', 'Ü', 'Ý', 'Ţ', 'ß', //
        'ŕ', 'á', 'â', 'ă', 'ä', 'ĺ', 'ć', 'ç', 'č', 'é', 'ę', 'ë', 'ě', 'í', 'î', 'ď', //
        'đ', 'ń', 'ň', 'ó', 'ô', 'ő', 'ö', '÷', 'ř', 'ů', 'ú', 'ű', 'ü', 'ý', 'ţ', '˙',
    ];
    match b {
        0x80.. => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

/// Windows-1250 or Windows-1252 for a file that is not UTF-8, from the whole byte stream.
///
/// Only the bytes that are a letter in both code pages but a consonant in 1250 and a vowel in
/// 1252 (`0xE8` č/è, `0xE6` ć/æ, `0xF8` ř/ø, `0xE0` ŕ/à, `0xE5` ĺ/å, `0xEF` ď/ï, `0xF2` ň/ò and
/// their capitals) vote, by the letters around them: between vowels or after a vowel at a word
/// end it is a consonant (`Poèetno` → `Početno`, `Petroviæ` → `Petrović`: 1250); between
/// consonants, after a consonant at a word end, or alone it is a vowel (`crème`, `caffè`, `à`:
/// 1252). `0xF0`/`0xD0` next to a letter (đ/Đ, while ð is Icelandic only) and the bytes that are
/// a letter in 1250 but a symbol in 1252 (`ł`, `ą`, `ż`, … with letters on both sides: `m³`,
/// `£5` don't vote; a lowercase one needs a lowercase neighbour: `O\xBFROURKE` is a mangled
/// apostrophe, not `OżROURKE`) count for 1250. 1250 wins only with more votes (ties stay 1252) and
/// only when most high bytes stand alone between ASCII bytes (runs are a multi-byte encoding).
fn single_byte_encoding<R: BufRead>(mut r: R) -> io::Result<Encoding> {
    let (mut prev, mut cur) = (b' ', b' ');
    let (mut score, mut high, mut alone) = (0i64, 0u64, 0u64);
    let mut step = |prev: u8, cur: u8, next: u8| {
        if cur >= 0x80 {
            score += vote_1250(prev, cur, next);
            high += 1;
            alone += u64::from(prev < 0x80 && next < 0x80);
        }
    };
    loop {
        let chunk = match r.fill_buf() {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            break;
        }
        for &next in chunk {
            step(prev, cur, next);
            (prev, cur) = (cur, next);
        }
        let n = chunk.len();
        r.consume(n);
    }
    step(prev, cur, b' ');
    // Mostly runs of high bytes: a multi-byte encoding (Shift-JIS, …), not Latin letters.
    let latin = alone * 2 >= high;
    Ok(if latin && score > 0 { Encoding::Windows1250 } else { Encoding::Windows1252 })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sound {
    Vowel,
    Consonant,
    /// A letter whose sound depends on the code page.
    Either,
    /// Not a letter.
    Gap,
}

/// The sound of a byte as a letter, where Windows-1250 and Windows-1252 agree on it.
fn sound(b: u8) -> Sound {
    match b {
        b'a' | b'e' | b'i' | b'o' | b'u' | b'y' | b'A' | b'E' | b'I' | b'O' | b'U' | b'Y' => Sound::Vowel,
        _ if b.is_ascii_alphabetic() => Sound::Consonant,
        0x8A | 0x8E | 0x9A | 0x9E | 0xDF => Sound::Consonant, // Š Ž š ž ß
        0x8C | 0x8D | 0x8F | 0x9C | 0x9D | 0x9F | 0xA3 | 0xA5 | 0xAA | 0xAF | 0xB3 | 0xB9 | 0xBA | 0xBC | 0xBE | 0xBF | 0xFF => {
            Sound::Either
        }
        0xD7 | 0xF7 => Sound::Gap, // × ÷
        0xC0..=0xFE => match b & !0x20 {
            0xC0 | 0xC5 | 0xC6 | 0xC8 | 0xCF | 0xD2 | 0xD8 => Sound::Either,
            0xC7 | 0xD0 | 0xD1 | 0xDE => Sound::Consonant, // Ç, Ð/Đ, Ñ/Ń, Þ/Ţ
            _ => Sound::Vowel,                             // Á, Ã/Ă, Ê/Ę, Õ/Ő, Ù/Ů, …
        },
        _ => Sound::Gap,
    }
}

/// +1/+2 when byte `b` between `prev` and `next` reads as Windows-1250, −1 when as Windows-1252.
fn vote_1250(prev: u8, b: u8, next: u8) -> i64 {
    use Sound::*;
    let letter = |s: Sound| s != Gap;
    let (p, n) = (sound(prev), sound(next));
    let upper = matches!(b, 0x8C | 0x8D | 0x8F | 0xA3 | 0xA5 | 0xAA | 0xAF | 0xBC);
    match b {
        // Consonant in 1250, vowel in 1252.
        0xC0 | 0xC5 | 0xC6 | 0xC8 | 0xCF | 0xD2 | 0xD8 | 0xE0 | 0xE5 | 0xE6 | 0xE8 | 0xEF | 0xF2 | 0xF8 => match (p, n) {
            (Gap, Gap) | (Consonant, Consonant | Gap) | (Gap, Consonant) => -1,
            (Vowel | Gap, Vowel | Gap) => 1,
            _ => 0,
        },
        // đ/Đ: ð is Icelandic only.
        0xD0 | 0xF0 if letter(p) || letter(n) => 1,
        // A letter in 1250 (Ś Ť Ź ś ť ź Ł Ą Ş Ż ł ą ş Ľ ľ ż), a symbol or a rare letter in 1252.
        0x8C | 0x8D | 0x8F | 0x9C | 0x9D | 0x9F | 0xA3 | 0xA5 | 0xAA | 0xAF | 0xB3 | 0xB9 | 0xBA | 0xBC | 0xBE | 0xBF
            if letter(p) && letter(n) && (upper || prev.is_ascii_lowercase() || next.is_ascii_lowercase()) =>
        {
            2
        }
        _ => 0,
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

    /// The parser before B136 (one byte at a time), the reference for the run-copying one.
        fn next_record_reference<R: BufRead>(p: &mut Parser<R>, rec: &mut Record) -> io::Result<bool> {
            rec.buf.clear();
            rec.ends.clear();
            let mut state = State::FieldStart;
            let mut started = false;
            loop {
                let chunk = p.r.fill_buf()?;
                if chunk.is_empty() {
                    if started {
                        rec.ends.push(rec.buf.len());
                    }
                    return Ok(started);
                }
                if std::mem::take(&mut p.skip_lf) && chunk[0] == b'\n' {
                    p.r.consume(1);
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
                        State::QuoteInQuoted if b != p.delim && b != b'\n' && b != b'\r' => p.stray += 1,
                        _ => {}
                    }
                    if b == p.delim {
                        rec.ends.push(rec.buf.len());
                        state = State::FieldStart;
                    } else if b == b'\n' || b == b'\r' {
                        rec.ends.push(rec.buf.len());
                        p.skip_lf = b == b'\r';
                        end = Some(i + 1);
                        break;
                    } else if b == b'"' && state == State::FieldStart {
                        state = State::Quoted;
                    } else {
                        p.stray += usize::from(b == b'"');
                        rec.buf.push(b);
                        state = State::Unquoted;
                    }
                }
                let used = end.unwrap_or(chunk.len());
                p.r.consume(used);
                if end.is_some() {
                    return Ok(true);
                }
            }
        }

    /// Same records, field ends and stray-quote counts as the byte-at-a-time parser, on random
    /// inputs over the special bytes, read through tiny buffers so that runs cross chunk ends.
    #[test]
    fn runs_match_byte_parser() {
        let alphabet = b"ab;,\t|\"\r\n x\"";
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for case in 0..4000 {
            let len = (next() % 80) as usize;
            let input: Vec<u8> = (0..len).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]).collect();
            let cap = 1 + (next() % 9) as usize;
            for delim in [b',', b';', b'\t', b'|'] {
                let mut a = Parser::new(std::io::BufReader::with_capacity(cap, input.as_slice()), delim);
                let mut b = Parser::new(std::io::BufReader::with_capacity(cap, input.as_slice()), delim);
                let (mut ra, mut rb) = (Record::default(), Record::default());
                loop {
                    let ga = a.next_record(&mut ra).unwrap();
                    let gb = next_record_reference(&mut b, &mut rb).unwrap();
                    assert_eq!(ga, gb, "case {case}");
                    assert_eq!((&ra.buf, &ra.ends, a.stray, a.skip_lf), (&rb.buf, &rb.ends, b.stray, b.skip_lf), "case {case}: {input:?}");
                    if !ga {
                        break;
                    }
                }
            }
        }
    }

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
        // Quoted headers with `;` and line breaks inside, one record (B53).
        assert_eq!(
            sniff_delimiter(b"Year,\"Payroll staff;\nAO/AA;\nHeadcount\",\"Payroll staff;\nEO;\nHeadcount\"\n2026,12,159\n"),
            b','
        );
        // Escaped quotes inside properly quoted fields (JSON geometry) are not out of place.
        assert_eq!(
            sniff_delimiter(b"code;name;shape\n1;A;\"{\"\"x\"\": [1, 2], \"\"y\"\": 3}\"\n2;B;\"{\"\"x\"\": [4, 5]}\"\n"),
            b';'
        );
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
    fn central_european_files_read_as_windows_1250() {
        // A Serbian bank export in cp1250: č ć đ Č Ć Đ must not become è æ ð È Æ Ð.
        let cells = read(b"Opis;Ra\xe8un partnera\nPo\xe8etno stanje;100\nUplata \xd0or\xf0e Petrovi\xe6;5\n\xc8a\xe8ak \x9aum\x9e;3\n");
        assert_eq!(
            rows(&cells),
            [
                [t("Opis"), t("Račun partnera")],
                [t("Početno stanje"), n(100.0)],
                [t("Uplata Đorđe Petrović"), n(5.0)],
                [t("Čačak šumž"), n(3.0)]
            ]
        );
        // Polish and Czech letters that are symbols in Windows-1252.
        let cells = read(b"Miasto;Kwota\nWa\xb3brzych;1\nm\xb9ka \xbfona;2\nP\xf8\xedbram;3\n");
        assert_eq!(rows(&cells)[1..], [[t("Wałbrzych"), n(1.0)], [t("mąka żona"), n(2.0)], [t("Příbram"), n(3.0)]]);
        // Western files stay Windows-1252: French, German, Spanish, Italian, Danish.
        let cells = read(b"Nom;Prix\nCr\xe8me br\xfbl\xe9e;1\nTr\xe8s \xe0 la mode;2\nGr\xf6\xdfe;3\nNi\xf1o a\xf1o;4\ncaff\xe8;5\nK\xf8benhavn S\xf8ren;6\n");
        assert_eq!(
            rows(&cells)[1..],
            [
                [t("Crème brûlée"), n(1.0)],
                [t("Très à la mode"), n(2.0)],
                [t("Größe"), n(3.0)],
                [t("Niño año"), n(4.0)],
                [t("caffè"), n(5.0)],
                [t("København Søren"), n(6.0)]
            ]
        );
        // Units and prices keep their 1252 symbols.
        let cells = read(b"Fl\xe4che;Preis\n12 m\xb2;\xa35\n1 m\xb3;\xbd\nLAING O\xbfROURKE;1\n");
        assert_eq!(rows(&cells)[1..], [[t("12 m²"), t("£5")], [t("1 m³"), t("½")], [t("LAING O¿ROURKE"), n(1.0)]]);
        // Shift-JIS (runs of high bytes) is not a Latin code page: left as Windows-1252.
        let sjis = b"\x93\xc1\x92\xe8\x94\xed\x8d\xd0\x92\x6e\x95\xfb;1\n";
        assert_eq!(single_byte_encoding(&sjis[..]).unwrap(), Encoding::Windows1252);
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
