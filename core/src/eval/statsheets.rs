//! StatSheets loader (`testdata/irregular/statsheets/dataset`, Krekel et al. 2026): 737 sheets from
//! 14 statistics publishers, 6 languages, labelled HEADER / DATA / TITLE / OTHER on a *compressed*
//! grid (every run of ≥ 3 empty rows or columns shortened to 2), stored as NumPy `.npz` files.
//!
//! The compressed grid is mapped back to sheet coordinates with the empty rows and columns of the
//! sheet as our reader sees it. Because two readers can disagree about what is empty, every sheet's
//! alignment is checked: the share of labelled (non-EMPTY) cells that land on a non-empty cell.
//! Sheets below [`MIN_ALIGNMENT`] are skipped and counted in the corpus notes. A CSV file is one
//! sheet; the manifest's `sheet_name` for it is an id, not the file stem, so it is found by index.
//!
//! Roles: HEADER → header (includes group/section headers in this corpus), DATA → data, TITLE →
//! title, OTHER → other (footnotes, sources). Row labels (HEADER cells with DATA to their right)
//! become data, hedgehog's and DECO's convention; the count is in the corpus notes. Score it in
//! the coarse role space ([`Corpus::coarse_roles`]).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use super::gt::{Corpus, GtCell, GtFile, GtSheet, LabelCount};
use super::EvalError;
use crate::analysis::Role;
use crate::model::{Rect, Sheet, Value};

/// Minimum share of labelled cells that must fall on non-empty cells after decompression.
pub const MIN_ALIGNMENT: f64 = 0.95;

/// A parsed `.npy` array: element type, shape, raw little-endian data.
struct Npy {
    descr: String,
    shape: Vec<usize>,
    data: Vec<u8>,
}

fn parse_npy(b: Vec<u8>) -> Result<Npy, String> {
    if b.len() < 10 || &b[..6] != b"\x93NUMPY" {
        return Err("not a .npy file".into());
    }
    let (hlen, start) = if b[6] == 1 {
        (u16::from_le_bytes([b[8], b[9]]) as usize, 10)
    } else {
        (u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize, 12)
    };
    let header = std::str::from_utf8(b.get(start..start + hlen).ok_or("short header")?).map_err(|e| e.to_string())?;
    if header.contains("'fortran_order': True") {
        return Err("fortran-order arrays are not supported".into());
    }
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|s| s.split('\'').nth(1))
        .ok_or("no descr")?
        .to_string();
    let shape_text = header.split("'shape':").nth(1).and_then(|s| s.split(['(', ')']).nth(1)).ok_or("no shape")?;
    let shape = shape_text
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<usize>().map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Npy { descr, shape, data: b[start + hlen..].to_vec() })
}

impl Npy {
    fn len(&self) -> usize {
        self.shape.iter().product()
    }

    fn ints(&self) -> Result<Vec<i64>, String> {
        let size: usize = self.descr[2..].parse().map_err(|_| format!("dtype {}", self.descr))?;
        let signed = self.descr.as_bytes()[1] == b'i';
        if !matches!(self.descr.as_bytes()[1], b'i' | b'u') || ![1, 2, 4, 8].contains(&size) {
            return Err(format!("unsupported dtype {}", self.descr));
        }
        let need = self.len() * size;
        let data = self.data.get(..need).ok_or("short data")?;
        Ok(data
            .chunks_exact(size)
            .map(|c| {
                let mut buf = [0u8; 8];
                buf[..size].copy_from_slice(c);
                let v = u64::from_le_bytes(buf);
                if signed && size < 8 && c[size - 1] & 0x80 != 0 {
                    (v | (!0u64 << (size * 8))) as i64
                } else {
                    v as i64
                }
            })
            .collect())
    }

    fn strings(&self) -> Result<Vec<String>, String> {
        let kind = self.descr.as_bytes()[1];
        let n: usize = self.descr[2..].parse().map_err(|_| format!("dtype {}", self.descr))?;
        let width = match kind {
            b'S' => n,
            b'U' => 4 * n,
            _ => return Err(format!("unsupported dtype {}", self.descr)),
        };
        let data = self.data.get(..self.len() * width).ok_or("short data")?;
        Ok(data
            .chunks_exact(width.max(1))
            .map(|c| {
                if kind == b'S' {
                    String::from_utf8_lossy(c).trim_end_matches('\0').to_string()
                } else {
                    c.chunks_exact(4)
                        .map(|u| u32::from_le_bytes([u[0], u[1], u[2], u[3]]))
                        .take_while(|&u| u != 0)
                        .filter_map(char::from_u32)
                        .collect()
                }
            })
            .collect())
    }
}

fn read_npz(path: &Path) -> Result<BTreeMap<String, Npy>, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    let mut out = BTreeMap::new();
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| e.to_string())?;
        let name = e.name().map_err(|e| e.to_string())?.trim_end_matches(".npy").to_string();
        let mut b = Vec::new();
        e.read_to_end(&mut b).map_err(|e| e.to_string())?;
        out.insert(name, parse_npy(b)?);
    }
    Ok(out)
}

/// `B10:E83` → Rect (0-based).
fn parse_range(s: &str) -> Option<Rect> {
    let cell = |c: &str| -> Option<(u32, u32)> {
        let letters: String = c.chars().take_while(|x| x.is_ascii_alphabetic()).collect();
        let digits = &c[letters.len()..];
        let col = letters.bytes().try_fold(0u32, |a, b| Some(a * 26 + (b.to_ascii_uppercase() - b'A' + 1) as u32))?;
        let row: u32 = digits.parse().ok()?;
        (col > 0 && row > 0).then(|| (row - 1, col - 1))
    };
    let (a, b) = s.split_once(':').unwrap_or((s, s));
    let ((r0, c0), (r1, c1)) = (cell(a)?, cell(b)?);
    Some(Rect { top: r0.min(r1), left: c0.min(c1), bottom: r0.max(r1), right: c0.max(c1) })
}

fn filled(v: &Option<Value>) -> bool {
    match v {
        None => false,
        Some(Value::Text(t)) => !t.trim().is_empty(),
        _ => true,
    }
}

/// Harmonizes StatSheets with hedgehog's (and DECO's) convention: a HEADER cell with DATA cells to
/// its right in the same row is a row label (stub), which hedgehog keeps as a data column. Column
/// headers have no DATA on their row and stay headers.
fn row_labels_as_data(labels: &mut [i64], rows: usize, cols: usize, changed: &mut u64) {
    for i in 0..rows {
        let row = &mut labels[i * cols..(i + 1) * cols];
        let Some(last_data) = row.iter().rposition(|&v| v == 2) else { continue };
        for v in &mut row[..last_data] {
            if *v == 1 {
                *v = 2;
                *changed += 1;
            }
        }
    }
}

/// Compressed index → original index along one axis: runs of ≥ 3 empty lines keep their first 2.
fn decompress(nonempty: &[bool]) -> Vec<u32> {
    let mut map = Vec::new();
    let mut i = 0;
    while i < nonempty.len() {
        if nonempty[i] {
            map.push(i as u32);
            i += 1;
            continue;
        }
        let mut j = i;
        while j < nonempty.len() && !nonempty[j] {
            j += 1;
        }
        let run = j - i;
        let keep = if run >= 3 { 2 } else { run };
        map.extend((i..i + keep).map(|x| x as u32));
        i = j;
    }
    map
}

/// Labelled cells of one sheet in sheet coordinates, and the alignment of the best mapping.
fn map_labels(sheet: &Sheet, labels: &[i64], rows: usize, cols: usize, anchor: (i64, i64)) -> (Vec<GtCell>, f64) {
    let (mut max_r, mut max_c) = (0u32, 0u32);
    for c in &sheet.cells {
        if filled(&c.value) {
            max_r = max_r.max(c.row);
            max_c = max_c.max(c.col);
        }
    }
    let mut row_ne = vec![false; max_r as usize + 1];
    let mut col_ne = vec![false; max_c as usize + 1];
    let mut cells = std::collections::HashSet::new();
    for c in &sheet.cells {
        if filled(&c.value) {
            row_ne[c.row as usize] = true;
            col_ne[c.col as usize] = true;
            cells.insert((c.row, c.col));
        }
    }
    // A merged area is labelled on every cell it covers; its value sits at the top-left.
    for m in &sheet.merges {
        let area = (m.bottom - m.top + 1) as u64 * (m.right - m.left + 1) as u64;
        if area > 10_000 || !cells.contains(&(m.top, m.left)) {
            continue;
        }
        for r in m.top..=m.bottom.min(max_r) {
            for c in m.left..=m.right.min(max_c) {
                cells.insert((r, c));
            }
        }
    }
    let rmap = decompress(&row_ne);
    let cmap = decompress(&col_ne);
    // Past the last filled line the grids continue one to one.
    let orig = |map: &[u32], k: i64| -> Option<u32> {
        if k < 0 {
            return None;
        }
        let k = k as usize;
        Some(match map.get(k) {
            Some(&v) => v,
            None => map.last().map_or(0, |&l| l + 1) + (k - map.len()) as u32,
        })
    };
    let mut best: (Vec<GtCell>, f64) = (Vec::new(), -1.0);
    let variants = [anchor, (0, 0), (-anchor.0, -anchor.1)];
    for (k, &(ar, ac)) in variants.iter().enumerate() {
        if k > 0 && variants[..k].contains(&(ar, ac)) {
            continue;
        }
        let mut out = Vec::new();
        let (mut on, mut total) = (0usize, 0usize);
        for i in 0..rows {
            for j in 0..cols {
                let role = match labels[i * cols + j] {
                    1 => Role::Header,
                    2 => Role::Data,
                    3 => Role::Title,
                    4 => Role::Other,
                    _ => continue,
                };
                total += 1;
                let (Some(r), Some(c)) = (orig(&rmap, i as i64 + ar), orig(&cmap, j as i64 + ac)) else { continue };
                on += cells.contains(&(r, c)) as usize;
                out.push(GtCell { row: r, col: c, role });
            }
        }
        let align = if total == 0 { 1.0 } else { on as f64 / total as f64 };
        if align > best.1 {
            best = (out, align);
        }
    }
    best
}

/// A manifest row: sheet name, labels file, whether the file is a CSV.
type SheetEntry = (String, String, bool);

/// Loads StatSheets from `<data_root>/irregular/statsheets/dataset`. Opens every workbook once to
/// map the compressed label grids (about a minute).
pub fn load(data_root: &Path) -> Result<Corpus, EvalError> {
    let base = data_root.join("irregular").join("statsheets").join("dataset");
    let manifest = base.join("manifest.csv");
    let text = std::fs::read_to_string(&manifest).map_err(|e| EvalError(format!("{}: {e}", manifest.display())))?;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut reader = super::csv::CsvReader::new(text.as_bytes());
    let mut rec = Vec::new();
    while reader.next_record(&mut rec).map_err(|e| EvalError(format!("{}: {e}", manifest.display())))? {
        rows.push(rec.clone());
    }
    let header = rows.first().cloned().unwrap_or_default();
    let col = |name: &str| header.iter().position(|h| h == name).ok_or(EvalError(format!("manifest: no `{name}` column")));
    let (c_src, c_file, c_sheet, c_mime, c_labels) =
        (col("source")?, col("file_path")?, col("sheet_name")?, col("mime_type")?, col("labels_path")?);

    let mut by_file: BTreeMap<String, (String, Vec<SheetEntry>)> = BTreeMap::new();
    for r in rows.iter().skip(1) {
        if r.len() <= c_labels.max(c_mime) {
            continue;
        }
        by_file
            .entry(r[c_file].clone())
            .or_insert_with(|| (r[c_src].clone(), Vec::new()))
            .1
            .push((r[c_sheet].clone(), r[c_labels].clone(), r[c_mime].contains("csv")));
    }

    let mut corpus = Corpus { name: "statsheets".into(), has_tables: true, has_roles: true, coarse_roles: true, ..Corpus::default() };
    let (mut misaligned, mut unreadable, mut align_sum, mut aligned) = (0usize, 0usize, 0.0f64, 0usize);
    let (mut stubs, mut csv_aligned, mut csv_total) = (0u64, 0usize, 0usize);
    let mut counts: BTreeMap<&'static str, u64> = BTreeMap::new();
    for (file, (source, sheets)) in by_file {
        let path = base.join(&file);
        let Ok(wb) = super::open(&path) else {
            unreadable += sheets.len();
            continue;
        };
        let mut gts = Vec::new();
        for (name, labels_path, csv) in sheets {
            csv_total += csv as usize;
            let index = csv.then_some(0);
            let Some((si, _)) = super::run::find_sheet(&wb, &name, index) else {
                unreadable += 1;
                continue;
            };
            let Ok(npz) = read_npz(&base.join(&labels_path)) else {
                unreadable += 1;
                continue;
            };
            let (Some(l), Some(a)) = (npz.get("labels"), npz.get("anchor_labels")) else {
                unreadable += 1;
                continue;
            };
            let (Ok(lv), Ok(av)) = (l.ints(), a.ints()) else {
                unreadable += 1;
                continue;
            };
            let (rows, cols) = (l.shape.first().copied().unwrap_or(0), l.shape.get(1).copied().unwrap_or(0));
            let mut lv = lv;
            row_labels_as_data(&mut lv, rows, cols, &mut stubs);
            let anchor = (av.first().copied().unwrap_or(0), av.get(1).copied().unwrap_or(0));
            let (mut cells, align) = map_labels(&wb.sheets[si], &lv, rows, cols, anchor);
            if align < MIN_ALIGNMENT {
                misaligned += 1;
                continue;
            }
            align_sum += align;
            aligned += 1;
            csv_aligned += csv as usize;
            cells.sort_by_key(|c| (c.row, c.col));
            cells.dedup_by_key(|c| (c.row, c.col));
            for c in &cells {
                *counts.entry(c.role.as_str()).or_default() += 1;
            }
            let tables = npz
                .get("ranges_original")
                .and_then(|r| r.strings().ok())
                .unwrap_or_default()
                .iter()
                .filter_map(|s| parse_range(s))
                .collect();
            gts.push(GtSheet { name, index, tables, cells });
        }
        if !gts.is_empty() {
            corpus.files.push(GtFile { id: format!("{source}/{file}"), path, fallback_paths: Vec::new(), sheets: gts });
        }
    }
    for (k, v) in counts {
        corpus.labels.insert(k.to_uppercase(), LabelCount { role: super::labelled::role_named(k), count: v });
    }
    corpus.notes.push(format!(
        "{aligned} sheets aligned ({csv_aligned} of {csv_total} CSV; mean {:.1}% of labelled cells on filled cells); \
         skipped: {misaligned} misaligned (< {:.0}%), {unreadable} unreadable; {stubs} row-label HEADER cells scored as data",
        100.0 * align_sum / aligned.max(1) as f64,
        100.0 * MIN_ALIGNMENT
    ));
    Ok(corpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompress_keeps_two_of_long_runs() {
        // filled, 5 empty, filled, 2 empty, filled
        let ne = [true, false, false, false, false, false, true, false, false, true];
        assert_eq!(decompress(&ne), vec![0, 1, 2, 6, 7, 8, 9]);
    }

    #[test]
    fn ranges_and_npy() {
        assert_eq!(parse_range("B10:E83"), Some(Rect { top: 9, left: 1, bottom: 82, right: 4 }));
        assert_eq!(parse_range("AA1"), Some(Rect { top: 0, left: 26, bottom: 0, right: 26 }));
        let mut b = b"\x93NUMPY\x01\x00".to_vec();
        let h = "{'descr': '<i4', 'fortran_order': False, 'shape': (2, 2), }";
        let pad = 64 - (10 + h.len() + 1) % 64;
        let header = format!("{h}{}\n", " ".repeat(pad));
        b.extend((header.len() as u16).to_le_bytes());
        b.extend(header.as_bytes());
        for v in [1i32, -2, 3, 4] {
            b.extend(v.to_le_bytes());
        }
        let a = parse_npy(b).unwrap();
        assert_eq!((a.shape.clone(), a.ints().unwrap()), (vec![2, 2], vec![1, -2, 3, 4]));
    }
}
