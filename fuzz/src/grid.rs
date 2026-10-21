//! The `normalize` target: bytes → a small workbook built straight in the model (no file), so the
//! fuzzer drives the layout, header, value and totals logic with grids no reader would produce.

use hh_core::model::{Cell, Rect, Sheet, Value, Workbook};

const TEXTS: &[&str] = &[
    "", " ", "Name", "City", "Region", "Year", "Month", "Date", "Amount", "Budget", "Price", "Qty", "Total", "TOTAL",
    "Subtotal", "Ukupno", "Svega", "Summe", "Gesamt", "Total général", "Sum", "of which", "Note: preliminary",
    "Source: statistics office", "2023", "2024", "Q1", "Q2 2023", "Jan", "January", "jan. 2023", "31.12.2023",
    "2023-01-31", "1/2/2023", "13/13/2023", "1.234,56", "1,234.56", "(1,234)", "-", "–", "n/a", "#N/A", "12%",
    "€ 1.000", "$1,000.00", "1 000 000", "0,5", "TRUE", "x", "*", "…", "Beograd", "Niš", "Zürich", "東京",
    "a very long description text that goes on and on and mentions the total of everything in 2022",
];

const FORMULAS: &[&str] = &["SUM(B2:B5)", "SUBTOTAL(9,C2:C9)", "A1+B1", "B2*1.2", "SUM(B:B)", "AVERAGE(A1:Z99)", "#REF!+1", ""];

struct Bytes<'a>(&'a [u8], usize);

impl Bytes<'_> {
    fn u8(&mut self) -> u8 {
        let b = self.0.get(self.1).copied().unwrap_or(0);
        self.1 += 1;
        b
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { self.u8() as usize % n }
    }
    fn done(&self) -> bool {
        self.1 >= self.0.len()
    }
}

/// Up to 3 sheets of up to 60 × 14 cells with numbers, labels, dates, formulas and merges.
pub fn workbook(data: &[u8]) -> Workbook {
    let mut b = Bytes(data, 0);
    let n_sheets = 1 + b.below(3);
    let mut sheets = Vec::new();
    for s in 0..n_sheets {
        let rows = 1 + b.below(60) as u32;
        let cols = 1 + b.below(14) as u32;
        // a sparse offset so the used range does not start at A1
        let (r0, c0) = (b.below(4) as u32 * if b.u8() > 250 { 100_000 } else { 1 }, b.below(3) as u32);
        let mut cells = Vec::new();
        for r in 0..rows {
            for c in 0..cols {
                if b.done() {
                    break;
                }
                let k = b.u8();
                let value = match k % 16 {
                    0..=3 => None,
                    4..=7 => Some(Value::Number(number(&mut b))),
                    8..=11 => Some(Value::Text(TEXTS[b.below(TEXTS.len())].to_string())),
                    12 => Some(Value::DateTime(36_000.0 + b.u8() as f64 * 31.0 + b.u8() as f64 / 256.0)),
                    13 => Some(Value::Bool(b.u8() & 1 == 1)),
                    14 => Some(Value::Error("#DIV/0!".into())),
                    _ => Some(Value::Text(format!("{} {}", TEXTS[b.below(TEXTS.len())], b.u8()))),
                };
                let formula = (k >= 0xf0).then(|| FORMULAS[b.below(FORMULAS.len())].to_string());
                if value.is_none() && formula.is_none() {
                    continue;
                }
                cells.push(Cell { row: r0 + r, col: c0 + c, value, formula });
            }
        }
        let mut merges = Vec::new();
        for _ in 0..b.below(5) {
            let top = r0 + b.below(rows as usize + 2) as u32;
            let left = c0 + b.below(cols as usize + 2) as u32;
            // occasionally inverted or huge
            let (bottom, right) = match b.u8() {
                0..=200 => (top + b.below(3) as u32, left + b.below(4) as u32),
                201..=230 => (top, left), // single cell
                _ => (top + 1_000_000, left + 16_000),
            };
            merges.push(Rect { top, left, bottom, right });
        }
        sheets.push(Sheet { name: format!("S{s}"), cells, merges, formats: Vec::new() });
    }
    Workbook { sheets }
}

fn number(b: &mut Bytes) -> f64 {
    match b.u8() % 12 {
        0 => 0.0,
        10 => f64::NAN,
        11 => f64::INFINITY,
        1 => -(b.u8() as f64),
        2 => b.u8() as f64 * 1000.0 + b.u8() as f64,
        3 => (b.u8() as f64) / 7.0,
        4 => 1e15 * b.u8() as f64,
        5 => f64::MAX,
        6 => f64::MIN_POSITIVE,
        7 => 1900.0 + b.below(200) as f64,
        8 => -0.0,
        _ => b.u8() as f64,
    }
}
