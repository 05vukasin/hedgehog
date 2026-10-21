//! Cell formatting of xlsx / xlsm workbooks: `xl/styles.xml` resolved into one packed [`Format`]
//! per cell format (`cellXfs` entry), and the `s="…"` attribute of every `<c>` of a sheet, read in
//! the same streaming pass over the sheet XML that finds the merged areas (see `merges.rs`).
//!
//! calamine reads no styles. Only what helps tell titles, headers and totals apart is kept: font
//! weight, slant, underline, colour, size; a fill; borders; horizontal alignment, indent, wrapping;
//! and the kind of number format. Anything unexpected → no formats for that workbook or sheet
//! (features then say "unknown"); nothing is invented.

use std::collections::HashMap;
use std::io::Read;

use calamine::Dimensions;
use memchr::memmem;

use super::merges::{cell_ref, declared_dimension, decode, local, parse_merge_cells, read_full, tags};
use crate::model::{Align, Cell, Format, FormatSpec, NumFmt, Rect};

#[derive(Debug, Clone, Copy, Default)]
struct Font {
    bold: bool,
    italic: bool,
    underline: bool,
    color: bool,
    /// Half points.
    size: u16,
}

#[derive(Debug, Clone, Copy, Default)]
struct Border {
    top: bool,
    bottom: bool,
    left: bool,
    right: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct Xf {
    num_fmt: u32,
    font: u32,
    fill: u32,
    border: u32,
    align: Align,
    indent: u8,
    wrap: bool,
}

fn truthy(v: Option<&[u8]>) -> bool {
    !matches!(v, Some(b"0" | b"false"))
}

/// A font colour other than the default text colour (automatic, black, theme text 1).
fn colored(t: &super::merges::Tag<'_>) -> bool {
    if t.attr(b"auto").is_some_and(|v| v == b"1" || v == b"true") {
        return false;
    }
    if let Some(th) = t.attr(b"theme") {
        return th != b"1";
    }
    if let Some(ix) = t.attr(b"indexed") {
        return !matches!(ix, b"8" | b"64");
    }
    if let Some(rgb) = t.attr(b"rgb") {
        return !(rgb.len() >= 6 && rgb[rgb.len() - 6..].eq_ignore_ascii_case(b"000000"));
    }
    false
}

/// A cell fill colour that is no fill to the eye: white.
fn white(t: &super::merges::Tag<'_>) -> bool {
    t.attr(b"rgb").is_some_and(|rgb| rgb.len() >= 6 && rgb[rgb.len() - 6..].eq_ignore_ascii_case(b"FFFFFF"))
        || t.attr(b"theme") == Some(b"0")
}

/// Parses `xl/styles.xml` into the packed format of every `cellXfs` entry (index = the cells'
/// `s` attribute). `None` when the part has no `cellXfs`.
pub(super) fn parse_styles(xml: &[u8]) -> Option<Vec<Format>> {
    let mut num_fmts: HashMap<u32, NumFmt> = HashMap::new();
    let mut fonts: Vec<Font> = Vec::new();
    // (has a visible fill, solid fill whose colour is still to be checked)
    let mut fills: Vec<bool> = Vec::new();
    let mut borders: Vec<Border> = Vec::new();
    let mut xfs: Vec<Xf> = Vec::new();
    let mut section: &[u8] = b"";
    let mut saw_xfs = false;
    let mut in_pattern = false;
    for t in tags(xml) {
        let name = local(t.name);
        if section.is_empty() {
            if !t.closing && !t.self_closing {
                section = match name {
                    b"numFmts" | b"fonts" | b"fills" | b"borders" | b"cellXfs" | b"cellStyleXfs" | b"dxfs" => name,
                    _ => b"",
                };
                saw_xfs |= name == b"cellXfs";
            }
            continue;
        }
        if t.closing {
            if name == section {
                section = b"";
            } else if name == b"patternFill" {
                in_pattern = false;
            }
            continue;
        }
        match section {
            b"numFmts" if name == b"numFmt" => {
                let id = t.attr(b"numFmtId").and_then(num);
                let code = t.attr(b"formatCode").and_then(decode);
                if let (Some(id), Some(code)) = (id, code) {
                    num_fmts.insert(id, numfmt_kind(&code));
                }
            }
            b"fonts" => match name {
                b"font" => fonts.push(Font { size: 22, ..Font::default() }),
                _ => {
                    let Some(f) = fonts.last_mut() else { continue };
                    match name {
                        b"b" => f.bold = truthy(t.attr(b"val")),
                        b"i" => f.italic = truthy(t.attr(b"val")),
                        b"u" => f.underline = t.attr(b"val") != Some(b"none"),
                        b"color" => f.color = colored(&t),
                        b"sz" => {
                            if let Some(v) = t.attr(b"val").and_then(|v| std::str::from_utf8(v).ok()?.parse::<f32>().ok()) {
                                f.size = (v * 2.0).round().clamp(1.0, 511.0) as u16;
                            }
                        }
                        _ => {}
                    }
                }
            },
            b"fills" => match name {
                b"fill" => fills.push(false),
                b"patternFill" => {
                    let ty = t.attr(b"patternType").unwrap_or(b"none");
                    if let Some(f) = fills.last_mut() {
                        *f = !matches!(ty, b"none" | b"gray125");
                    }
                    in_pattern = !t.self_closing;
                }
                b"fgColor" if in_pattern => {
                    if white(&t)
                        && let Some(f) = fills.last_mut()
                    {
                        *f = false;
                    }
                }
                b"gradientFill" => {
                    if let Some(f) = fills.last_mut() {
                        *f = true;
                    }
                }
                _ => {}
            },
            b"borders" => match name {
                b"border" => borders.push(Border::default()),
                b"left" | b"right" | b"top" | b"bottom" | b"start" | b"end" => {
                    let on = t.attr(b"style").is_some_and(|s| s != b"none");
                    if let Some(b) = borders.last_mut() {
                        match name {
                            b"top" => b.top |= on,
                            b"bottom" => b.bottom |= on,
                            b"left" | b"start" => b.left |= on,
                            _ => b.right |= on,
                        }
                    }
                }
                _ => {}
            },
            b"cellXfs" => match name {
                b"xf" => {
                    let id = |k: &[u8]| t.attr(k).and_then(num).unwrap_or(0);
                    xfs.push(Xf {
                        num_fmt: id(b"numFmtId"),
                        font: id(b"fontId"),
                        fill: id(b"fillId"),
                        border: id(b"borderId"),
                        ..Xf::default()
                    });
                }
                b"alignment" => {
                    let Some(x) = xfs.last_mut() else { continue };
                    x.align = match t.attr(b"horizontal") {
                        None | Some(b"general") => Align::General,
                        Some(b"left") => Align::Left,
                        Some(b"center" | b"centerContinuous") => Align::Center,
                        Some(b"right") => Align::Right,
                        Some(_) => Align::Other,
                    };
                    x.indent = t.attr(b"indent").and_then(num).unwrap_or(0).min(15) as u8;
                    x.wrap = t.attr(b"wrapText").is_some_and(|v| v == b"1" || v == b"true");
                }
                _ => {}
            },
            _ => {}
        }
    }
    if !saw_xfs {
        return None;
    }
    let default_size = fonts.first().map_or(22, |f| f.size);
    Some(
        xfs.iter()
            .map(|x| {
                let font = fonts.get(x.font as usize).copied().unwrap_or(Font { size: default_size, ..Font::default() });
                let border = borders.get(x.border as usize).copied().unwrap_or_default();
                Format::pack(&FormatSpec {
                    bold: font.bold,
                    italic: font.italic,
                    underline: font.underline,
                    color: font.color,
                    fill: fills.get(x.fill as usize).copied().unwrap_or(false),
                    border_top: border.top,
                    border_bottom: border.bottom,
                    border_left: border.left,
                    border_right: border.right,
                    wrap: x.wrap,
                    align: x.align,
                    indent: x.indent,
                    num_fmt: num_fmts.get(&x.num_fmt).copied().unwrap_or_else(|| builtin_numfmt(x.num_fmt)),
                    size_half_pt: font.size,
                })
            })
            .collect(),
    )
}

fn num(v: &[u8]) -> Option<u32> {
    std::str::from_utf8(v).ok()?.trim().parse().ok()
}

/// Kind of a built-in number format id (ECMA-376 18.8.30, plus the East Asian date ids).
fn builtin_numfmt(id: u32) -> NumFmt {
    match id {
        1 => NumFmt::Integer,
        2 => NumFmt::Decimal,
        3 | 4 | 37..=40 | 41 | 43 => NumFmt::Thousands,
        5..=8 | 42 | 44 => NumFmt::Currency,
        9 | 10 => NumFmt::Percent,
        11 | 12 | 13 | 48 => NumFmt::Other,
        14..=22 | 27..=36 | 45..=47 | 50..=58 => NumFmt::Date,
        49 => NumFmt::Text,
        _ => NumFmt::General,
    }
}

/// Kind of a custom number format code, judged on its first (positive) section.
pub(crate) fn numfmt_kind(code: &str) -> NumFmt {
    let mut plain = String::new();
    let mut currency = false;
    let mut time = false;
    let mut chars = code.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ';' => break,
            '"' => {
                for q in chars.by_ref() {
                    if q == '"' {
                        break;
                    }
                    currency |= is_currency(q);
                }
            }
            '\\' => {
                if let Some(e) = chars.next() {
                    currency |= is_currency(e);
                }
            }
            '_' | '*' => {
                chars.next();
            }
            '[' => {
                let mut inner = String::new();
                for q in chars.by_ref() {
                    if q == ']' {
                        break;
                    }
                    inner.push(q);
                }
                if let Some(sym) = inner.strip_prefix('$') {
                    // `[$€-407]` names a currency; `[$-409]` only a locale.
                    currency |= !sym.is_empty() && !sym.starts_with('-');
                } else if !inner.is_empty() && inner.to_ascii_lowercase().trim_start_matches(['h', 'm', 's']).is_empty() {
                    time = true; // [h], [mm], [ss]
                }
            }
            _ => {
                currency |= is_currency(c);
                plain.push(c.to_ascii_lowercase());
            }
        }
    }
    let p = plain.trim();
    if p.is_empty() && !time {
        return NumFmt::General;
    }
    if p == "general" {
        return NumFmt::General;
    }
    let digits = p.contains(['0', '#', '?']);
    if p.contains('@') && !digits {
        return NumFmt::Text;
    }
    let p_wo_general = p.replace("general", "");
    if time || (!digits && p_wo_general.contains(['y', 'd', 'h', 's', 'm'])) || p_wo_general.contains(['y', 'd', 'h']) {
        return NumFmt::Date;
    }
    if p.contains('%') {
        return NumFmt::Percent;
    }
    if currency {
        return NumFmt::Currency;
    }
    if !digits {
        return if p_wo_general.trim().is_empty() { NumFmt::General } else { NumFmt::Other };
    }
    if p.contains('e') || p.contains('/') {
        return NumFmt::Other;
    }
    if p.contains(',') {
        return NumFmt::Thousands;
    }
    if p.contains('.') {
        return NumFmt::Decimal;
    }
    NumFmt::Integer
}

fn is_currency(c: char) -> bool {
    matches!(c, '$' | '€' | '£' | '¥' | '₹' | '₽' | '₩' | '₺' | '₪' | '₫' | '₱' | '¢' | '₴' | '₦')
}

/// One streaming pass over a sheet part: the merged areas (`merges.rs` semantics) and, when `xfs`
/// is given, the format of every cell of `cells` (sorted row-major, as the reader built them).
/// `None` when the part is not the one calamine read (different `<dimension>`) or unreadable; the
/// formats are `None` when a cell has no `r` attribute (positions would be guesses).
///
/// Only two patterns are searched (SIMD `memmem`): the `<c` start tags (with the root element's
/// namespace prefix, if any) and `mergeCells`; the bytes in between are not looked at.
pub(super) fn scan_sheet(
    f: &mut impl Read,
    dims: Dimensions,
    xfs: Option<&[Format]>,
    cells: &[Cell],
) -> Option<(Vec<Rect>, Option<Vec<Format>>)> {
    const CHUNK: usize = 256 * 1024;
    const TAIL: usize = 128;
    let default = xfs.map(|x| x.first().copied().unwrap_or(Format::pack(&FormatSpec::default())));
    let mut formats: Option<Vec<Format>> = default.map(|d| vec![d; cells.len()]);
    let mut buf: Vec<u8> = Vec::with_capacity(CHUNK + TAIL);
    let mut chunk = vec![0u8; CHUNK];
    let mut cell_tag: Option<Vec<u8>> = None; // `<c` or `<x:c`, known once the dimension is checked
    let merge_finder = memmem::Finder::new(b"mergeCells");
    let mut cursor = 0usize;
    let key = |c: &Cell| (c.row, c.col);
    loop {
        let n = read_full(f, &mut chunk)?;
        buf.extend_from_slice(&chunk[..n]);
        let eof = n == 0;
        if cell_tag.is_none() {
            match declared_dimension(&buf) {
                Some(d) if d.unwrap_or_default() == dims => {
                    let mut pat = b"<".to_vec();
                    if let Some(root) = tags(&buf).find(|t| !t.closing && local(t.name) == b"worksheet") {
                        pat.extend_from_slice(&root.name[..root.name.len() - b"worksheet".len()]);
                    }
                    pat.push(b'c');
                    cell_tag = Some(pat);
                }
                Some(_) => return None,
                None if eof || buf.len() >= 4 * CHUNK => return None,
                None => continue,
            }
        }
        let pat = cell_tag.as_deref().unwrap_or(b"<c");
        // The `mergeCells` start tag, if it is in the buffer: every cell comes before it.
        let mut merge_at = None;
        let mut from = 0;
        while let Some(j) = merge_finder.find(&buf[from..]).map(|j| j + from) {
            if let Some(start) = merge_tag_start(&buf, j) {
                merge_at = Some(start);
                break;
            }
            from = j + 1;
        }
        let limit = merge_at.unwrap_or(buf.len());
        let mut i = 0;
        if formats.is_some() {
            let finder = memmem::Finder::new(pat);
            let mut stalled = false; // a cell tag continues in the next chunk
            while let Some(lt) = finder.find(&buf[i..limit]).map(|j| j + i) {
                let Some(&after) = buf.get(lt + pat.len()) else {
                    (i, stalled) = (lt, true);
                    break;
                };
                if !(after.is_ascii_whitespace() || after == b'>' || after == b'/') {
                    i = lt + 1; // `<col`, `<cols`, ...
                    continue;
                }
                let Some(gt) = memchr::memchr(b'>', &buf[lt..]).map(|j| j + lt) else {
                    (i, stalled) = (lt, true);
                    break;
                };
                let (mut r, mut s) = (None, 0u32);
                if let Some(t) = tags(&buf[lt..=gt]).next() {
                    for (k, v) in t.attrs() {
                        match k {
                            b"r" => r = cell_ref(v),
                            b"s" => s = num(v).unwrap_or(0),
                            _ => {}
                        }
                    }
                }
                i = gt + 1;
                let Some(pos) = r else {
                    formats = None; // positions without `r` would be guesses
                    break;
                };
                let fmt = xfs.and_then(|x| x.get(s as usize).copied()).or(default).unwrap_or_default();
                let idx = if cells.get(cursor).is_some_and(|c| key(c) == pos) {
                    Some(cursor)
                } else if cells.get(cursor).is_none_or(|c| pos < key(c)) && (cursor == 0 || key(&cells[cursor - 1]) < pos) {
                    None // a styled empty cell between two cells
                } else {
                    cells.binary_search_by_key(&pos, key).ok()
                };
                if let (Some(k), Some(fs)) = (idx, formats.as_mut()) {
                    fs[k] = fmt;
                    cursor = k + 1;
                }
            }
            if !stalled && formats.is_some() {
                // No cell tag starts before the last `pat.len()` bytes.
                i = i.max(buf.len().saturating_sub(pat.len()));
            }
        }
        if let Some(start) = merge_at {
            let mut rest = buf.split_off(start);
            f.read_to_end(&mut rest).ok()?;
            return Some((parse_merge_cells(&rest), formats));
        }
        if eof {
            return Some((Vec::new(), formats));
        }
        // Keep the unprocessed part, at least the last TAIL bytes (a `mergeCells` match across
        // the chunk boundary needs its `<`); a cell tag seen twice gets the same format twice.
        let keep_from = if formats.is_some() { i.min(buf.len().saturating_sub(TAIL)) } else { buf.len().saturating_sub(TAIL) };
        buf.drain(..keep_from);
    }
}

/// The match at `j` is the name of a `mergeCells` start tag (`<mergeCells` or `<x:mergeCells`
/// followed by whitespace, `>` or `/`): index of its `<`.
fn merge_tag_start(buf: &[u8], j: usize) -> Option<usize> {
    let after = *buf.get(j + b"mergeCells".len())?;
    if !(after.is_ascii_whitespace() || after == b'>' || after == b'/') {
        return None;
    }
    let lt = buf[..j].iter().rposition(|&b| b == b'<')?;
    let prefix = &buf[lt + 1..j];
    let ok = prefix.is_empty()
        || (prefix.last() == Some(&b':')
            && prefix[..prefix.len() - 1].iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.'));
    ok.then_some(lt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_number_formats() {
        use NumFmt::*;
        let cases = [
            ("General", General),
            ("0", Integer),
            ("0.000", Decimal),
            ("#,##0", Thousands),
            ("_(* #,##0.00_);_(* \\(#,##0.00\\);_(* \"-\"??_);_(@_)", Thousands),
            ("\"$\"#,##0_);[Red]\\(\"$\"#,##0\\)", Currency),
            ("[$€-407] #,##0.00", Currency),
            ("#,##0.00 [$€-1]", Currency),
            ("0.0%", Percent),
            ("mmmm\\ d\\,\\ yyyy", Date),
            ("dd\\-mmm\\-yy", Date),
            ("[h]:mm:ss", Date),
            ("[$-409]mmmm d, yyyy", Date),
            ("[$-407]#,##0", Thousands),
            ("h:mm AM/PM", Date),
            ("@", Text),
            ("0.00E+00", Other),
            ("\"Fuel Cost:\"\\ 0.00", Decimal),
            ("0;\\-0;;", Integer),
            ("[Red]0.00", Decimal),
            ("#,##0.00 \\K\\č", Thousands),
        ];
        for (code, want) in cases {
            assert_eq!(numfmt_kind(code), want, "{code}");
        }
        assert_eq!(builtin_numfmt(14), Date);
        assert_eq!(builtin_numfmt(44), Currency);
    }

    #[test]
    fn styles_resolve_per_xf() {
        let xml = br#"<styleSheet><numFmts count="1"><numFmt numFmtId="164" formatCode="0.0%"/></numFmts>
            <fonts count="2"><font><sz val="11"/><color theme="1"/><name val="Calibri"/></font>
            <font><b/><i val="0"/><u/><sz val="14"/><color rgb="FFFF0000"/></font></fonts>
            <fills count="4"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill>
            <fill><patternFill patternType="solid"><fgColor rgb="FFC6E0B4"/></patternFill></fill>
            <fill><patternFill patternType="solid"><fgColor rgb="FFFFFFFF"/></patternFill></fill></fills>
            <borders count="2"><border><left/><right/><top/><bottom/></border>
            <border><left style="thin"/><right/><top style="none"/><bottom style="double"><color auto="1"/></bottom></border></borders>
            <cellStyleXfs count="1"><xf numFmtId="0" fontId="1" fillId="2" borderId="1"/></cellStyleXfs>
            <cellXfs count="3"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>
            <xf numFmtId="164" fontId="1" fillId="2" borderId="1" xfId="0"><alignment horizontal="center" indent="2" wrapText="1"/></xf>
            <xf numFmtId="3" fontId="0" fillId="3" borderId="0" xfId="0"/></cellXfs>
            <dxfs count="1"><dxf><font><b/></font><fill><patternFill patternType="solid"/></fill></dxf></dxfs></styleSheet>"#;
        let x = parse_styles(xml).unwrap();
        assert_eq!(x.len(), 3);
        let d = x[0].unpack().unwrap();
        assert!(!d.bold && !d.fill && !d.color && d.size_half_pt == 22 && d.num_fmt == NumFmt::General);
        let h = x[1].unpack().unwrap();
        assert!(h.bold && !h.italic && h.underline && h.color && h.fill && h.border_left && h.border_bottom);
        assert!(!h.border_top && !h.border_right && h.wrap);
        assert_eq!((h.align, h.indent, h.size_half_pt, h.num_fmt), (Align::Center, 2, 28, NumFmt::Percent));
        let w = x[2].unpack().unwrap();
        assert!(!w.fill);
        assert_eq!(w.num_fmt, NumFmt::Thousands);
        assert!(parse_styles(b"<styleSheet><fonts/></styleSheet>").is_none());
    }

    #[test]
    fn sheet_scan_finds_formats_and_merges() {
        let cell = |row, col| Cell { row, col, value: Some(crate::model::Value::Number(1.0)), formula: None };
        let cells = vec![cell(0, 0), cell(1, 0), cell(1, 2)];
        let xml = br#"<worksheet><dimension ref="A1:C2"/><sheetData><row r="1"><c r="A1" s="1" t="s"><v>0</v></c>
            <c r="B1" s="1"/></row><row r="2"><c r="A2"><v>1</v></c><c r="C2" s="2"><v>2</v></c></row></sheetData>
            <mergeCells count="1"><mergeCell ref="A1:B1"/></mergeCells></worksheet>"#;
        let xfs = [Format::pack(&FormatSpec::default()), Format::pack(&FormatSpec { bold: true, ..FormatSpec::default() }),
            Format::pack(&FormatSpec { fill: true, ..FormatSpec::default() })];
        let dims = Dimensions { start: (0, 0), end: (1, 2) };
        let (m, f) = scan_sheet(&mut &xml[..], dims, Some(&xfs), &cells).unwrap();
        assert_eq!(m, vec![Rect { top: 0, left: 0, bottom: 0, right: 1 }]);
        assert_eq!(f.unwrap(), vec![xfs[1], xfs[0], xfs[2]]);
        // No styles: merges only. Wrong dimension: not this part.
        assert_eq!(scan_sheet(&mut &xml[..], dims, None, &cells).unwrap().1, None);
        assert!(scan_sheet(&mut &xml[..], Dimensions { start: (0, 0), end: (5, 5) }, Some(&xfs), &cells).is_none());
        // A cell without `r`: formats unknown, merges still found.
        let no_r = br#"<worksheet><dimension ref="A1:C2"/><sheetData><row r="1"><c s="1"><v>0</v></c></row></sheetData></worksheet>"#;
        let (m, f) = scan_sheet(&mut &no_r[..], dims, Some(&xfs), &cells).unwrap();
        assert!(m.is_empty() && f.is_none());
        // Namespace prefix, chunk-crossing tags: a long sheet.
        let mut big = String::from(r#"<x:worksheet xmlns:x="ns"><x:dimension ref="A1:B5000"/><x:sheetData>"#);
        let mut many = Vec::new();
        for r in 0..5000u32 {
            big.push_str(&format!(r#"<x:row r="{}"><x:c r="A{}" s="{}"><x:v>1</x:v></x:c><x:c r="B{}" s="0"/></x:row>"#, r + 1, r + 1, r % 3, r + 1));
            many.push(cell(r, 0));
        }
        big.push_str(r#"</x:sheetData><x:mergeCells count="1"><x:mergeCell ref="A1:B1"/></x:mergeCells></x:worksheet>"#);
        let dims = Dimensions { start: (0, 0), end: (4999, 1) };
        let (m, f) = scan_sheet(&mut big.as_bytes(), dims, Some(&xfs), &many).unwrap();
        assert_eq!(m.len(), 1);
        let f = f.unwrap();
        assert!((0..5000).all(|r| f[r] == xfs[r % 3]));
    }

    #[test]
    fn merge_tag_starts() {
        let b = b"<a/><mergeCells count=\"1\"><x:mergeCells>mergeCellsX";
        assert_eq!(merge_tag_start(b, 5), Some(4));
        assert_eq!(merge_tag_start(b, 29), Some(26));
        assert_eq!(merge_tag_start(b, 42), None);
    }
}
