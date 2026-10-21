//! The `cells` tool: original source cells of a range, for citing exact values.

use hh_core::model::{Value, Workbook};
use hh_core::{a1, Grid};

use crate::text::{num, parse_range, range_a1, sheet_ref, short};

/// Most cells one call may ask for.
pub const MAX_CELLS: u64 = 200;

fn value_text(v: &Value) -> String {
    match v {
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Number(x) => num(*x),
        Value::Text(t) => short(t, 200),
        Value::DateTime(x) => hh_core::normalize::iso(*x),
        Value::Error(e) => e.clone(),
    }
}

/// `A1<TAB>value` lines for the non-empty cells of `range` on `sheet` (merged areas marked,
/// formulas shown after the value).
pub fn cells(wb: &Workbook, sheet: &str, range: &str) -> Result<String, String> {
    let (in_ref, rect) = parse_range(range).ok_or_else(|| format!("bad range {range:?}; use A1 notation such as B3:F12 or B3"))?;
    let wanted = in_ref.as_deref().filter(|s| !s.is_empty()).unwrap_or(sheet).trim();
    let s = wb
        .sheets
        .iter()
        .find(|s| s.name == wanted)
        .or_else(|| wb.sheets.iter().find(|s| s.name.trim().eq_ignore_ascii_case(wanted)))
        .ok_or_else(|| {
            let names: Vec<String> = wb.sheets.iter().take(40).map(|s| format!("'{}'", s.name)).collect();
            format!("no sheet {wanted:?}; sheets: {}", names.join(", "))
        })?;
    let area = u64::from(rect.bottom - rect.top + 1) * u64::from(rect.right - rect.left + 1);
    if area > MAX_CELLS {
        return Err(format!(
            "range {} has {area} cells; at most {MAX_CELLS} per call — ask for a smaller range (or use query for table values)",
            range_a1(rect)
        ));
    }
    let grid = Grid::new(s);
    let mut lines = Vec::new();
    let mut empty = 0usize;
    for r in rect.top..=rect.bottom {
        for c in rect.left..=rect.right {
            let merge = grid.merge_at(r, c);
            let cell = grid.cell(r, c);
            let mut line = a1(r, c);
            match cell {
                Some(cell) => {
                    line.push('\t');
                    match &cell.value {
                        Some(v) => line.push_str(&value_text(v)),
                        None => line.push_str("(formula without a saved value)"),
                    }
                    if let Some(f) = &cell.formula {
                        line.push_str(&format!("\t={}", short(f, 120)));
                    }
                }
                None => match merge {
                    Some(m) if (m.top, m.left) != (r, c) => {
                        line.push_str(&format!("\t[merged into {}]", a1(m.top, m.left)));
                        lines.push(line);
                        continue;
                    }
                    _ => {
                        empty += 1;
                        continue;
                    }
                },
            }
            if let Some(m) = merge {
                if (m.top, m.left) == (r, c) {
                    line.push_str(&format!("\t[merged {}]", range_a1(m)));
                } else {
                    line.push_str(&format!("\t[merged into {}]", a1(m.top, m.left)));
                }
            }
            lines.push(line);
        }
    }
    let head = format!(
        "{} · source values ({} cells, {empty} empty not listed)",
        sheet_ref(&s.name, &range_a1(rect)),
        area
    );
    let mut out = vec![head];
    out.extend(lines);
    Ok(out.join("\n"))
}
