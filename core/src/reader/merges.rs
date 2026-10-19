//! Merged areas of xlsx sheets by a byte scan of the sheet XML.
//!
//! calamine's `merge_cells_by_sheet_name` re-parses the whole sheet XML (every cell event) only to
//! reach `<mergeCells>` at its end — about 40% of the reader's time on large sheets. Here the
//! part is just inflated and searched for the `mergeCells` element with `memmem`; the few tags
//! after it are parsed by hand with calamine's exact rules (first `mergeCells` element; every
//! `mergeCell` start tag's `ref` attribute; one bad reference empties the list, as calamine's
//! error does through `unwrap_or_default`).
//!
//! The sheet name → part path mapping repeats calamine's (`_rels/.rels` → workbook part,
//! `workbook.xml.rels` → targets). Whenever anything is unusual — an entity in a sheet name, a
//! part that is not found, a `<dimension>` that differs from the one calamine read — the
//! scanner returns `None` and the caller falls back to calamine.

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::sync::atomic::AtomicUsize;

use calamine::Dimensions;
use memchr::memmem;

use crate::model::Rect;

pub(super) static SCANNED: AtomicUsize = AtomicUsize::new(0);
pub(super) static FALLBACK: AtomicUsize = AtomicUsize::new(0);

pub struct Scanner<R: Read + Seek> {
    zip: zip::ZipArchive<R>,
    /// Sheet name → zip path of its part.
    parts: HashMap<String, String>,
}

impl<R: Read + Seek> Scanner<R> {
    pub fn new(reader: R) -> Option<Self> {
        let mut zip = zip::ZipArchive::new(reader).ok()?;
        // Workbook part, like calamine's read_package_relationships (last officeDocument wins).
        let rels = read_small(&mut zip, "_rels/.rels")?;
        let mut doc = None;
        for t in tags(&rels) {
            if !t.closing && local(t.name) == b"Relationship" {
                let ty = t.attr(b"Type").unwrap_or(b"");
                if ty.ends_with(b"/relationships/officeDocument") {
                    doc = Some(decode(t.attr(b"Target")?)?);
                }
            }
        }
        let doc = doc?;
        let xl_path = doc.rfind('/').map(|i| &doc[..=i]).map(|p| p.strip_prefix('/').unwrap_or(p)).unwrap_or("");
        let xl_path = xl_path.to_string();

        // Relationship id → target (raw, like calamine), last one wins.
        let rels = read_small(&mut zip, &format!("{xl_path}_rels/workbook.xml.rels"))?;
        let mut targets: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
        for t in tags(&rels) {
            if !t.closing && local(t.name) == b"Relationship" {
                if let Some(id) = t.attr(b"Id") {
                    targets.insert(id.to_vec(), t.attr(b"Target").unwrap_or(b"").to_vec());
                }
            }
        }
        let wb = read_small(&mut zip, &format!("{xl_path}workbook.xml"))?;
        let mut parts = HashMap::new();
        for t in tags(&wb) {
            if t.closing || local(t.name) != b"sheet" {
                continue;
            }
            let mut name = None;
            let mut rid: &[u8] = b"";
            for (k, v) in t.attrs() {
                if k == b"name" {
                    name = Some(decode(v)?);
                } else if local(k) == b"id" {
                    rid = v;
                }
            }
            let (Some(name), Some(target)) = (name, targets.get(rid)) else { continue };
            let target = std::str::from_utf8(target).ok()?;
            let path = match target.strip_prefix('/') {
                Some(abs) => abs.to_string(),
                None => format!("{xl_path}{target}"),
            };
            parts.insert(name, path);
        }
        Some(Scanner { zip, parts })
    }

    /// Merged areas of sheet `name`, or `None` to let the caller ask calamine. `dims` is the
    /// `<dimension>` calamine read for that sheet; the part found here must declare the same.
    pub fn merges(&mut self, name: &str, dims: Dimensions) -> Option<Vec<Rect>> {
        let path = self.parts.get(name)?;
        let mut f = self.zip.by_name(path).ok()?;
        let mut buf: Vec<u8> = Vec::with_capacity(CHUNK + TAIL);
        let mut chunk = vec![0u8; CHUNK];
        let finder = memmem::Finder::new(b"mergeCells");
        let mut checked_dims = false;
        // Bytes of `buf` already searched (keeping the last TAIL bytes for matches that cross
        // chunk boundaries and for the tag start before a match).
        loop {
            let n = read_full(&mut f, &mut chunk)?;
            buf.extend_from_slice(&chunk[..n]);
            if !checked_dims {
                // `<dimension>` comes before `<sheetData>`, normally within the first chunk.
                match declared_dimension(&buf) {
                    Some(d) if d.unwrap_or_default() == dims => checked_dims = true,
                    Some(_) => return None, // not the part calamine read
                    None if n == 0 || buf.len() >= 4 * CHUNK => return None,
                    None => continue,
                }
            }
            let mut from = 0;
            while let Some(j) = finder.find(&buf[from..]).map(|j| j + from) {
                if let Some(start) = tag_start(&buf, j) {
                    // Found: the rest of the part holds the list.
                    let mut rest = buf.split_off(start);
                    f.read_to_end(&mut rest).ok()?;
                    return Some(parse_merge_cells(&rest));
                }
                from = j + 1;
            }
            if n == 0 {
                return Some(Vec::new()); // no <mergeCells>
            }
            let keep = buf.len().min(TAIL);
            buf.drain(..buf.len() - keep);
        }
    }
}

const CHUNK: usize = 256 * 1024;
const TAIL: usize = 128;

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> Option<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(n)
}

fn read_small<R: Read + Seek>(zip: &mut zip::ZipArchive<R>, path: &str) -> Option<Vec<u8>> {
    let mut f = zip.by_name(path).ok()?;
    let mut v = Vec::new();
    f.read_to_end(&mut v).ok()?;
    Some(v)
}

/// The match at `j` is the name of a `mergeCells` start tag (`<mergeCells` or `<x:mergeCells`
/// followed by whitespace, `>` or `/`): index of its `<`.
fn tag_start(buf: &[u8], j: usize) -> Option<usize> {
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

/// `Some(Some(d))`: the part declares `<dimension ref=d>` before `<sheetData>`; `Some(None)`: it
/// does not; `None`: undecided yet (need more bytes) or unparsable.
fn declared_dimension(buf: &[u8]) -> Option<Option<Dimensions>> {
    for t in tags(buf) {
        if t.closing {
            continue;
        }
        match local(t.name) {
            b"dimension" => match t.attr(b"ref") {
                Some(r) => return dimension(r).map(Some),
                None => continue, // calamine also ignores a dimension without ref
            },
            b"sheetData" => return Some(None),
            _ => {}
        }
    }
    None
}

/// Merge list starting at the `<mergeCells` tag. calamine's semantics: every `mergeCell` start
/// tag until `</mergeCells>`; a missing end or a bad reference empties the whole list.
fn parse_merge_cells(xml: &[u8]) -> Vec<Rect> {
    let mut out = Vec::new();
    let mut it = tags(xml);
    match it.next() {
        Some(t) if t.self_closing => return out, // <mergeCells/>
        Some(_) => {}
        None => return out,
    }
    for t in it {
        let name = local(t.name);
        if t.closing && name == b"mergeCells" {
            return out;
        }
        if !t.closing && name == b"mergeCell" {
            if let Some(r) = t.attr(b"ref") {
                match dimension(r) {
                    Some(d) => out.push(Rect { top: d.start.0, left: d.start.1, bottom: d.end.0, right: d.end.1 }),
                    None => return Vec::new(),
                }
            }
        }
    }
    Vec::new() // no </mergeCells>: calamine's XmlEof error
}

/// calamine's `get_dimension`: `A1` or `A1:B2`, letters then digits, both required. Arithmetic
/// wraps like calamine's in a release build.
fn dimension(r: &[u8]) -> Option<Dimensions> {
    let mut parts = r.split(|&b| b == b':');
    let a = cell_ref(parts.next()?)?;
    match parts.next() {
        None => Some(Dimensions { start: a, end: a }),
        Some(b) => {
            let b = cell_ref(b)?;
            parts.next().is_none().then_some(Dimensions { start: a, end: b })
        }
    }
}

fn cell_ref(s: &[u8]) -> Option<(u32, u32)> {
    let (mut col, mut row) = (0u32, 0u32);
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            c @ b'A'..=b'Z' => col = col.wrapping_mul(26).wrapping_add((c - b'A') as u32 + 1),
            c @ b'a'..=b'z' => col = col.wrapping_mul(26).wrapping_add((c - b'a') as u32 + 1),
            c @ b'0'..=b'9' => {
                row = (c - b'0') as u32;
                i += 1;
                break;
            }
            _ => return None,
        }
        i += 1;
    }
    while i < s.len() {
        match s[i] {
            c @ b'0'..=b'9' => row = row.wrapping_mul(10).wrapping_add((c - b'0') as u32),
            _ => return None,
        }
        i += 1;
    }
    Some((row.checked_sub(1)?, col.checked_sub(1)?))
}

/// Unescapes the five XML entities and character references; `None` for anything else.
fn decode(v: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(v).ok()?;
    if !s.contains('&') {
        return Some(s.to_string());
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let end = rest[i..].find(';')? + i;
        let ent = &rest[i + 1..end];
        let ch = match ent {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(h) = ent.strip_prefix("#x") {
                    u32::from_str_radix(h, 16).ok()?
                } else {
                    ent.strip_prefix('#')?.parse().ok()?
                };
                char::from_u32(code)?
            }
        };
        out.push(ch);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn local(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

struct Tag<'a> {
    name: &'a [u8],
    attrs: &'a [u8],
    closing: bool,
    self_closing: bool,
}

impl<'a> Tag<'a> {
    fn attrs(&self) -> Attrs<'a> {
        Attrs { s: self.attrs }
    }

    fn attr(&self, key: &[u8]) -> Option<&'a [u8]> {
        self.attrs().find(|(k, _)| *k == key).map(|(_, v)| v)
    }
}

struct Attrs<'a> {
    s: &'a [u8],
}

impl<'a> Iterator for Attrs<'a> {
    type Item = (&'a [u8], &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let s = self.s;
        let start = s.iter().position(|b| !b.is_ascii_whitespace() && *b != b'/')?;
        let eq = start + s[start..].iter().position(|&b| b == b'=')?;
        let key = s[start..eq].trim_ascii();
        let q0 = eq + 1 + s[eq + 1..].iter().position(|b| !b.is_ascii_whitespace())?;
        let quote = s[q0];
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        let q1 = q0 + 1 + s[q0 + 1..].iter().position(|&b| b == quote)?;
        self.s = &s[q1 + 1..];
        Some((key, &s[q0 + 1..q1]))
    }
}

/// Element tags of an XML fragment, skipping comments, CDATA, declarations and processing
/// instructions.
fn tags(xml: &[u8]) -> Tags<'_> {
    Tags { xml, pos: 0 }
}

struct Tags<'a> {
    xml: &'a [u8],
    pos: usize,
}

impl<'a> Iterator for Tags<'a> {
    type Item = Tag<'a>;

    fn next(&mut self) -> Option<Tag<'a>> {
        let xml = self.xml;
        loop {
            let lt = self.pos + memchr::memchr(b'<', &xml[self.pos..])?;
            let rest = &xml[lt..];
            if rest.starts_with(b"<!--") {
                self.pos = lt + memmem::find(rest, b"-->")? + 3;
                continue;
            }
            if rest.starts_with(b"<![CDATA[") {
                self.pos = lt + memmem::find(rest, b"]]>")? + 3;
                continue;
            }
            // End of tag, respecting quoted attribute values.
            let mut i = 1;
            let mut quote = 0u8;
            loop {
                let b = *rest.get(i)?;
                if quote != 0 {
                    if b == quote {
                        quote = 0;
                    }
                } else if b == b'"' || b == b'\'' {
                    quote = b;
                } else if b == b'>' {
                    break;
                }
                i += 1;
            }
            self.pos = lt + i + 1;
            if rest[1] == b'?' || rest[1] == b'!' {
                continue;
            }
            let closing = rest[1] == b'/';
            let body = &rest[1 + closing as usize..i];
            let self_closing = body.last() == Some(&b'/');
            let body = if self_closing { &body[..body.len() - 1] } else { body };
            let name_end = body.iter().position(|b| b.is_ascii_whitespace()).unwrap_or(body.len());
            return Some(Tag { name: &body[..name_end], attrs: &body[name_end..], closing, self_closing });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_like_calamine() {
        assert_eq!(dimension(b"A1:B2"), Some(Dimensions { start: (0, 0), end: (1, 1) }));
        assert_eq!(dimension(b"c3"), Some(Dimensions { start: (2, 2), end: (2, 2) }));
        assert_eq!(dimension(b"XFD1048576").map(|d| d.end), Some((1_048_575, 16_383)));
        assert_eq!(dimension(b"A1:B2:C3"), None);
        assert_eq!(dimension(b"A:B"), None);
        assert_eq!(dimension(b"1:2"), None);
        assert_eq!(dimension(b"$A$1"), None);
    }

    #[test]
    fn merge_list() {
        let xml = br#"<mergeCells count="3"><mergeCell ref="A1:C1"/><x:mergeCell ref='B3:B9'></x:mergeCell><!-- <mergeCell ref="Z9"/> --><mergeCell other="1"/></mergeCells><pageMargins/>"#;
        assert_eq!(
            parse_merge_cells(xml),
            vec![Rect { top: 0, left: 0, bottom: 0, right: 2 }, Rect { top: 2, left: 1, bottom: 8, right: 1 }]
        );
        assert!(parse_merge_cells(br#"<mergeCells/>"#).is_empty());
        assert!(parse_merge_cells(br#"<mergeCells><mergeCell ref="A1:B"/><mergeCell ref="A1:B2"/></mergeCells>"#).is_empty());
        assert!(parse_merge_cells(br#"<mergeCells><mergeCell ref="A1:B2"/>"#).is_empty());
    }

    #[test]
    fn tag_starts_and_names() {
        let b = b"<a/><mergeCells count=\"1\"><x:mergeCells>mergeCellsX";
        assert_eq!(tag_start(b, 5), Some(4));
        assert_eq!(tag_start(b, 29), Some(26));
        assert_eq!(tag_start(b, 42), None);
        assert_eq!(decode(b"A &amp; B &#x41;&#66;"), Some("A & B AB".into()));
        assert_eq!(decode(b"&nbsp;"), None);
    }
}
