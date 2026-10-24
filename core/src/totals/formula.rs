//! Small formula reader: finds cell references and classifies the formula shape.
//! Not a full Excel grammar — it only needs to recognise aggregates (`SUM(C5:C20)`,
//! `SUBTOTAL(9,…)`, `AGGREGATE(9,…)`, `C5+C6+C7`, `AVERAGE`, `COUNT`, `MIN`, `MAX`) and to list every
//! reference with 0-based coordinates. Anything it does not understand becomes [`FormulaKind::Other`].

use crate::model::Rect;

/// Last 0-based row / column of an xlsx sheet; whole-column and whole-row references extend to them.
pub const MAX_ROW: u32 = 1_048_575;
pub const MAX_COL: u32 = 16_383;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKind {
    /// `C5`, `$C$5`
    Cell,
    /// `C5:C20`
    Range,
    /// `A:A`, `B:D`
    Columns,
    /// `5:5`, `3:9`
    Rows,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ref {
    /// Sheet named in the reference (`Sheet2!B3` → `Sheet2`); `None` = the formula's own sheet.
    pub sheet: Option<String>,
    /// Points into another workbook (`[1]Sheet1!A1`, `'C:\x\[Book.xlsx]S'!A1`).
    pub external: bool,
    /// Inclusive, 0-based, normalised so `top <= bottom` and `left <= right`.
    pub rect: Rect,
    pub kind: RefKind,
}

impl Ref {
    /// True if the reference points into `sheet_name` of the same workbook (unqualified or
    /// qualified with its own name).
    pub fn is_local(&self, sheet_name: &str) -> bool {
        !self.external
            && match &self.sheet {
                None => true,
                Some(s) => s.eq_ignore_ascii_case(sheet_name) || s.to_lowercase() == sheet_name.to_lowercase(),
            }
    }

    pub fn contains(&self, row: u32, col: u32) -> bool {
        let r = &self.rect;
        r.top <= row && row <= r.bottom && r.left <= col && col <= r.right
    }

    /// Number of cells covered (u64: whole columns are large).
    pub fn area(&self) -> u64 {
        (self.rect.bottom - self.rect.top + 1) as u64 * (self.rect.right - self.rect.left + 1) as u64
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FormulaKind {
    /// `SUM(…)`, `AGGREGATE(9,…)`, or a `+` chain mixing references and SUM calls.
    Sum,
    /// `SUBTOTAL(9|109, …)` — Excel skips nested SUBTOTAL cells inside the range.
    Subtotal,
    /// Only references joined by `+`: `C5+C6+C7`.
    AdditionChain,
    Average,
    Count,
    Min,
    Max,
    /// A single cell link: `B7`, `Sheet2!C9`.
    Reference,
    Other,
}

impl FormulaKind {
    /// Formula whose value should equal the sum of its [`ParsedFormula::sum_refs`].
    pub fn is_sum(self) -> bool {
        matches!(self, FormulaKind::Sum | FormulaKind::Subtotal | FormulaKind::AdditionChain)
    }

    /// Any recognised aggregate (sum, average, count, min, max).
    pub fn is_aggregate(self) -> bool {
        !matches!(self, FormulaKind::Reference | FormulaKind::Other)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedFormula {
    pub kind: FormulaKind,
    /// Every reference in the formula, in order of appearance.
    pub refs: Vec<Ref>,
    /// For sum kinds: the operands being added (a ref may repeat, as in `C5+C5`). Empty otherwise.
    pub sum_refs: Vec<Ref>,
    /// Upper-case function names in order (`_xlfn.` prefixes removed).
    pub functions: Vec<String>,
    /// Contains an error literal such as `#REF!`.
    pub has_error: bool,
}

impl ParsedFormula {
    /// True if any reference points outside the given sheet (other sheet or other workbook).
    pub fn has_remote_refs(&self, sheet_name: &str) -> bool {
        self.refs.iter().any(|r| !r.is_local(sheet_name))
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ref(Ref),
    /// Function name; the opening parenthesis is part of the token.
    Func(String),
    Num(f64),
    Str,
    Name,
    Err,
    Op(char),
    Open,
    Close,
    Comma,
}

enum Part {
    Cell(u32, u32),
    Col(u32),
    Row(u32),
}

fn parse_col(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 3 || !s.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    let mut c: u32 = 0;
    for b in s.bytes() {
        c = c * 26 + (b.to_ascii_uppercase() - b'A' + 1) as u32;
    }
    (c >= 1 && c - 1 <= MAX_COL).then(|| c - 1)
}

fn parse_row(s: &str) -> Option<u32> {
    if s.is_empty() || s.len() > 7 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let r: u32 = s.parse().ok()?;
    (r >= 1 && r - 1 <= MAX_ROW).then(|| r - 1)
}

/// `$C$5` → Cell, `$C` → Col, `5` → Row.
fn parse_part(s: &str) -> Option<Part> {
    let s = s.strip_prefix('$').unwrap_or(s);
    let split = s.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(s.len());
    let (letters, rest) = s.split_at(split);
    let digits = rest.strip_prefix('$').unwrap_or(rest);
    match (letters.is_empty(), digits.is_empty()) {
        (false, false) => Some(Part::Cell(parse_row(digits)?, parse_col(letters)?)),
        (false, true) if rest.is_empty() => Some(Part::Col(parse_col(letters)?)),
        (true, false) if rest == digits => Some(Part::Row(parse_row(digits)?)),
        _ => None,
    }
}

fn rect(r1: u32, c1: u32, r2: u32, c2: u32) -> Rect {
    Rect { top: r1.min(r2), left: c1.min(c2), bottom: r1.max(r2), right: c1.max(c2) }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '.' | '$' | '\\' | '?')
}

/// Strips `[...]` workbook parts and a path from a sheet name; returns (name, external).
fn clean_sheet(name: &str) -> (String, bool) {
    match name.rfind(']') {
        Some(i) => (name[i + 1..].to_string(), true),
        None => (name.to_string(), false),
    }
}

fn tokenize(f: &str) -> Vec<Tok> {
    let chars: Vec<char> = f.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    // Sheet prefix waiting for its reference, and whether an external `[n]` was seen.
    let mut sheet: Option<String> = None;
    let mut external = false;
    let read_word = |mut j: usize| {
        let start = j;
        while j < n && is_word_char(chars[j]) {
            j += 1;
        }
        (chars[start..j].iter().collect::<String>(), j)
    };
    while i < n {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\r' | '\n' | '@' => i += 1,
            '"' => {
                i += 1;
                while i < n {
                    if chars[i] == '"' {
                        if i + 1 < n && chars[i + 1] == '"' {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
                out.push(Tok::Str);
            }
            '\'' => {
                // Quoted sheet name, `''` escapes a quote.
                let mut name = String::new();
                i += 1;
                while i < n {
                    if chars[i] == '\'' {
                        if i + 1 < n && chars[i + 1] == '\'' {
                            name.push('\'');
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    name.push(chars[i]);
                    i += 1;
                }
                i += 1;
                if i < n && chars[i] == '!' {
                    i += 1;
                    let (s, ext) = clean_sheet(&name);
                    sheet = Some(s);
                    external |= ext;
                } else {
                    out.push(Tok::Name);
                }
            }
            '[' => {
                // External workbook index `[1]Sheet!A1` or a structured reference `Table[Col]`.
                let operand_start = !matches!(out.last(), Some(Tok::Name));
                let mut depth = 0;
                while i < n {
                    if chars[i] == '[' {
                        depth += 1;
                    } else if chars[i] == ']' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    i += 1;
                }
                i += 1;
                if operand_start {
                    external = true;
                }
            }
            '#' => {
                // Error literal: #REF!, #N/A, #DIV/0!, #NULL!, #VALUE!, #NAME?, #NUM!
                i += 1;
                while i < n && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '/' | '!' | '?')) {
                    let stop = matches!(chars[i], '!' | '?');
                    i += 1;
                    if stop {
                        break;
                    }
                }
                out.push(Tok::Err);
                sheet = None;
                external = false;
            }
            '(' => {
                out.push(Tok::Open);
                i += 1;
            }
            ')' => {
                out.push(Tok::Close);
                i += 1;
            }
            ',' | ';' => {
                out.push(Tok::Comma);
                i += 1;
            }
            c if is_word_char(c) => {
                let (word, j) = read_word(i);
                i = j;
                if i < n && chars[i] == '!' {
                    i += 1;
                    let (s, ext) = clean_sheet(&word);
                    sheet = Some(s);
                    external |= ext;
                    continue;
                }
                if i < n && chars[i] == '(' && sheet.is_none() {
                    i += 1;
                    let up = word.to_ascii_uppercase();
                    let name = up
                        .strip_prefix("_XLFN.")
                        .or_else(|| up.strip_prefix("_XLWS."))
                        .unwrap_or(&up)
                        .to_string();
                    out.push(Tok::Func(name));
                    continue;
                }
                let first = parse_part(&word);
                let mut tok = None;
                if let Some(p1) = &first {
                    if i < n && chars[i] == ':' {
                        // Second half of a range; may carry a (redundant) sheet prefix.
                        let mut k = i + 1;
                        let (mut w2, mut k2) = read_word(k);
                        if k2 < n && chars[k2] == '!' {
                            k = k2 + 1;
                            (w2, k2) = read_word(k);
                        }
                        let r = match (p1, parse_part(&w2)) {
                            (Part::Cell(r1, c1), Some(Part::Cell(r2, c2))) => {
                                Some((rect(*r1, *c1, r2, c2), RefKind::Range))
                            }
                            (Part::Col(c1), Some(Part::Col(c2))) => Some((rect(0, *c1, MAX_ROW, c2), RefKind::Columns)),
                            (Part::Row(r1), Some(Part::Row(r2))) => Some((rect(*r1, 0, r2, MAX_COL), RefKind::Rows)),
                            _ => None,
                        };
                        if let Some((rect, kind)) = r {
                            i = k2;
                            tok = Some(Tok::Ref(Ref { sheet: sheet.take(), external, rect, kind }));
                        }
                    } else if let Part::Cell(r, c) = p1 {
                        tok = Some(Tok::Ref(Ref {
                            sheet: sheet.take(),
                            external,
                            rect: rect(*r, *c, *r, *c),
                            kind: RefKind::Cell,
                        }));
                    }
                }
                let tok = tok.unwrap_or_else(|| {
                    // Number literal (with exponent sign split off by the word reader).
                    if word.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
                        let mut w = word.clone();
                        if (w.ends_with('E') || w.ends_with('e')) && i < n && matches!(chars[i], '+' | '-') {
                            let (exp, j) = read_word(i + 1);
                            w = format!("{w}{}{exp}", chars[i]);
                            i = j;
                        }
                        w.parse().map(Tok::Num).unwrap_or(Tok::Name)
                    } else {
                        Tok::Name
                    }
                });
                if matches!(tok, Tok::Ref(_)) {
                    external = false;
                } else {
                    sheet = None;
                    external = false;
                }
                out.push(tok);
            }
            _ => {
                out.push(Tok::Op(c));
                i += 1;
            }
        }
    }
    out
}

/// One `+`-separated top-level term.
enum Term {
    Ref(Ref),
    Call { kind: FormulaKind, refs: Vec<Ref> },
    Other,
}

fn split_top(toks: &[Tok], is_sep: impl Fn(&Tok) -> bool) -> Vec<&[Tok]> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, t) in toks.iter().enumerate() {
        match t {
            Tok::Func(_) | Tok::Open => depth += 1,
            Tok::Close => depth -= 1,
            t if depth == 0 && is_sep(t) => {
                parts.push(&toks[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&toks[start..]);
    parts
}

fn code_kind(code: f64, subtotal: bool) -> Option<FormulaKind> {
    let c = code as i64;
    if code.fract() != 0.0 {
        return None;
    }
    let base = if subtotal && c > 100 { c - 100 } else { c };
    Some(match base {
        9 if subtotal => FormulaKind::Subtotal,
        9 => FormulaKind::Sum,
        1 => FormulaKind::Average,
        2 | 3 => FormulaKind::Count,
        4 => FormulaKind::Max,
        5 => FormulaKind::Min,
        _ => return None,
    })
}

fn term(toks: &[Tok]) -> Term {
    match toks {
        [Tok::Ref(r)] => Term::Ref(r.clone()),
        [Tok::Func(name), inner @ .., Tok::Close] => {
            // The closing parenthesis must belong to this call, not to a nested one.
            let mut depth = 0;
            for t in inner {
                match t {
                    Tok::Func(_) | Tok::Open => depth += 1,
                    Tok::Close => {
                        depth -= 1;
                        if depth < 0 {
                            return Term::Other;
                        }
                    }
                    _ => {}
                }
            }
            let args = split_top(inner, |t| matches!(t, Tok::Comma));
            let refs_from = |args: &[&[Tok]]| -> Option<Vec<Ref>> {
                args.iter()
                    .map(|a| match a {
                        [Tok::Ref(r)] => Some(r.clone()),
                        _ => None,
                    })
                    .collect()
            };
            let num = |a: &[Tok]| match a {
                [Tok::Num(v)] => Some(*v),
                _ => None,
            };
            let (kind, rest) = match name.as_str() {
                "SUM" => (Some(FormulaKind::Sum), &args[..]),
                "AVERAGE" | "AVERAGEA" => (Some(FormulaKind::Average), &args[..]),
                "COUNT" | "COUNTA" => (Some(FormulaKind::Count), &args[..]),
                "MIN" | "MINA" => (Some(FormulaKind::Min), &args[..]),
                "MAX" | "MAXA" => (Some(FormulaKind::Max), &args[..]),
                "SUBTOTAL" if args.len() >= 2 => (num(args[0]).and_then(|c| code_kind(c, true)), &args[1..]),
                // AGGREGATE(function, options, ref1, …) — reference form only.
                "AGGREGATE" if args.len() >= 3 && num(args[1]).is_some() => {
                    (num(args[0]).and_then(|c| code_kind(c, false)), &args[2..])
                }
                _ => (None, &args[..]),
            };
            match (kind, refs_from(rest)) {
                (Some(kind), Some(refs)) if !refs.is_empty() => Term::Call { kind, refs },
                _ => Term::Other,
            }
        }
        _ => Term::Other,
    }
}

fn classify(toks: &[Tok]) -> (FormulaKind, Vec<Ref>) {
    let mut toks = toks;
    while let [Tok::Op('+' | '='), rest @ ..] = toks {
        toks = rest;
    }
    if toks.is_empty() {
        return (FormulaKind::Other, Vec::new());
    }
    let terms: Vec<Term> = split_top(toks, |t| matches!(t, Tok::Op('+'))).into_iter().map(term).collect();
    if terms.len() == 1 {
        return match terms.into_iter().next().unwrap() {
            Term::Ref(r) if r.kind == RefKind::Cell => (FormulaKind::Reference, Vec::new()),
            Term::Call { kind, refs } => {
                let refs = if kind.is_sum() { refs } else { Vec::new() };
                (kind, refs)
            }
            _ => (FormulaKind::Other, Vec::new()),
        };
    }
    let mut sum_refs = Vec::new();
    let mut calls = 0;
    let mut subtotal_calls = 0;
    for t in terms {
        match t {
            Term::Ref(r) => sum_refs.push(r),
            Term::Call { kind: k @ (FormulaKind::Sum | FormulaKind::Subtotal), refs } => {
                calls += 1;
                subtotal_calls += (k == FormulaKind::Subtotal) as usize;
                sum_refs.extend(refs);
            }
            _ => return (FormulaKind::Other, Vec::new()),
        }
    }
    let kind = match (calls, subtotal_calls) {
        (0, _) => FormulaKind::AdditionChain,
        (c, s) if c == s => FormulaKind::Subtotal,
        _ => FormulaKind::Sum,
    };
    (kind, sum_refs)
}

/// Parses a formula (with or without the leading `=`).
pub fn parse(formula: &str) -> ParsedFormula {
    let toks = tokenize(formula);
    let (kind, sum_refs) = classify(&toks);
    let mut refs = Vec::new();
    let mut functions = Vec::new();
    let mut has_error = false;
    for t in toks {
        match t {
            Tok::Ref(r) => refs.push(r),
            Tok::Func(f) => functions.push(f),
            Tok::Err => has_error = true,
            _ => {}
        }
    }
    // An error literal inside an aggregate (`SUM(#REF!)`) already made it `Other`.
    ParsedFormula { kind, refs, sum_refs, functions, has_error }
}

// TEMPORARY attribution entry point. Keep the normal parser unchanged and remove this helper
// with the profiling patch before any production acceptance comparison.
pub(crate) fn parse_profiled(formula: &str, profile: &mut [std::time::Duration; 3]) -> ParsedFormula {
    let started = std::time::Instant::now();
    let toks = tokenize(formula);
    profile[0] += started.elapsed();
    let started = std::time::Instant::now();
    let (kind, sum_refs) = classify(&toks);
    profile[1] += started.elapsed();
    let started = std::time::Instant::now();
    let mut refs = Vec::new();
    let mut functions = Vec::new();
    let mut has_error = false;
    for t in toks {
        match t {
            Tok::Ref(r) => refs.push(r),
            Tok::Func(f) => functions.push(f),
            Tok::Err => has_error = true,
            _ => {}
        }
    }
    profile[2] += started.elapsed();
    ParsedFormula { kind, refs, sum_refs, functions, has_error }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(t: u32, l: u32, b: u32, rt: u32) -> Rect {
        Rect { top: t, left: l, bottom: b, right: rt }
    }

    fn one(f: &str) -> Ref {
        let p = parse(f);
        assert_eq!(p.refs.len(), 1, "{f}: {:?}", p.refs);
        p.refs[0].clone()
    }

    #[test]
    fn reference_forms() {
        assert_eq!(one("C5").rect, r(4, 2, 4, 2));
        assert_eq!(one("C5").kind, RefKind::Cell);
        assert_eq!(one("$C$5").rect, r(4, 2, 4, 2));
        assert_eq!(one("C$5").rect, r(4, 2, 4, 2));
        assert_eq!(one("SUM(C5:C20)").rect, r(4, 2, 19, 2));
        assert_eq!(one("SUM($C$20:$C5)").rect, r(4, 2, 19, 2));
        let a = one("SUM(A:A)");
        assert_eq!((a.kind, a.rect), (RefKind::Columns, r(0, 0, MAX_ROW, 0)));
        let b = one("SUM($B:$D)");
        assert_eq!((b.kind, b.rect), (RefKind::Columns, r(0, 1, MAX_ROW, 3)));
        let row = one("SUM(5:5)");
        assert_eq!((row.kind, row.rect), (RefKind::Rows, r(4, 0, 4, MAX_COL)));
        let s = one("Sheet2!B3");
        assert_eq!((s.sheet.as_deref(), s.rect), (Some("Sheet2"), r(2, 1, 2, 1)));
        assert!(!s.is_local("Sheet1") && s.is_local("sheet2"));
        let q = one("SUM('My Sheet'!B3:B9)");
        assert_eq!((q.sheet.as_deref(), q.rect), (Some("My Sheet"), r(2, 1, 8, 1)));
        let esc = one("'Bob''s'!A1");
        assert_eq!(esc.sheet.as_deref(), Some("Bob's"));
        let ext = one("[1]Sheet1!A1");
        assert!(ext.external && ext.sheet.as_deref() == Some("Sheet1"));
        let ext2 = one("'C:\\x\\[Book.xlsx]Data'!$A$1:$A$3");
        assert!(ext2.external && ext2.sheet.as_deref() == Some("Data") && ext2.kind == RefKind::Range);
        assert_eq!(one("XFD1048576").rect, r(MAX_ROW, MAX_COL, MAX_ROW, MAX_COL));
        assert_eq!(one("AA10").rect, r(9, 26, 9, 26));
        assert_eq!(one("Sheet1!A1:Sheet1!B2").rect, r(0, 0, 1, 1));
        assert_eq!(one("Data_2!A1").sheet.as_deref(), Some("Data_2"));
    }

    #[test]
    fn non_references() {
        assert!(parse("\"C5 is a string\"").refs.is_empty());
        assert!(parse("LOG10(100)").refs.is_empty());
        assert!(parse("XFE1+A0").refs.is_empty()); // out of range column, row 0
        assert!(parse("1.5E+3*2").refs.is_empty());
        assert!(parse("TRUE").refs.is_empty());
        assert!(parse("Table1[Amount]").refs.is_empty());
        assert!(parse("SUM(#REF!)").has_error);
        assert_eq!(parse("IF(A1>0,\"x\",B2)").refs.len(), 2);
        assert_eq!(parse("VLOOKUP(A2,Prices!$A$2:$C$99,3,FALSE)").refs.len(), 2);
    }

    #[test]
    fn classification() {
        use FormulaKind::*;
        let k = |f: &str| parse(f).kind;
        assert_eq!(k("SUM(C5:C20)"), Sum);
        assert_eq!(k("=SUM(C5:C20)"), Sum);
        assert_eq!(k("sum(C5:C9, E5:E9)"), Sum);
        assert_eq!(k("SUBTOTAL(9,C5:C20)"), Subtotal);
        assert_eq!(k("SUBTOTAL(109,C5:C20)"), Subtotal);
        assert_eq!(k("SUBTOTAL(1,C5:C20)"), Average);
        assert_eq!(k("SUBTOTAL(3,C5:C20)"), Count);
        assert_eq!(k("_xlfn.AGGREGATE(9,6,C5:C20)"), Sum);
        assert_eq!(k("C5+C6+C7"), AdditionChain);
        assert_eq!(k("+C5+C6"), AdditionChain);
        assert_eq!(k("C10+C20+SUM(C30:C31)"), Sum);
        assert_eq!(k("SUBTOTAL(9,C2:C4)+SUBTOTAL(9,C6:C8)"), Subtotal);
        assert_eq!(k("AVERAGE(B2:B9)"), Average);
        assert_eq!(k("COUNTA(B2:B9)"), Count);
        assert_eq!(k("MIN(B2:B9)"), Min);
        assert_eq!(k("MAX(B2:B9)"), Max);
        assert_eq!(k("B7"), Reference);
        assert_eq!(k("Sheet2!B7"), Reference);
        assert_eq!(k("C5-C6"), Other);
        assert_eq!(k("C5*2"), Other);
        assert_eq!(k("SUM(C5:C9)/2"), Other);
        assert_eq!(k("ROUND(SUM(C5:C9),2)"), Other);
        assert_eq!(k("SUM(C5:C9)+1"), Other);
        assert_eq!(k("SUM(C5:C9)*SUM(D5:D9)"), Other);
        assert_eq!(k("(C5)+(C6)"), Other);
        assert_eq!(k("SUM(IF(A1:A9>0,B1:B9))"), Other);
        assert_eq!(k(""), Other);
    }

    #[test]
    fn sum_refs_and_functions() {
        let p = parse("C10+C20+SUM(C30:C31)");
        assert_eq!(p.sum_refs.len(), 3);
        assert_eq!(p.sum_refs[2].rect, r(29, 2, 30, 2));
        assert_eq!(p.functions, vec!["SUM"]);
        let a = parse("AVERAGE(B2:B9)");
        assert!(a.sum_refs.is_empty() && a.refs.len() == 1);
        assert!(parse("SUM(Sheet2!A1:A3)").has_remote_refs("Sheet1"));
        assert!(!parse("SUM(Sheet1!A1:A3)").has_remote_refs("Sheet1"));
    }

    #[test]
    fn garbage_does_not_panic() {
        for f in ["(((", ")))", "SUM(", "'unterminated", "\"open", "A1:", ":B2", "[1", "#", "$", "$$A$$1", "Ä1+ö2", "1E", "SUM(,,)", "{1,2;3,4}"] {
            let _ = parse(f);
        }
    }

    #[test]
    fn classification_preserves_all_tokens_after_an_unsupported_term() {
        let reference = |sheet: Option<&str>, rect, kind| Ref {
            sheet: sheet.map(str::to_string), external: false, rect, kind,
        };
        let a1 = reference(None, r(0, 0, 0, 0), RefKind::Cell);
        let local_range = reference(None, r(0, 0, 1, 0), RefKind::Range);
        let later = reference(Some("Later"), r(0, 3, 1, 3), RefKind::Range);
        let cases = [
            ("SUM(A1:A2)", FormulaKind::Sum, vec![local_range.clone()], vec!["SUM"], false),
            ("A1+A1", FormulaKind::AdditionChain, vec![a1.clone(), a1.clone()], vec![], false),
            ("A1:A2", FormulaKind::Other, vec![local_range], vec![], false),
            ("IF(A1>0,B1,C1)+SUM('Later'!D1:D2)+#REF!", FormulaKind::Other,
                vec![a1.clone(), reference(None, r(0, 1, 0, 1), RefKind::Cell),
                    reference(None, r(0, 2, 0, 2), RefKind::Cell), later.clone()],
                vec!["IF", "SUM"], true),
            ("A1+1+SUM('Later'!D1:D2)", FormulaKind::Other,
                vec![a1.clone(), later], vec!["SUM"], false),
            ("A1++SUM(B1:B2)", FormulaKind::Other,
                vec![a1, reference(None, r(0, 1, 1, 1), RefKind::Range)], vec!["SUM"], false),
        ];
        for (formula, kind, refs, functions, has_error) in cases {
            let sum_refs = if kind.is_sum() { refs.clone() } else { Vec::new() };
            let expected = ParsedFormula {
                kind, refs, sum_refs,
                functions: functions.into_iter().map(str::to_string).collect(), has_error,
            };
            assert_eq!(parse(formula), expected, "{formula}");
        }
    }
}
