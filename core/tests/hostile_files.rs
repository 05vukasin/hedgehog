//! Regression tests for hostile and broken files (found by the fuzzer, `fuzz/`): each must give a
//! clear error or a normal result, never a panic, an abort or a runaway allocation. The inputs
//! are built here, byte by byte where needed; no spreadsheet files are checked in.

use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use hh_core::reader::{open_with, Limits, ReadOptions};
use hh_core::model::Value;

fn tmp(name: &str) -> PathBuf {
    // One folder per call: tests run in parallel and several use the same file name.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("hh_hostile_{}_{n}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

/// A small valid xlsx (two sheets, shared strings, a merge).
fn base_xlsx() -> Vec<u8> {
    let mut wb = rust_xlsxwriter::Workbook::new();
    let ws = wb.add_worksheet();
    ws.write(0, 0, "Region").unwrap();
    ws.write(0, 1, "Amount").unwrap();
    ws.write(1, 0, "North").unwrap();
    ws.write(1, 1, 10.0).unwrap();
    ws.write(2, 0, "South").unwrap();
    ws.write(2, 1, 20.0).unwrap();
    ws.merge_range(4, 0, 4, 1, "note", &rust_xlsxwriter::Format::new()).unwrap();
    wb.add_worksheet().write(0, 0, "x").unwrap();
    wb.save_to_buffer().unwrap()
}

fn parts(zip: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut z = zip::ZipArchive::new(Cursor::new(zip)).unwrap();
    (0..z.len())
        .map(|i| {
            let mut f = z.by_index(i).unwrap();
            let mut v = Vec::new();
            f.read_to_end(&mut v).unwrap();
            (f.name().unwrap().to_string(), v)
        })
        .collect()
}

fn zip_of(parts: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (n, d) in parts {
        w.start_file(n.as_str(), o).unwrap();
        w.write_all(d).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// `base_xlsx` with `edit` applied to the text of part `name`.
fn xlsx_with(name: &str, edit: impl Fn(&str) -> String) -> Vec<u8> {
    let mut p = parts(&base_xlsx());
    let part = p.iter_mut().find(|(n, _)| n == name).unwrap_or_else(|| panic!("no part {name}"));
    let text = String::from_utf8(part.1.clone()).unwrap();
    let new = edit(&text);
    assert_ne!(new, text, "edit of {name} changed nothing");
    part.1 = new.into_bytes();
    zip_of(&p)
}

fn open_bytes(name: &str, bytes: &[u8], opts: ReadOptions) -> Result<hh_core::Workbook, hh_core::Error> {
    let p = tmp(name);
    std::fs::write(&p, bytes).unwrap();
    let r = open_with(&p, opts);
    std::fs::remove_file(&p).ok();
    if let Some(d) = p.parent() {
        std::fs::remove_dir(d).ok();
    }
    r
}

fn open_default(name: &str, bytes: &[u8]) -> Result<hh_core::Workbook, hh_core::Error> {
    open_bytes(name, bytes, ReadOptions::default())
}

#[test]
fn shared_string_count_is_not_trusted() {
    // uniqueCount="2147483647" made calamine reserve 51 GB and abort.
    let b = xlsx_with("xl/sharedStrings.xml", |s| {
        let i = s.find("uniqueCount=\"").unwrap() + 13;
        let j = i + s[i..].find('"').unwrap();
        format!("{}2147483647{}", &s[..i], &s[j..])
    });
    let wb = open_default("ss.xlsx", &b).unwrap();
    assert_eq!(wb.sheets[0].cells.len(), 7);
}

#[test]
fn overlong_cell_references_are_errors_not_wraparounds() {
    let b = xlsx_with("xl/worksheets/sheet1.xml", |s| s.replacen("r=\"B2\"", "r=\"B99999999999\"", 1));
    let e = open_default("ref.xlsx", &b).unwrap_err();
    assert!(e.to_string().contains("overflow"), "{e}");
    assert!(!e.to_string().contains('\n'));
    let b = xlsx_with("xl/worksheets/sheet1.xml", |s| s.replacen("r=\"B2\"", "r=\"ZZZZZZZZB2\"", 1));
    assert!(open_default("col.xlsx", &b).is_err());
}

#[test]
fn inverted_dimension_and_merge() {
    let b = xlsx_with("xl/worksheets/sheet1.xml", |s| {
        let s = s.replacen("<dimension ref=\"A1:B5\"/>", "<dimension ref=\"B5:A1\"/>", 1);
        s.replacen("ref=\"A5:B5\"", "ref=\"B5:A5\"", 1)
    });
    let wb = open_default("inv.xlsx", &b).unwrap();
    let m = wb.sheets[0].merges[0];
    assert_eq!((m.top, m.left, m.bottom, m.right), (4, 0, 4, 1));
    hh_core::normalize_workbook(&wb, &Default::default());
}

#[test]
fn sheet_wide_merge_and_far_cells_stay_fast() {
    // Cells at A1 and XFD1048576 and a merge over the whole sheet: work must follow the cells,
    // not the area (a table over the merge had 16k equal header names, made unique in O(n²)).
    let b = xlsx_with("xl/worksheets/sheet1.xml", |s| {
        let s = s.replacen("ref=\"A5:B5\"", "ref=\"C1:XFD1048576\"", 1);
        s.replacen("</sheetData>", "<row r=\"1048576\"><c r=\"XFD1048576\"><v>1</v></c></row></sheetData>", 1)
    });
    let t = Instant::now();
    let wb = open_default("wide.xlsx", &b).unwrap();
    assert_eq!(wb.sheets[0].cells.len(), 8);
    hh_core::normalize_workbook(&wb, &Default::default());
    assert!(t.elapsed() < Duration::from_secs(60), "{:?}", t.elapsed());
}

#[test]
fn many_equal_headers_are_renamed_in_linear_time() {
    use hh_core::model::{Cell, Sheet, Value, Workbook};
    let n = 20_000u32;
    let mut cells = Vec::new();
    for row in 0..4u32 {
        for col in 0..n {
            let value = if row == 0 { Value::Text("Amount".into()) } else { Value::Number((row * col) as f64) };
            cells.push(Cell { row, col, value: Some(value), formula: None });
        }
    }
    let wb = Workbook { sheets: vec![Sheet { name: "S".into(), cells, merges: vec![], formats: vec![] }] };
    let t = Instant::now();
    let norm = hh_core::normalize_workbook(&wb, &hh_core::NormalizeOptions { rules_only: true, ..Default::default() });
    let names: std::collections::HashSet<&str> = norm.tables[0].columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names.len(), norm.tables[0].columns.len());
    assert!(t.elapsed() < Duration::from_secs(60), "{:?}", t.elapsed());
}

// -- limits ----------------------------------------------------------------------------------

#[test]
fn limits_refuse_with_clear_errors() {
    let b = base_xlsx();
    let with = |l: Limits| ReadOptions { limits: l, ..Default::default() };
    let e = open_bytes("z.xlsx", &b, with(Limits { max_decompressed_bytes: 1000, ..Default::default() })).unwrap_err();
    assert!(e.is_limit() && e.to_string().contains("decompresses to"), "{e}");
    let e = open_bytes("c.xlsx", &b, with(Limits { max_cells: 3, ..Default::default() })).unwrap_err();
    assert!(e.is_limit() && e.to_string().contains("more than 3 cells"), "{e}");
    let e = open_bytes("s.xlsx", &b, with(Limits { max_shared_strings: 1, ..Default::default() })).unwrap_err();
    assert!(e.is_limit() && e.to_string().contains("shared strings"), "{e}");
    let past = Instant::now().checked_sub(Duration::from_secs(1));
    let e = open_bytes("t.xlsx", &b, with(Limits { deadline: past, ..Default::default() })).unwrap_err();
    assert!(e.is_limit() && e.to_string().contains("time limit"), "{e}");
    // CSV counts cells too.
    let p = tmp("c.csv");
    std::fs::write(&p, "a,b\n1,2\n3,4\n").unwrap();
    let e = open_with(&p, with(Limits { max_cells: 4, ..Default::default() })).unwrap_err();
    assert!(e.is_limit(), "{e}");
    assert!(open_with(&p, ReadOptions::default()).is_ok());
    std::fs::remove_file(&p).ok();
    // Normalizing stops at the deadline too.
    let wb = open_default("n.xlsx", &b).unwrap();
    let opts = hh_core::NormalizeOptions { deadline: past, ..Default::default() };
    assert!(hh_core::try_normalize_workbook(&wb, &opts).is_err());
    assert!(hh_core::try_normalize_workbook(&wb, &Default::default()).is_ok());
}

fn ods(content: &str, manifest_extra: &str) -> Vec<u8> {
    let manifest = format!(
        "<?xml version=\"1.0\"?><manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\">\
         <manifest:file-entry manifest:full-path=\"/\" manifest:media-type=\"application/vnd.oasis.opendocument.spreadsheet\"/>\
         <manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\">{manifest_extra}</manifest:file-entry></manifest:manifest>"
    );
    let content = format!(
        "<?xml version=\"1.0\"?><office:document-content xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
         xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\">\
         <office:body><office:spreadsheet><table:table table:name=\"S\">{content}</table:table></office:spreadsheet></office:body></office:document-content>"
    );
    zip_of(&[
        ("mimetype".into(), b"application/vnd.oasis.opendocument.spreadsheet".to_vec()),
        ("META-INF/manifest.xml".into(), manifest.into_bytes()),
        ("content.xml".into(), content.into_bytes()),
    ])
}

#[test]
fn ods_repeat_bomb_is_refused_before_parsing() {
    let cell = "<table:table-cell office:value-type=\"float\" office:value=\"1\"><text:p>1</text:p></table:table-cell>";
    let ok = ods(&format!("<table:table-row>{cell}{cell}</table:table-row>"), "");
    let wb = open_default("ok.ods", &ok).unwrap();
    assert_eq!(wb.sheets[0].cells.len(), 2);
    // A few hundred bytes asking for 16384 × 1,048,576 cells.
    let bomb = ods(
        "<table:table-row table:number-rows-repeated=\"1048576\"><table:table-cell table:number-columns-repeated=\"16384\" office:value-type=\"float\" office:value=\"1\"/></table:table-row>",
        "",
    );
    let t = Instant::now();
    let e = open_default("bomb.ods", &bomb).unwrap_err();
    assert!(e.is_limit() && e.to_string().contains("cells"), "{e}");
    assert!(t.elapsed() < Duration::from_secs(10));
    let enc = ods(&format!("<table:table-row>{cell}</table:table-row>"), "<manifest:encryption-data manifest:checksum-type=\"SHA1\"/>");
    let e = open_default("enc.ods", &enc).unwrap_err();
    assert!(e.to_string().contains("password-protected"), "{e}");
}

#[test]
fn ods_with_backslash_entry_names_opens() {
    // B52: some Windows tools write `META-INF\manifest.xml` (uk_gov RTS accompanying tables).
    let cell = "<table:table-cell office:value-type=\"float\" office:value=\"1\"><text:p>1</text:p></table:table-cell>";
    let backslash = |bytes: Vec<u8>| {
        let p: Vec<(String, Vec<u8>)> = parts(&bytes).into_iter().map(|(n, d)| (n.replace('/', "\\"), d)).collect();
        assert!(p.iter().any(|(n, _)| n == "META-INF\\manifest.xml"));
        zip_of(&p)
    };
    let ok = backslash(ods(&format!("<table:table-row>{cell}{cell}</table:table-row>"), ""));
    let wb = open_default("ok.ods", &ok).unwrap();
    assert_eq!(wb.sheets[0].cells.len(), 2);
    // The password check still reads the manifest.
    let enc = backslash(ods(&format!("<table:table-row>{cell}</table:table-row>"), "<manifest:encryption-data manifest:checksum-type=\"SHA1\"/>"));
    let e = open_default("enc.ods", &enc).unwrap_err();
    assert!(e.to_string().contains("password-protected"), "{e}");
}

// -- OLE / xls ---------------------------------------------------------------------------------

const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const FREESECT: u32 = 0xFFFF_FFFF;

/// A version-3 compound file: sector 0 the FAT, sector 1 the directory (root + one stream named
/// `name`), the stream from sector 2 on (≥ 4096 bytes, so in regular sectors). `fat_edit` may
/// break the FAT.
fn cfb(name: &str, stream: &[u8], fat_edit: impl Fn(&mut Vec<u32>)) -> Vec<u8> {
    assert!(stream.len() >= 4096 && stream.len().is_multiple_of(512));
    let n = stream.len() / 512;
    let mut h = vec![0u8; 512];
    h[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    h[24..26].copy_from_slice(&0x3Eu16.to_le_bytes());
    h[26..28].copy_from_slice(&3u16.to_le_bytes());
    h[28..30].copy_from_slice(&0xFFFEu16.to_le_bytes());
    h[30..32].copy_from_slice(&9u16.to_le_bytes());
    h[32..34].copy_from_slice(&6u16.to_le_bytes());
    h[44..48].copy_from_slice(&1u32.to_le_bytes());
    h[48..52].copy_from_slice(&1u32.to_le_bytes());
    h[56..60].copy_from_slice(&4096u32.to_le_bytes());
    h[60..64].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
    h[68..72].copy_from_slice(&ENDOFCHAIN.to_le_bytes());
    for i in 0..109 {
        let v = if i == 0 { 0 } else { FREESECT };
        h[76 + 4 * i..80 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    let mut fat = vec![FREESECT; 128];
    fat[0] = 0xFFFF_FFFD;
    fat[1] = ENDOFCHAIN;
    for i in 0..n {
        fat[2 + i] = if i + 1 == n { ENDOFCHAIN } else { (3 + i) as u32 };
    }
    fat_edit(&mut fat);
    let mut out = h;
    for v in fat {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let mut dir = vec![0u8; 512];
    let entry = |dir: &mut Vec<u8>, at: usize, name: &str, typ: u8, child: u32, start: u32, size: u32| {
        let e = &mut dir[at * 128..at * 128 + 128];
        let utf16: Vec<u8> = name.encode_utf16().chain([0]).flat_map(|u| u.to_le_bytes()).collect();
        e[..utf16.len()].copy_from_slice(&utf16);
        e[64..66].copy_from_slice(&(utf16.len() as u16).to_le_bytes());
        e[66] = typ;
        e[67] = 1;
        e[68..72].copy_from_slice(&FREESECT.to_le_bytes());
        e[72..76].copy_from_slice(&FREESECT.to_le_bytes());
        e[76..80].copy_from_slice(&child.to_le_bytes());
        e[116..120].copy_from_slice(&start.to_le_bytes());
        e[120..124].copy_from_slice(&size.to_le_bytes());
    };
    entry(&mut dir, 0, "Root Entry", 5, 1, ENDOFCHAIN, 0);
    entry(&mut dir, 1, name, 2, FREESECT, 2, stream.len() as u32);
    out.extend_from_slice(&dir);
    out.extend_from_slice(stream);
    out
}

fn record(typ: u16, data: &[u8]) -> Vec<u8> {
    let mut r = typ.to_le_bytes().to_vec();
    r.extend_from_slice(&(data.len() as u16).to_le_bytes());
    r.extend_from_slice(data);
    r
}

fn bof(dt: u16) -> Vec<u8> {
    let mut d = vec![0u8; 16];
    d[..2].copy_from_slice(&0x0600u16.to_le_bytes());
    d[2..4].copy_from_slice(&dt.to_le_bytes());
    record(0x0809, &d)
}

/// A BIFF8 workbook stream with one sheet holding a number at A1 and a FORMULA record at B1
/// whose token array is `rgce` (cce and tokens).
fn biff_with_formula(rgce: &[u8]) -> Vec<u8> {
    biff_with(&[], rgce)
}

/// [`biff_with_formula`] with extra records in the workbook globals.
fn biff_with(extra_globals: &[u8], rgce: &[u8]) -> Vec<u8> {
    let mut globals = bof(0x0005);
    globals.extend_from_slice(extra_globals);
    let sheet_name = b"S";
    let boundsheet_len = 4 + 8 + sheet_name.len();
    let eof = record(0x000A, &[]);
    let sheet_pos = globals.len() + boundsheet_len + eof.len();
    let mut bs = (sheet_pos as u32).to_le_bytes().to_vec();
    bs.extend_from_slice(&[0, 0, sheet_name.len() as u8, 0]);
    bs.extend_from_slice(sheet_name);
    globals.extend(record(0x0085, &bs));
    globals.extend(&eof);
    assert_eq!(globals.len(), sheet_pos);
    let mut s = globals;
    s.extend(bof(0x0010));
    let mut num = vec![0u8; 6];
    num.extend_from_slice(&5.0f64.to_le_bytes());
    s.extend(record(0x0203, &num));
    let mut f = vec![0, 0, 1, 0, 0, 0];
    f.extend_from_slice(&1.0f64.to_le_bytes());
    f.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    f.extend_from_slice(rgce);
    s.extend(record(0x0006, &f));
    s.extend(&eof);
    s.resize(s.len().max(4096).div_ceil(512) * 512, 0);
    s
}

#[test]
fn xls_formula_reading_past_its_record_is_an_unreadable_formula() {
    // cce = 200 but two token bytes: slicing rgce[2..202] panicked.
    let b = cfb("Workbook", &biff_with_formula(&[200, 0, 0x1E, 1]), |_| {});
    let wb = open_default("f.xls", &b).unwrap();
    let cells = &wb.sheets[0].cells;
    assert_eq!(cells.len(), 2, "{cells:?}");
    assert!(cells[1].formula.as_deref().unwrap_or("").starts_with("Unrecognised formula"), "{cells:?}");
    // Truncated tokens: PtgArea3d needs 10 bytes after its id.
    let b = cfb("Workbook", &biff_with_formula(&[3, 0, 0x3B, 1, 0]), |_| {});
    assert!(open_default("g.xls", &b).is_ok());
    // A valid one still parses: PtgInt 7.
    let b = cfb("Workbook", &biff_with_formula(&[3, 0, 0x1E, 7, 0]), |_| {});
    let wb = open_default("h.xls", &b).unwrap();
    assert_eq!(wb.sheets[0].cells[1].formula.as_deref(), Some("7"));
}

#[test]
fn xls_short_records_do_not_panic() {
    // Records cut short: an empty EXTERNSHEET, a BoundSheet-like empty MERGECELLS count, a short
    // defined name (Lbl). Each used to index past its data.
    for extra in [record(0x0017, &[]), record(0x0018, &[0, 0, 0, 9]), record(0x0018, &[0; 15])] {
        let b = cfb("Workbook", &biff_with(&extra, &[3, 0, 0x1E, 7, 0]), |_| {});
        let _ = open_default("short.xls", &b);
    }
}

#[test]
fn cfb_chain_loops_and_bad_ids_are_errors() {
    let stream = biff_with_formula(&[3, 0, 0x1E, 7, 0]);
    // The directory chain points at itself: it grew until memory ran out.
    let b = cfb("Workbook", &stream, |fat| fat[1] = 1);
    let e = open_default("loop.xls", &b).unwrap_err();
    assert!(e.to_string().contains("loops"), "{e}");
    // A stream chain that loops back: read up to the stream's length, then stops.
    let b = cfb("Workbook", &stream, |fat| fat[5] = 2);
    let _ = open_default("loop2.xls", &b);
    // A sector id past the FAT: indexing it panicked.
    let b = cfb("Workbook", &stream, |fat| fat[3] = 1000);
    assert!(open_default("past.xls", &b).is_err());
    // The file ends inside the stream.
    let full = cfb("Workbook", &stream, |_| {});
    assert!(open_default("short.xls", &full[..3 * 512 + 30]).is_err());
}

#[test]
fn encrypted_ooxml_says_password_protected() {
    let b = cfb("EncryptedPackage", &vec![0u8; 4096], |_| {});
    let e = open_default("enc.xlsx", &b).unwrap_err();
    assert!(e.to_string().contains("password-protected"), "{e}");
    assert!(!e.is_limit());
}

#[test]
fn garbage_and_empty_files_are_one_line_errors() {
    for (name, bytes) in [
        ("empty.xlsx", &b""[..]),
        ("junk.xlsx", &b"PK\x03\x04junk"[..]),
        ("junk.xls", &b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1"[..]),
        ("junk.ods", &b"not a zip"[..]),
    ] {
        let e = open_default(name, bytes).unwrap_err();
        assert!(!e.to_string().is_empty() && !e.to_string().contains('\n'), "{name}: {e}");
    }
}

/// A minimal xlsx with `n` sheets, each a small table, written by hand (rust_xlsxwriter would
/// itself open one temp file per sheet).
fn many_sheets_xlsx(n: usize) -> Vec<u8> {
    const MAIN: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    const PKG: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
    let mut ct = String::from(
        "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>",
    );
    let (mut sheets, mut rels, mut parts) = (String::new(), String::new(), Vec::new());
    for i in 1..=n {
        ct.push_str(&format!(
            "<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"
        ));
        sheets.push_str(&format!("<sheet name=\"S{i}\" sheetId=\"{i}\" r:id=\"rId{i}\"/>"));
        rels.push_str(&format!("<Relationship Id=\"rId{i}\" Type=\"{REL}/worksheet\" Target=\"worksheets/sheet{i}.xml\"/>"));
        let text = |r: String, v: &str| format!("<c r=\"{r}\" t=\"inlineStr\"><is><t>{v}</t></is></c>");
        let mut rows = format!("<row r=\"1\">{}{}</row>", text("A1".into(), "City"), text("B1".into(), "Amount"));
        for (k, (city, v)) in [("North", 10), ("South", 20), ("East", 30)].iter().enumerate() {
            let r = k + 2;
            rows.push_str(&format!("<row r=\"{r}\">{}<c r=\"B{r}\"><v>{v}</v></c></row>", text(format!("A{r}"), city)));
        }
        parts.push((format!("xl/worksheets/sheet{i}.xml"), format!("<worksheet xmlns=\"{MAIN}\"><sheetData>{rows}</sheetData></worksheet>").into_bytes()));
    }
    ct.push_str("</Types>");
    parts.push(("[Content_Types].xml".into(), ct.into_bytes()));
    parts.push((
        "_rels/.rels".into(),
        format!("<Relationships xmlns=\"{PKG}\"><Relationship Id=\"rId1\" Type=\"{REL}/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>").into_bytes(),
    ));
    parts.push((
        "xl/workbook.xml".into(),
        format!("<workbook xmlns=\"{MAIN}\" xmlns:r=\"{REL}\"><sheets>{sheets}</sheets></workbook>").into_bytes(),
    ));
    parts.push(("xl/_rels/workbook.xml.rels".into(), format!("<Relationships xmlns=\"{PKG}\">{rels}</Relationships>").into_bytes()));
    zip_of(&parts)
}

#[test]
fn normalizing_many_tables_does_not_run_out_of_file_descriptors() {
    // 1,500 tables: the xlsx writer keeps a temp file open per output sheet, and past the usual
    // soft limit of 1024 open files it panicked ("Too many open files").
    let src = tmp("many.xlsx");
    std::fs::write(&src, many_sheets_xlsx(1500)).unwrap();
    let out = tmp("many_out");
    let r = hh_core::normalize_file(&src, &out, &Default::default());
    std::fs::remove_file(&src).ok();
    std::fs::remove_dir_all(&out).ok();
    let r = r.unwrap();
    assert!(r.tables >= 1500, "{}", r.tables);
}

/// A BIFF8 workbook stream: `extra_globals` after the BOF, one sheet "S" holding `cells`
/// (cell records).
fn biff_sheet(extra_globals: &[u8], cells: &[Vec<u8>]) -> Vec<u8> {
    let mut globals = bof(0x0005);
    globals.extend_from_slice(extra_globals);
    let sheet_name = b"S";
    let boundsheet_len = 4 + 8 + sheet_name.len();
    let eof = record(0x000A, &[]);
    let sheet_pos = globals.len() + boundsheet_len + eof.len();
    let mut bs = (sheet_pos as u32).to_le_bytes().to_vec();
    bs.extend_from_slice(&[0, 0, sheet_name.len() as u8, 0]);
    bs.extend_from_slice(sheet_name);
    globals.extend(record(0x0085, &bs));
    globals.extend(&eof);
    let mut s = globals;
    s.extend(bof(0x0010));
    for c in cells {
        s.extend_from_slice(c);
    }
    s.extend(&eof);
    s.resize(s.len().max(4096).div_ceil(512) * 512, 0);
    s
}

#[test]
fn xls_biff8_strings_ignore_a_cp1252_codepage() {
    // A BIFF8 file whose CODEPAGE record says 1252 (written by other tools than Excel): its
    // strings are still UTF-16, compressed to one byte per character. They were decoded as
    // cp1252 after being widened, so every character was followed by a NUL ("P\0o\0p\0…").
    let codepage = record(0x0042, &1252u16.to_le_bytes());
    let mut label = vec![0, 0, 0, 0, 0, 0]; // row 0, col 0, xf 0
    label.extend_from_slice(&[4, 0, 0, b'P', b'o', b'p', 0xe9]); // cch 4, compressed
    let mut wide = vec![1, 0, 0, 0, 0, 0];
    wide.extend_from_slice(&[2, 0, 1, b'O', 0, 0x0e, 0x04]); // cch 2, UTF-16: "OЎ"
    let b = cfb("Workbook", &biff_sheet(&codepage, &[record(0x0204, &label), record(0x0204, &wide)]), |_| {});
    let wb = open_default("cp1252.xls", &b).unwrap();
    let cells = &wb.sheets[0].cells;
    assert_eq!(cells[0].value, Some(Value::Text("Popé".into())), "{cells:?}");
    assert_eq!(cells[1].value, Some(Value::Text("OЎ".into())), "{cells:?}");
}

#[test]
fn xls_1904_dates_are_1900_serials() {
    // A 1904 workbook (DATEMODE 1) counts days from 1904-01-01: its serial 45292 is 2028-01-02,
    // which is 46754 in the 1900 system hedgehog uses. It was read as 2024-01-01, four years and
    // a day early. A time of day (serial < 1) stays a time.
    let globals = [record(0x0022, &1u16.to_le_bytes()), record(0x00E0, &[0, 0, 14, 0])].concat();
    let num = |col: u8, x: f64| {
        let mut d = vec![0, 0, col, 0, 0, 0];
        d.extend_from_slice(&x.to_le_bytes());
        record(0x0203, &d)
    };
    let b = cfb("Workbook", &biff_sheet(&globals, &[num(0, 45292.0), num(1, 0.5)]), |_| {});
    let wb = open_default("d1904.xls", &b).unwrap();
    let cells = &wb.sheets[0].cells;
    assert_eq!(cells[0].value, Some(Value::DateTime(46754.0)), "{cells:?}");
    assert_eq!(cells[1].value, Some(Value::DateTime(0.5)), "{cells:?}");
}
