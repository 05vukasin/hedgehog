//! A small SQL tokenizer for the notes and hints around a query (B75, B76, B88): which tables a
//! statement names (with their file prefix `f2.T1`), the text filters of its WHERE clauses
//! (`col = 'x'`, `col IN ('a', 'b')`, `col LIKE 'x%'`), its subqueries and CTEs, and `a + b`
//! over columns. It reads SQLite's lexical rules (strings, quoted identifiers, comments) but does
//! not parse statements: everything here only steers hints, never what runs.

/// One token of a statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// A name: bare (`T1`, `SELECT`) or quoted (`"Revenue / Q1"`, `[x]`, `` `x` ``).
    Ident { name: String, quoted: bool },
    /// A string literal, unescaped.
    Str(String),
    /// A number literal as written.
    Num(String),
    /// An operator or punctuation (`=`, `<>`, `(`, `,`, `.`, `+`, `||`).
    Op(String),
}

/// A token and its byte span in the statement.
#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub start: usize,
    pub end: usize,
}

impl Token {
    /// A bare word, compared without case (`kw("select")`).
    pub fn is_kw(&self, kw: &str) -> bool {
        matches!(&self.tok, Tok::Ident { name, quoted: false } if name.eq_ignore_ascii_case(kw))
    }

    pub fn is_op(&self, op: &str) -> bool {
        matches!(&self.tok, Tok::Op(o) if o == op)
    }

    pub fn ident(&self) -> Option<&str> {
        match &self.tok {
            Tok::Ident { name, .. } => Some(name),
            _ => None,
        }
    }
}

/// The tokens of `sql` (comments dropped). An unterminated string or name ends at the end.
pub fn tokens(sql: &str) -> Vec<Token> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let start = i;
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'-' && b.get(i + 1) == Some(&b'-') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i = sql[i + 2..].find("*/").map_or(b.len(), |k| i + 2 + k + 2);
        } else if c == b'\'' || c == b'"' || c == b'`' || c == b'[' {
            let close = if c == b'[' { b']' } else { c };
            let mut s = Vec::new();
            i += 1;
            while i < b.len() {
                if b[i] == close {
                    if close != b']' && b.get(i + 1) == Some(&close) {
                        s.push(close);
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                s.push(b[i]);
                i += 1;
            }
            let s = String::from_utf8_lossy(&s).into_owned();
            let tok = if c == b'\'' { Tok::Str(s) } else { Tok::Ident { name: s, quoted: true } };
            out.push(Token { tok, start, end: i });
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            out.push(Token { tok: Tok::Num(sql[start..i].to_string()), start, end: i });
        } else if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$' || b[i] >= 0x80) {
                i += 1;
            }
            out.push(Token { tok: Tok::Ident { name: sql[start..i].to_string(), quoted: false }, start, end: i });
        } else {
            let two = sql.get(i..i + 2).unwrap_or("");
            let op = if ["<>", "!=", "<=", ">=", "==", "||", "<<", ">>"].contains(&two) { two } else { &sql[i..i + 1] };
            i += op.len();
            out.push(Token { tok: Tok::Op(op.to_string()), start, end: i });
        }
    }
    out
}

/// A table reference: an optional schema (`f2`) and a name (`T1`, `T1_all`, `U1`, a CTE).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    pub schema: Option<String>,
    pub name: String,
}

/// Names that follow FROM / JOIN (and commas in a FROM list): the tables a statement reads, in
/// order, each once. CTE names are included; the caller tells them apart.
pub fn table_refs(toks: &[Token]) -> Vec<TableRef> {
    let mut out: Vec<TableRef> = Vec::new();
    let mut i = 0;
    let mut in_from = false;
    let mut depth_from: Vec<bool> = Vec::new();
    while i < toks.len() {
        let t = &toks[i];
        if t.is_op("(") {
            depth_from.push(in_from);
            in_from = false;
        } else if t.is_op(")") {
            in_from = depth_from.pop().unwrap_or(false);
        } else if t.is_kw("from") || t.is_kw("join") {
            in_from = true;
            if let Some((r, next)) = name_at(toks, i + 1) {
                if !out.contains(&r) {
                    out.push(r);
                }
                i = next;
                continue;
            }
        } else if in_from && t.is_op(",") {
            if let Some((r, next)) = name_at(toks, i + 1) {
                if !out.contains(&r) {
                    out.push(r);
                }
                i = next;
                continue;
            }
        } else if ["where", "group", "order", "having", "limit", "union", "except", "intersect", "window", "select"]
            .iter()
            .any(|k| t.is_kw(k))
        {
            in_from = false;
        }
        i += 1;
    }
    out
}

/// `[schema .] name` at `i` (not a subquery, not a keyword), and the index after it.
fn name_at(toks: &[Token], i: usize) -> Option<(TableRef, usize)> {
    let first = toks.get(i)?.ident()?;
    if matches!(&toks[i].tok, Tok::Ident { quoted: false, .. }) && ["select", "with", "values", "lateral"].iter().any(|k| toks[i].is_kw(k)) {
        return None;
    }
    if toks.get(i + 1).is_some_and(|t| t.is_op(".")) {
        let second = toks.get(i + 2)?.ident()?;
        // pragma_table_info(...) and other table-valued functions are no tables
        if toks.get(i + 3).is_some_and(|t| t.is_op("(")) {
            return None;
        }
        return Some((TableRef { schema: Some(first.to_string()), name: second.to_string() }, i + 3));
    }
    if toks.get(i + 1).is_some_and(|t| t.is_op("(")) {
        return None;
    }
    Some((TableRef { schema: None, name: first.to_string() }, i + 1))
}

/// A text or number filter of a WHERE / ON / HAVING clause: `col = 'x'`, `'x' = col`,
/// `col IN ('a', 'b')`, `col LIKE 'x%'`, `lower(col) = 'x'`.
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    /// The table alias or name before the column (`o` in `o.orderID`), if any.
    pub qualifier: Option<String>,
    pub column: String,
    /// `=`, `in` or `like`.
    pub op: &'static str,
    pub values: Vec<Lit>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Lit {
    Text(String),
    Num(String),
}

/// The column (with its qualifier) that ends just before token `i`: `col`, `t.col`, or the
/// column inside a one-argument function call `lower(col)` / `trim(t.col)`.
fn column_before(toks: &[Token], i: usize) -> Option<(Option<String>, String)> {
    let mut j = i.checked_sub(1)?;
    if toks[j].is_op(")") {
        // f(col) — one column inside one call
        let open = j.checked_sub(2).filter(|&k| toks[k].is_op("(")).or_else(|| j.checked_sub(4).filter(|&k| toks[k].is_op("(")))?;
        if !toks.get(open.checked_sub(1)?)?.ident().is_some() {
            return None;
        }
        j -= 1;
    }
    let name = toks[j].ident()?.to_string();
    if matches!(&toks[j].tok, Tok::Ident { quoted: false, .. }) && is_keyword(&name) {
        return None;
    }
    let qual = j.checked_sub(2).filter(|&k| toks[k + 1].is_op(".")).and_then(|k| toks[k].ident()).map(str::to_string);
    Some((qual, name))
}

fn is_keyword(w: &str) -> bool {
    ["and", "or", "not", "where", "select", "from", "on", "when", "then", "else", "case", "end", "null", "is", "between"]
        .iter()
        .any(|k| w.eq_ignore_ascii_case(k))
}

fn lit(t: &Token) -> Option<Lit> {
    match &t.tok {
        Tok::Str(s) => Some(Lit::Text(s.clone())),
        Tok::Num(n) => Some(Lit::Num(n.clone())),
        _ => None,
    }
}

/// The filters of a statement, at most `max`.
pub fn filters(toks: &[Token], max: usize) -> Vec<Filter> {
    let mut out: Vec<Filter> = Vec::new();
    for i in 0..toks.len() {
        if out.len() >= max {
            break;
        }
        let t = &toks[i];
        let op = if t.is_op("=") || t.is_op("==") {
            "="
        } else if t.is_kw("like") || t.is_kw("glob") {
            "like"
        } else if t.is_kw("in") {
            "in"
        } else {
            continue;
        };
        // NOT LIKE / NOT IN say nothing about missing values.
        if i > 0 && toks[i - 1].is_kw("not") {
            continue;
        }
        let f = match op {
            "in" => {
                if !toks.get(i + 1).is_some_and(|t| t.is_op("(")) {
                    continue;
                }
                let mut vals = Vec::new();
                let mut k = i + 2;
                while let Some(v) = toks.get(k).and_then(lit) {
                    vals.push(v);
                    if toks.get(k + 1).is_some_and(|t| t.is_op(",")) {
                        k += 2;
                    } else {
                        break;
                    }
                }
                if vals.is_empty() || !toks.get(k + 1).is_some_and(|t| t.is_op(")")) {
                    continue;
                }
                column_before(toks, i).map(|(q, c)| (q, c, vals))
            }
            _ => {
                let right = toks.get(i + 1).and_then(lit);
                match right {
                    // a literal followed by an operator is part of an expression (`= 'a' || x`)
                    Some(v) if !toks.get(i + 2).is_some_and(|t| matches!(&t.tok, Tok::Op(o) if o != ")" && o != ",")) => {
                        column_before(toks, i).map(|(q, c)| (q, c, vec![v]))
                    }
                    Some(_) => None,
                    None if op == "=" => {
                        // 'x' = col
                        let left = i.checked_sub(1).and_then(|k| lit(&toks[k]));
                        match (left, toks.get(i + 1).and_then(Token::ident)) {
                            (Some(v), Some(_)) => {
                                let qual = toks.get(i + 2).filter(|t| t.is_op(".")).and_then(|_| toks[i + 1].ident()).map(str::to_string);
                                let name = if qual.is_some() { toks.get(i + 3).and_then(Token::ident) } else { toks[i + 1].ident() };
                                name.map(|n| (qual, n.to_string(), vec![v]))
                            }
                            _ => None,
                        }
                    }
                    None => None,
                }
            }
        };
        if let Some((qualifier, column, values)) = f {
            let f = Filter { qualifier, column, op, values };
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

/// The statement has a WHERE (or HAVING) clause anywhere.
pub fn has_where(toks: &[Token]) -> bool {
    toks.iter().any(|t| t.is_kw("where") || t.is_kw("having"))
}

/// A derived table or CTE: its name (the alias or CTE name, if any) and the SQL that gives its
/// columns (`SELECT * FROM (<body>)`, or `WITH … SELECT * FROM <cte>`).
#[derive(Debug, Clone)]
pub struct Derived {
    pub name: Option<String>,
    /// A CTE (else a derived table in FROM / JOIN).
    pub cte: bool,
    pub columns_sql: String,
}

/// Index of the `)` that closes the `(` at `open`.
fn closing(toks: &[Token], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (k, t) in toks.iter().enumerate().skip(open) {
        if t.is_op("(") {
            depth += 1;
        } else if t.is_op(")") {
            depth -= 1;
            if depth == 0 {
                return Some(k);
            }
        }
    }
    None
}

/// The derived tables (`FROM (SELECT …) AS x`) and CTEs (`WITH x AS (SELECT …)`) of a statement.
pub fn derived(sql: &str, toks: &[Token]) -> Vec<Derived> {
    let mut out = Vec::new();
    // CTEs: WITH [RECURSIVE] name [(cols)] AS ( … ) [, name AS ( … )]*
    if toks.first().is_some_and(|t| t.is_kw("with")) {
        let mut k = 1;
        if toks.get(k).is_some_and(|t| t.is_kw("recursive")) {
            k += 1;
        }
        let mut names = Vec::new();
        let mut end_of_with = None;
        while let Some(name) = toks.get(k).and_then(Token::ident) {
            let name = name.to_string();
            k += 1;
            if toks.get(k).is_some_and(|t| t.is_op("(")) {
                k = closing(toks, k).map_or(toks.len(), |c| c + 1);
            }
            if !toks.get(k).is_some_and(|t| t.is_kw("as")) {
                break;
            }
            k += 1;
            if toks.get(k).is_some_and(|t| t.is_kw("not")) {
                k += 1;
            }
            if toks.get(k).is_some_and(|t| t.is_kw("materialized")) {
                k += 1;
            }
            if !toks.get(k).is_some_and(|t| t.is_op("(")) {
                break;
            }
            let Some(c) = closing(toks, k) else { break };
            names.push(name);
            end_of_with = Some(toks[c].end);
            k = c + 1;
            if toks.get(k).is_some_and(|t| t.is_op(",")) {
                k += 1;
            } else {
                break;
            }
        }
        if let Some(end) = end_of_with {
            for n in names {
                out.push(Derived {
                    cte: true,
                    columns_sql: format!("{} SELECT * FROM {}", &sql[..end], crate::text::quote_ident(&n)),
                    name: Some(n),
                });
            }
        }
    }
    // Derived tables: FROM ( SELECT … ) [AS] alias / JOIN ( SELECT … ) [AS] alias
    for i in 0..toks.len() {
        if !(toks[i].is_kw("from") || toks[i].is_kw("join") || (toks[i].is_op(",") && i > 0)) {
            continue;
        }
        let Some(open) = toks.get(i + 1).filter(|t| t.is_op("(")).map(|_| i + 1) else { continue };
        if !toks.get(open + 1).is_some_and(|t| t.is_kw("select") || t.is_kw("with") || t.is_kw("values")) {
            continue;
        }
        let Some(c) = closing(toks, open) else { continue };
        let body = &sql[toks[open + 1].start..toks[c].start];
        let mut a = c + 1;
        if toks.get(a).is_some_and(|t| t.is_kw("as")) {
            a += 1;
        }
        let name = toks.get(a).filter(|t| matches!(&t.tok, Tok::Ident { quoted: true, .. }) || !is_clause(t)).and_then(Token::ident);
        // A CTE body above may be needed by the subquery: keep the WITH clause in front.
        let with = out.iter().find_map(|d: &Derived| d.columns_sql.rfind(" SELECT * FROM ").map(|p| d.columns_sql[..p].to_string()));
        let columns_sql = match with {
            Some(w) => format!("{w} SELECT * FROM ({body})"),
            None => format!("SELECT * FROM ({body})"),
        };
        out.push(Derived { name: name.map(str::to_string), cte: false, columns_sql });
    }
    out
}

fn is_clause(t: &Token) -> bool {
    ["where", "group", "order", "having", "limit", "join", "left", "right", "inner", "outer", "cross", "natural", "on", "union", "except", "intersect", "window", "full"]
        .iter()
        .any(|k| t.is_kw(k))
}

/// Columns added with `+` (`a + b`, `t."x" + coalesce(y, 0)`): the bare column names on either
/// side of a `+`, each once.
pub fn plus_columns(toks: &[Token]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |n: &str| {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(n)) {
            out.push(n.to_string());
        }
    };
    for i in 0..toks.len() {
        if !toks[i].is_op("+") {
            continue;
        }
        if let Some((_, c)) = column_before(toks, i).filter(|_| i > 0 && !toks[i - 1].is_op(")")) {
            push(&c);
        }
        // right side: col or t.col (not a call)
        if let Some(n) = toks.get(i + 1).and_then(Token::ident) {
            if toks.get(i + 2).is_some_and(|t| t.is_op(".")) {
                if let Some(n2) = toks.get(i + 3).and_then(Token::ident) {
                    push(n2);
                }
            } else if !toks.get(i + 2).is_some_and(|t| t.is_op("(")) && !(matches!(&toks[i + 1].tok, Tok::Ident { quoted: false, .. }) && is_keyword(n)) {
                push(n);
            }
        }
    }
    out
}

/// Schema prefixes `f<k>` the statement uses as `f<k>.<table>` (k ≥ 1), each once, in order.
pub fn file_prefixes(toks: &[Token]) -> Vec<usize> {
    let mut out = Vec::new();
    for i in 0..toks.len().saturating_sub(2) {
        if !toks[i + 1].is_op(".") || toks[i + 2].ident().is_none() {
            continue;
        }
        // f2.T1.col is a column of an attached table; t.col (an alias) is not a schema: the
        // name after the dot must look like a table of ours (T<n>…, U<n>…).
        let Some(name) = toks[i].ident() else { continue };
        let Some(k) = name.strip_prefix(['f', 'F']).and_then(|d| d.parse::<usize>().ok()).filter(|&k| k >= 1) else { continue };
        let table = toks[i + 2].ident().unwrap_or_default();
        let ours = table.strip_prefix(['T', 't', 'U', 'u']).is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()));
        if ours && !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_strings_names_and_comments() {
        let t = tokens("SELECT \"a \"\"b\"\" c\", 'it''s' -- x\n FROM [T1] /* y */ WHERE x <> 1.5");
        let toks: Vec<&Tok> = t.iter().map(|t| &t.tok).collect();
        assert_eq!(toks[1], &Tok::Ident { name: "a \"b\" c".into(), quoted: true });
        assert_eq!(toks[3], &Tok::Str("it's".into()));
        assert_eq!(toks[5], &Tok::Ident { name: "T1".into(), quoted: true });
        assert_eq!(toks[8], &Tok::Op("<>".into()));
        assert_eq!(toks[9], &Tok::Num("1.5".into()));
    }

    #[test]
    fn finds_tables_and_prefixes() {
        let t = tokens("SELECT * FROM f1.T1 o JOIN \"f2\".\"T1\" d ON o.id = d.id, T3_all WHERE x IN (SELECT y FROM T4)");
        let refs = table_refs(&t);
        let names: Vec<String> = refs.iter().map(|r| format!("{}{}", r.schema.as_deref().map(|s| format!("{s}.")).unwrap_or_default(), r.name)).collect();
        assert_eq!(names, ["f1.T1", "f2.T1", "T3_all", "T4"]);
        assert_eq!(file_prefixes(&t), [1, 2]);
        assert!(file_prefixes(&tokens("SELECT f1.x FROM T1 f1")).is_empty());
        assert!(table_refs(&tokens("SELECT name FROM pragma_table_info('T1')")).is_empty());
    }

    #[test]
    fn finds_filters() {
        let t = tokens("SELECT SUM(x) FROM T1 WHERE agency_name = 'NYPD' AND t.\"Country Name\" IN ('A', 'B') AND lower(c) LIKE 'x%' AND 'Q' = d AND y = 2020 AND z NOT IN ('n')");
        let f = filters(&t, 10);
        let cols: Vec<(&str, &str, usize)> = f.iter().map(|f| (f.column.as_str(), f.op, f.values.len())).collect();
        assert_eq!(cols, [("agency_name", "=", 1), ("Country Name", "in", 2), ("c", "like", 1), ("d", "=", 1), ("y", "=", 1)]);
        assert_eq!(f[1].qualifier.as_deref(), Some("t"));
        assert!(filters(&tokens("SELECT a FROM T1 WHERE a = 'x' || b"), 5).is_empty());
    }

    #[test]
    fn finds_derived_tables_and_ctes() {
        let sql = "WITH a AS (SELECT x, SUM(y) AS s FROM T1 GROUP BY x) SELECT SUM(z) FROM (SELECT x AS z FROM a) sub";
        let d = derived(sql, &tokens(sql));
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].name.as_deref(), Some("a"));
        assert!(d[0].columns_sql.ends_with("GROUP BY x) SELECT * FROM \"a\""), "{}", d[0].columns_sql);
        assert_eq!(d[1].name.as_deref(), Some("sub"));
        assert!(d[1].columns_sql.starts_with("WITH a AS") && d[1].columns_sql.ends_with("SELECT * FROM (SELECT x AS z FROM a)"), "{}", d[1].columns_sql);
    }

    #[test]
    fn finds_added_columns() {
        let t = tokens("SELECT a + b + t.\"c d\", e + 1, coalesce(f, 0) + g FROM T1");
        assert_eq!(plus_columns(&t), ["a", "b", "c d", "e", "g"]);
    }
}
