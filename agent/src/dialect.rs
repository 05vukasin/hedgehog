//! SQLite replacements for MySQL / Postgres / SQL Server habits, appended to SQL errors.
//!
//! Models write `MONTH(d)`, `EXTRACT(YEAR FROM d)`, `SELECT TOP 5`, `ILIKE`; SQLite answers with
//! `no such function: MONTH` or `near "FROM": syntax error`, which a small model cannot act on.
//! [`hint`] turns such an error into the SQLite way of writing the same thing. Dates in the views
//! are ISO text (`YYYY-MM-DD`), so date parts come from `strftime`.

/// The replacement for a function SQLite does not have (name compared without case).
fn function_hint(name: &str) -> Option<String> {
    let part = |unit: &str, fmt: &str, range: &str| {
        format!(
            "SQLite has no {name}(); dates are ISO text, use strftime('{fmt}', col) for the {unit} (text {range}; \
             CAST(strftime('{fmt}', col) AS INTEGER) for a number)"
        )
    };
    let n = name.to_ascii_uppercase();
    let h = match n.as_str() {
        "YEAR" => part("year", "%Y", "'2020'"),
        "MONTH" => part("month", "%m", "'01'..'12'"),
        "DAY" | "DAYOFMONTH" => part("day", "%d", "'01'..'31'"),
        "HOUR" => part("hour", "%H", "'00'..'23'"),
        "MINUTE" => part("minute", "%M", "'00'..'59'"),
        "WEEK" | "WEEKOFYEAR" => part("week", "%W", "'00'..'53'"),
        "DAYOFWEEK" | "WEEKDAY" | "DOW" => part("weekday", "%w", "'0' = Sunday .. '6'"),
        "DAYOFYEAR" => part("day of the year", "%j", "'001'..'366'"),
        "QUARTER" => format!(
            "SQLite has no {name}(); use (CAST(strftime('%m', col) AS INTEGER) + 2) / 3 (dates are ISO text 'YYYY-MM-DD')"
        ),
        "MONTHNAME" | "DAYNAME" => {
            format!("SQLite has no {name}(); use strftime('%m', col) / strftime('%w', col) and name them yourself")
        }
        "DATE_FORMAT" | "TO_CHAR" | "FORMAT_DATE" | "DATE_PART" | "DATEPART" | "DATE_TRUNC" | "DATETRUNC" | "TRUNC_DATE" => {
            format!(
                "SQLite has no {name}(); use strftime(format, col) — e.g. strftime('%Y-%m', col) for the month \
                 ('2020-03'), strftime('%Y', col) for the year (dates are ISO text 'YYYY-MM-DD')"
            )
        }
        "TO_DATE" | "STR_TO_DATE" | "TO_TIMESTAMP" | "PARSE_DATE" => format!(
            "SQLite has no {name}(); dates are already ISO text ('2020-03-31'): compare them as strings \
             (col >= '2020-03-01') or use date(col)"
        ),
        "DATEDIFF" | "DATE_DIFF" | "TIMESTAMPDIFF" | "AGE" => format!(
            "SQLite has no {name}(); days between two dates: julianday(b) - julianday(a)"
        ),
        "DATEADD" | "DATE_ADD" | "DATE_SUB" | "ADDDATE" | "SUBDATE" => format!(
            "SQLite has no {name}(); use date(col, '+7 days') / date(col, '-1 month')"
        ),
        "NOW" | "CURDATE" | "GETDATE" | "SYSDATE" | "CURRENT_DATE" | "TODAY" | "CURRENT_TIMESTAMP" | "SYSDATETIME" => {
            format!("SQLite has no {name}(); use date('now') (or datetime('now'))")
        }
        "CONCAT" | "CONCAT_WS" => format!("SQLite has no {name}(); join strings with ||: a || ' ' || b"),
        "ISNULL" | "NVL" => format!("SQLite has no {name}(); use col IS NULL to test, ifnull(col, 0) or coalesce(col, 0) to replace"),
        "LEN" | "CHAR_LENGTH" | "CHARACTER_LENGTH" => format!("SQLite has no {name}(); use length(col)"),
        "STRING_AGG" | "LISTAGG" | "ARRAY_AGG" => format!("SQLite has no {name}(); use group_concat(col, ', ')"),
        "LEFT" => format!("SQLite has no {name}(); use substr(col, 1, n)"),
        "RIGHT" => format!("SQLite has no {name}(); use substr(col, -n)"),
        "CHARINDEX" | "LOCATE" | "POSITION" | "STRPOS" => format!("SQLite has no {name}(); use instr(col, 'text')"),
        "GREATEST" => format!("SQLite has no {name}(); max(a, b, …) with several arguments is the row-wise maximum"),
        "LEAST" => format!("SQLite has no {name}(); min(a, b, …) with several arguments is the row-wise minimum"),
        "RAND" | "RANDOM_VALUE" => format!("SQLite has no {name}(); use random()"),
        "MEDIAN" | "PERCENTILE" | "PERCENTILE_CONT" | "PERCENTILE_DISC" | "QUANTILE" => format!(
            "SQLite has no {name}(); for a median sort and take the middle row: \
             SELECT x FROM t WHERE x IS NOT NULL ORDER BY x LIMIT 1 OFFSET (SELECT count(x) / 2 FROM t)"
        ),
        "STDDEV" | "STDEV" | "STDDEV_POP" | "STDDEV_SAMP" | "VARIANCE" | "VAR_POP" | "VAR_SAMP" => format!(
            "SQLite has no {name}(); variance = avg(x * x) - avg(x) * avg(x), standard deviation = sqrt(that)"
        ),
        "REGEXP" | "REGEXP_LIKE" | "RLIKE" | "REGEXP_REPLACE" | "REGEXP_SUBSTR" => {
            format!("SQLite has no {name}(); use LIKE '%text%' (case-insensitive for ASCII) or GLOB '*[0-9]*'")
        }
        "TOP" => "SQLite has no TOP n; put LIMIT n at the end: SELECT … ORDER BY x DESC LIMIT 5".to_string(),
        "ILIKE" => "SQLite has no ILIKE; LIKE is already case-insensitive for ASCII letters".to_string(),
        "TO_NUMBER" | "TO_INT" | "INT" | "FLOAT" | "DECIMAL" | "NUMERIC" | "CONVERT" => {
            format!("SQLite has no {name}(); use CAST(col AS INTEGER) or CAST(col AS REAL)")
        }
        _ => return None,
    };
    Some(h)
}

/// Words of `sql` outside string literals and quoted identifiers, upper-cased.
fn words(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in sql.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        if c == '\'' || c == '"' || c == '`' || c == '[' {
            quote = Some(if c == '[' { ']' } else { c });
        }
        if c.is_alphanumeric() || c == '_' {
            cur.push(c.to_ascii_uppercase());
        } else {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            if !c.is_whitespace() && quote.is_none() {
                out.push(c.to_string());
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The SQLite way to write what failed, for errors caused by another SQL dialect; `None` when the
/// error does not look like one. `err` is SQLite's message, `sql` the statement.
pub fn hint(err: &str, sql: &str) -> Option<String> {
    let low = err.to_ascii_lowercase();
    if let Some(rest) = low.find("no such function:").map(|i| &err[i + "no such function:".len()..]) {
        let name: String = rest.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        return function_hint(&name);
    }
    if !low.contains("syntax error") && !low.contains("no such column") {
        return None;
    }
    let w = words(sql);
    let has = |k: &str| w.iter().any(|x| x == k);
    let mut hints = Vec::new();
    // SELECT TOP 5 … / SELECT DISTINCT TOP 5 …
    if w.windows(3).any(|x| x[0] == "SELECT" && x[1] == "TOP" && x[2].chars().all(|c| c.is_ascii_digit()))
        || w.windows(4).any(|x| x[0] == "SELECT" && x[1] == "DISTINCT" && x[2] == "TOP")
    {
        hints.push("SQLite has no TOP n; put LIMIT n at the end: SELECT … ORDER BY x DESC LIMIT 5".to_string());
    }
    if has("ILIKE") {
        hints.push("SQLite has no ILIKE; LIKE is already case-insensitive for ASCII letters".into());
    }
    if w.windows(2).any(|x| x[0] == "EXTRACT" && x[1] == "(") {
        hints.push(
            "SQLite has no EXTRACT(part FROM col); dates are ISO text, use strftime: strftime('%Y', col) year, \
             strftime('%m', col) month, strftime('%d', col) day (CAST(… AS INTEGER) for a number)"
                .into(),
        );
    }
    if w.windows(2).any(|x| x[0] == ":" && x[1] == ":") && low.contains("syntax error") {
        hints.push("SQLite has no ::type casts; use CAST(col AS INTEGER) / CAST(col AS REAL) / CAST(col AS TEXT)".into());
    }
    if w.windows(2).any(|x| x[0] == "FETCH" && (x[1] == "FIRST" || x[1] == "NEXT")) {
        hints.push("SQLite has no FETCH FIRST n ROWS; use LIMIT n (OFFSET m)".into());
    }
    if has("INTERVAL") {
        hints.push("SQLite has no INTERVAL; use date(col, '+1 month') / date(col, '-7 days')".into());
    }
    (!hints.is_empty()).then(|| hints.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn functions() {
        let h = hint("no such function: MONTH", "SELECT MONTH(d) FROM T1").unwrap();
        assert!(h.contains("strftime('%m', col)") && h.contains("CAST("), "{h}");
        assert!(hint("no such function: year", "").unwrap().contains("'%Y'"));
        assert!(hint("no such function: DATE_FORMAT", "").unwrap().contains("strftime('%Y-%m', col)"));
        assert!(hint("no such function: DATEDIFF", "").unwrap().contains("julianday"));
        assert!(hint("no such function: NOW", "").unwrap().contains("date('now')"));
        assert!(hint("no such function: MEDIAN", "").unwrap().contains("OFFSET"));
        assert!(hint("no such function: ISNULL", "").unwrap().contains("IS NULL"));
        assert!(hint("no such function: LEN", "").unwrap().contains("length("));
        assert!(hint("no such function: frobnicate", "").is_none());
    }

    #[test]
    fn syntax() {
        let top = hint("near \"5\": syntax error", "SELECT TOP 5 a FROM T1 ORDER BY a").unwrap();
        assert!(top.contains("LIMIT"), "{top}");
        assert!(hint("near \"ILIKE\": syntax error", "SELECT a FROM T1 WHERE a ILIKE '%x%'").unwrap().contains("LIKE"));
        let ex = hint("near \"FROM\": syntax error", "SELECT EXTRACT(MONTH FROM d) FROM T1").unwrap();
        assert!(ex.contains("strftime('%m', col)"), "{ex}");
        assert!(hint("near \":\": syntax error", "SELECT a::int FROM T1").unwrap().contains("CAST"));
        // Words inside literals and quoted names do not count.
        assert!(hint("near \"x\": syntax error", "SELECT 'top 5', \"ILIKE\" FROM T1 x y").is_none());
        assert!(hint("no such table: T9", "SELECT TOP 5 a FROM T9").is_none());
    }
}
