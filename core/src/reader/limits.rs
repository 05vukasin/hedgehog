//! Per-file limits for untrusted input, checked before and while a file is read: decompressed
//! size of a zip package (zip bombs), cells, shared strings and wall-clock time; and clear errors
//! for password-protected files.
//!
//! The checks that need no parsing run before calamine sees the file: the zip central directory
//! declares every part's uncompressed size (and the zip reader refuses to inflate a part past it),
//! an ODS `content.xml` is scanned for its expanded cell count (`number-columns-repeated` /
//! `number-rows-repeated` let a few bytes ask for billions of cells), and a large
//! `sharedStrings.xml` has its `<si>` entries counted. Cells and time are checked again while
//! reading.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::time::Instant;

use memchr::memmem;

use super::merges::{local, tags};
use super::Error;

/// Limits on one file. The defaults allow every real file we know of and stop the files built to
/// exhaust memory; [`Limits::deadline`] is off by default (the MCP server sets one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Sum of the uncompressed sizes of a zip package's parts (xlsx / xlsm / xlsb / ods).
    pub max_decompressed_bytes: u64,
    /// Non-empty cells in the whole workbook (an ODS file also counts the empty cells its reader
    /// has to fill in between).
    pub max_cells: u64,
    /// Entries of the shared string table (xlsx).
    pub max_shared_strings: u64,
    /// Reading stops with an error after this instant (also checked by
    /// [`crate::normalize::try_normalize_workbook`]).
    pub deadline: Option<Instant>,
}

pub const DEFAULT_MAX_DECOMPRESSED_BYTES: u64 = 2 << 30;
pub const DEFAULT_MAX_CELLS: u64 = 20_000_000;
pub const DEFAULT_MAX_SHARED_STRINGS: u64 = 10_000_000;

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_decompressed_bytes: DEFAULT_MAX_DECOMPRESSED_BYTES,
            max_cells: DEFAULT_MAX_CELLS,
            max_shared_strings: DEFAULT_MAX_SHARED_STRINGS,
            deadline: None,
        }
    }
}

impl Limits {
    /// `Err` once the deadline has passed.
    pub fn check_time(&self) -> Result<(), Error> {
        match self.deadline {
            Some(d) if Instant::now() >= d => Err(Error::limit("reading took longer than the time limit")),
            _ => Ok(()),
        }
    }

    pub(super) fn check_cells(&self, n: u64) -> Result<(), Error> {
        if n > self.max_cells {
            return Err(Error::limit(format!(
                "the workbook has more than {} cells (limit {})",
                thousands(self.max_cells),
                thousands(self.max_cells)
            )));
        }
        Ok(())
    }
}

pub(super) fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn mb(n: u64) -> String {
    format!("{:.0} MB", n as f64 / 1_048_576.0)
}

const PASSWORD: &str =
    "the file is password-protected (encrypted); hedgehog cannot read encrypted workbooks — save a copy without a password";

/// An OLE container holding an `EncryptedPackage` stream is an encrypted xlsx / xlsb (Office's
/// "Encrypt with password"); calamine would report it as a broken xls.
pub(super) fn check_ole(path: &Path) -> Result<(), Error> {
    // The directory entry's name, UTF-16LE.
    let name: Vec<u8> = "EncryptedPackage".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let finder = memmem::Finder::new(&name);
    let mut f = BufReader::with_capacity(1 << 20, File::open(path).map_err(Error::from_io)?);
    let mut buf = vec![0u8; 1 << 20];
    let keep = name.len() - 1;
    let mut filled = 0;
    loop {
        let n = f.read(&mut buf[filled..]).map_err(Error::from_io)?;
        if n == 0 {
            return Ok(());
        }
        filled += n;
        if finder.find(&buf[..filled]).is_some() {
            return Err(Error(PASSWORD.into()));
        }
        if filled == buf.len() {
            buf.copy_within(filled - keep.., 0);
            filled = keep;
        }
    }
}

/// Maps calamine's password errors (xls `FILEPASS`, encrypted zip entries) to [`PASSWORD`].
pub(super) fn password_error(msg: &str) -> Option<Error> {
    let m = msg.to_ascii_lowercase();
    (m.contains("password") || m.contains("encrypt")).then(|| Error(PASSWORD.into()))
}

/// Checks a zip package (xlsx / xlsb / ods) before it is parsed.
pub(super) fn check_zip(path: &Path, limits: &Limits) -> Result<(), Error> {
    let Ok(mut zip) = zip::ZipArchive::new(BufReader::new(File::open(path).map_err(Error::from_io)?)) else {
        return Ok(()); // not a readable zip: calamine reports it
    };
    let mut total: u64 = 0;
    let mut shared_strings = None;
    let mut ods = false;
    for i in 0..zip.len() {
        let Ok(f) = zip.by_index_raw(i) else { continue };
        total = total.saturating_add(f.size());
        if f.encrypted() {
            return Err(Error(PASSWORD.into()));
        }
        let name = f.name().unwrap_or_default().to_ascii_lowercase();
        if name.ends_with("sharedstrings.xml") {
            shared_strings = Some((i, f.size()));
        } else if name == "content.xml" {
            ods = true;
        }
    }
    if total > limits.max_decompressed_bytes {
        return Err(Error::limit(format!(
            "the file decompresses to {}, more than the {} limit (a zip bomb?)",
            mb(total),
            mb(limits.max_decompressed_bytes)
        )));
    }
    // A small table cannot exceed the limit (an entry takes at least 5 bytes: `<si/>`).
    if let Some((i, size)) = shared_strings
        && size / 5 > limits.max_shared_strings
    {
        let f = zip.by_index(i).map_err(|e| Error(e.to_string()))?;
        let n = count_shared_strings(f, limits)?;
        if n > limits.max_shared_strings {
            return Err(Error::limit(format!(
                "the file has more than {} shared strings (limit {})",
                thousands(limits.max_shared_strings),
                thousands(limits.max_shared_strings)
            )));
        }
    }
    if ods {
        if let Ok(m) = zip.by_name("META-INF/manifest.xml") {
            let mut s = Vec::new();
            let _ = m.take(4 << 20).read_to_end(&mut s);
            if memmem::find(&s, b"encryption-data").is_some() {
                return Err(Error(PASSWORD.into()));
            }
        }
        if let Ok(f) = zip.by_name("content.xml") {
            let n = ods_cells(f, limits)?;
            limits.check_cells(n)?;
        }
    }
    Ok(())
}

/// Calls `each` on the complete tags of a streamed XML part (a tag cut by a chunk boundary is
/// carried to the next chunk). Stops early when `each` returns `false`.
fn scan_tags(mut r: impl Read, limits: &Limits, mut each: impl FnMut(&super::merges::Tag) -> bool) -> Result<(), Error> {
    let mut buf: Vec<u8> = Vec::with_capacity(1 << 20);
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        limits.check_time()?;
        let n = r.read(&mut chunk).map_err(|e| Error(e.to_string()))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let Some(end) = memchr::memrchr(b'>', &buf) else {
            if buf.len() > 64 << 20 {
                return Err(Error("corrupt file: a tag longer than 64 MB".into()));
            }
            continue;
        };
        for t in tags(&buf[..=end]) {
            if !each(&t) {
                return Ok(());
            }
        }
        buf.drain(..=end);
    }
    Ok(())
}

fn count_shared_strings(r: impl Read, limits: &Limits) -> Result<u64, Error> {
    let mut n = 0u64;
    let max = limits.max_shared_strings;
    scan_tags(r, limits, |t| {
        if !t.closing && local(t.name) == b"si" {
            n += 1;
        }
        n <= max
    })?;
    Ok(n)
}

const ODS_MAX_ROWS: u64 = 1_048_576;
const ODS_MAX_COLUMNS: u64 = 16_384;

/// Cells calamine's ODS reader would allocate for `content.xml`: for every sheet the cells of
/// every row up to its last non-empty cell (repeated columns expanded), and the dense rectangle
/// from the first to the last non-empty row (repeated rows expanded) — the larger of the two.
fn ods_cells(r: impl Read, limits: &Limits) -> Result<u64, Error> {
    #[derive(Default)]
    struct Sheet {
        listed: u64,
        rows: u64,
        width: u64,
        first: Option<u64>,
        last_end: u64,
    }
    impl Sheet {
        fn cells(&self) -> u64 {
            let dense = self.first.map_or(0, |f| self.last_end.saturating_sub(f).saturating_mul(self.width));
            dense.max(self.listed)
        }
    }
    let mut total = 0u64;
    let mut sheet = Sheet::default();
    let (mut row_rep, mut pos, mut row_w) = (1u64, 0u64, 0u64);
    let max = limits.max_cells;
    let num = |v: Option<&[u8]>, cap: u64| -> u64 {
        v.and_then(|v| std::str::from_utf8(v).ok()).and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(1).min(cap)
    };
    scan_tags(r, limits, |t| {
        match (local(t.name), t.closing) {
            (b"table", false) if !t.self_closing => {
                total = total.saturating_add(sheet.cells());
                sheet = Sheet::default();
            }
            (b"table-row", false) => {
                row_rep = num(t.attr(b"table:number-rows-repeated"), ODS_MAX_ROWS);
                pos = 0;
                row_w = 0;
                if t.self_closing {
                    sheet.rows = (sheet.rows + row_rep).min(ODS_MAX_ROWS);
                }
            }
            (b"table-cell" | b"covered-table-cell", false) => {
                let rep = num(t.attr(b"table:number-columns-repeated"), ODS_MAX_COLUMNS);
                pos = (pos + rep).min(ODS_MAX_COLUMNS);
                let filled = !t.self_closing || t.attr(b"office:value-type").is_some() || t.attr(b"table:formula").is_some();
                if filled {
                    row_w = pos;
                }
            }
            (b"table-row", true) => {
                let start = sheet.rows;
                sheet.rows = (sheet.rows + row_rep).min(ODS_MAX_ROWS);
                if row_w > 0 {
                    sheet.listed = sheet.listed.saturating_add(row_w);
                    sheet.width = sheet.width.max(row_w);
                    sheet.first.get_or_insert(start);
                    sheet.last_end = sheet.rows;
                }
            }
            _ => {}
        }
        total.saturating_add(sheet.cells()) <= max
    })?;
    Ok(total.saturating_add(sheet.cells()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ods_content(body: &str) -> String {
        format!("<office:document-content><office:body><office:spreadsheet><table:table table:name=\"S\">{body}</table:table></office:spreadsheet></office:body></office:document-content>")
    }

    #[test]
    fn ods_cell_estimate() {
        let l = Limits::default();
        let one = ods_content("<table:table-row><table:table-cell office:value-type=\"float\" office:value=\"1\"/></table:table-row>");
        assert_eq!(ods_cells(one.as_bytes(), &l).unwrap(), 1);
        // Trailing empty cells and rows (common in real files) cost nothing.
        let trailing = ods_content(
            "<table:table-row><table:table-cell office:value-type=\"string\"><text:p>a</text:p></table:table-cell><table:table-cell table:number-columns-repeated=\"16000\"/></table:table-row>\
             <table:table-row table:number-rows-repeated=\"1048000\"><table:table-cell table:number-columns-repeated=\"16384\"/></table:table-row>",
        );
        assert_eq!(ods_cells(trailing.as_bytes(), &l).unwrap(), 1);
        // A full-width row repeated a million times: about 17 billion cells.
        let bomb = ods_content(
            "<table:table-row table:number-rows-repeated=\"1048576\"><table:table-cell table:number-columns-repeated=\"16384\" office:value-type=\"float\" office:value=\"1\"/></table:table-row>",
        );
        assert!(ods_cells(bomb.as_bytes(), &l).unwrap() > l.max_cells);
        // Two cells far apart: the dense rectangle between them.
        let corners = ods_content(
            "<table:table-row><table:table-cell office:value-type=\"float\" office:value=\"1\"/></table:table-row>\
             <table:table-row table:number-rows-repeated=\"100000\"><table:table-cell/></table:table-row>\
             <table:table-row><table:table-cell table:number-columns-repeated=\"999\"/><table:table-cell office:value-type=\"float\" office:value=\"1\"/></table:table-row>",
        );
        assert_eq!(ods_cells(corners.as_bytes(), &l).unwrap(), 100_002 * 1000);
    }

    #[test]
    fn shared_string_count_streams_across_chunks() {
        let mut xml = String::from("<sst>");
        for i in 0..300_000 {
            xml.push_str(&format!("<si><t>{i}</t></si>"));
        }
        xml.push_str("</sst>");
        let l = Limits { max_shared_strings: 1_000_000, ..Default::default() };
        assert_eq!(count_shared_strings(xml.as_bytes(), &l).unwrap(), 300_000);
        let l = Limits { max_shared_strings: 1000, ..Default::default() };
        assert_eq!(count_shared_strings(xml.as_bytes(), &l).unwrap(), 1001);
    }

    #[test]
    fn numbers_with_commas() {
        assert_eq!(thousands(20_000_000), "20,000,000");
        assert_eq!(thousands(999), "999");
    }
}
