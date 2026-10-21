//! Change-point segmentation of a block's rows: stacked tables with different schemas, also
//! without a blank row or a second header.
//!
//! Every row gets a sparse signature: per column a value class (empty, short/long text, date,
//! integer, decimal), computed with [`crate::values`] so numbers and dates stored as text count as
//! numbers and dates. For a cut `t` the rows above and below are compared column by column
//! (total-variation distance of the class mix, weighted by how filled the column is). Binary
//! segmentation picks the cut with the best margin over its acceptance threshold:
//! a strong type-profile change on its own, or a weaker one that coincides with a header-like row,
//! a blank gap, a total row just above, or a change of the filled column range.
//! A row repeating the table's header (page break) is never a cut by itself.
//!
//! Cost: O(rows × distinct columns) per recursion level, memory O(cells + columns).

use std::collections::HashMap;

use crate::model::{Sheet, Value};
use crate::values::{parse_date, parse_number, parse_time, NumberHint};

use super::blocks::{Entry, Row};
use super::cells::Kind;
use super::params::*;
use super::tables::{is_header_row, is_headerish, is_title_row_simple, is_total_row, signature, Signature};

const K: usize = 6;
const EMPTY: u8 = 0;
const TEXT: u8 = 1;
const LONG: u8 = 2;
const DATE: u8 = 3;
const INT: u8 = 4;
const DEC: u8 = 5;

fn coarse(k: usize) -> usize {
    match k as u8 {
        EMPTY => 0,
        TEXT | LONG => 1,
        DATE => 2,
        _ => 3,
    }
}

fn class_of(sheet: &Sheet, e: &Entry) -> u8 {
    let Some(i) = e.cell else { return EMPTY };
    if matches!(e.kind, Kind::Missing | Kind::Blank | Kind::Rule) {
        return EMPTY;
    }
    let c = &sheet.cells[i];
    match &c.value {
        Some(Value::Number(n)) => {
            if n.fract() == 0.0 {
                INT
            } else {
                DEC
            }
        }
        Some(Value::DateTime(_)) => DATE,
        // Plain numeric text ("1,200.50", "(500)", "01/02/2001"): classed by its characters.
        Some(Value::Text(s)) if e.kind == Kind::Num => {
            if s.contains('/') {
                DATE
            } else if s.contains(['.', ',']) {
                DEC
            } else {
                INT
            }
        }
        Some(Value::Text(s)) => {
            let t = s.trim();
            // Only short, digit-heavy text can be a decorated number or a date ("1.234,50 EUR",
            // "Jan 5, 2001"); addresses and descriptions skip the parsers.
            if t.len() <= 32 {
                let (digits, alnum) = t.chars().fold((0usize, 0usize), |(d, a), ch| {
                    (d + ch.is_ascii_digit() as usize, a + ch.is_alphanumeric() as usize)
                });
                if digits > 0 && digits * 2 >= alnum {
                    if let Some(p) = parse_number(t, NumberHint::default()) {
                        return if p.normalized().fract() == 0.0 { INT } else { DEC };
                    }
                    if parse_date(t, None).is_some() || parse_time(t).is_some() {
                        return DATE;
                    }
                }
                TEXT
            } else if t.chars().count() >= CP_LONG_TEXT {
                LONG
            } else {
                TEXT
            }
        }
        Some(_) => EMPTY,
        None if c.formula.is_some() => DEC,
        None => EMPTY,
    }
}

struct Sigs {
    /// Per row (relative to `a`): (column index, class), filled cells only.
    rows: Vec<Vec<(u16, u8)>>,
    /// Data rows: header-like rows and totals are left out of the profiles.
    data: Vec<bool>,
    ncols: usize,
    /// Sheet column of each column index.
    col_of: Vec<u32>,
}

fn signatures(sheet: &Sheet, rows: &[Row]) -> Option<Sigs> {
    // Column → index: a flat table over the block's column span (blocks are at most a few
    // thousand columns wide; wider ones fall back to a map).
    let lo = rows.iter().map(|r| r.left()).min().unwrap_or(0);
    let hi = rows.iter().map(|r| r.right()).max().unwrap_or(0);
    let mut flat: Vec<u16> = if hi - lo < 4096 { vec![u16::MAX; (hi - lo + 1) as usize] } else { Vec::new() };
    let mut idx: HashMap<u32, u16> = HashMap::new();
    let mut ncols = 0usize;
    let mut col_of = Vec::new();
    let mut out = Vec::with_capacity(rows.len());
    let mut data = Vec::with_capacity(rows.len());
    for r in rows {
        let mut v = Vec::with_capacity(r.entries.len());
        let classes: Vec<u8> = r.entries.iter().map(|e| class_of(sheet, e)).collect();
        // Data row: holds a measure (years don't count), or a date or two with at most as many
        // texts (a row of many dates across the columns is a period header), or is not
        // header-like at all. Totals are left out too.
        let (mut measures, mut dates, mut texts) = (0, 0, 0);
        for (e, &k) in r.entries.iter().zip(&classes) {
            match k {
                INT | DEC if !(e.label_num && !e.date) => measures += 1,
                DATE => dates += 1,
                TEXT | LONG => texts += 1,
                _ => {}
            }
        }
        let d = (measures > 0 || ((1..=2).contains(&dates) && dates >= texts) || !is_headerish(r))
            && !is_total_row(sheet, r);
        data.push(d);
        if !d {
            out.push(v);
            continue;
        }
        for (e, &k) in r.entries.iter().zip(&classes) {
            if k == EMPTY {
                continue;
            }
            let slot = if flat.is_empty() {
                idx.entry(e.left).or_insert(u16::MAX)
            } else {
                &mut flat[(e.left - lo) as usize]
            };
            if *slot == u16::MAX {
                if ncols >= CP_MAX_COLS {
                    return None;
                }
                *slot = ncols as u16;
                col_of.push(e.left);
                ncols += 1;
            }
            v.push((*slot, k));
        }
        out.push(v);
    }
    Some(Sigs { rows: out, data, ncols, col_of })
}

/// Row indices (into `rows`) in `a+1..b` where a new table starts.
pub(crate) fn boundaries(sheet: &Sheet, rows: &[Row], a: usize, b: usize, header: &Signature) -> Vec<usize> {
    let n = b.saturating_sub(a);
    if n < 2 * CP_MIN_ROWS {
        return Vec::new();
    }
    let Some(sigs) = signatures(sheet, &rows[a..b]) else { return Vec::new() };
    if sigs.ncols == 0 || n.saturating_mul(sigs.ncols) > CP_MAX_WORK {
        return Vec::new();
    }
    let mut cuts = Vec::new();
    let mut stack = vec![(a, b)];
    while let Some((lo, hi)) = stack.pop() {
        if cuts.len() >= CP_MAX_CUTS {
            break;
        }
        if let Some(t) = best_cut(sheet, rows, &sigs, a, lo, hi, header) {
            cuts.push(t);
            stack.push((lo, t));
            stack.push((t, hi));
        }
    }
    cuts.sort_unstable();
    cuts
}


#[allow(clippy::too_many_arguments)]
fn best_cut(
    sheet: &Sheet,
    rows: &[Row],
    sigs: &Sigs,
    base: usize,
    lo: usize,
    hi: usize,
    header: &Signature,
) -> Option<usize> {
    let n = hi - lo;
    if n < 2 * CP_MIN_ROWS {
        return None;
    }
    let nc = sigs.ncols;
    let mut total = vec![0u32; nc * K];
    for r in &sigs.rows[lo - base..hi - base] {
        for &(c, k) in r {
            total[c as usize * K + k as usize] += 1;
        }
    }
    // Profiles are over data rows only; each side needs `CP_MIN_ROWS` of them.
    let mut data_before = vec![0usize; n + 1];
    for j in 0..n {
        data_before[j + 1] = data_before[j] + sigs.data[lo - base + j] as usize;
    }
    let mut above = vec![0u32; nc * K];
    let mut cands: Vec<(usize, f32, f32)> = Vec::new();
    let mut ranges: Vec<(usize, bool)> = Vec::new();
    for t in lo + 1..hi {
        for &(c, k) in &sigs.rows[t - 1 - base] {
            above[c as usize * K + k as usize] += 1;
        }
        let (da, db) = (data_before[t - lo], data_before[n] - data_before[t - lo]);
        if da < CP_MIN_ROWS || db < CP_MIN_ROWS {
            continue;
        }
        let (na, nb) = (da as f32, db as f32);
        // `all`: every column, empty included (shape + content); `shared`: content of the
        // columns filled on both sides only.
        let (mut num, mut den, mut snum, mut sden) = (0f32, 0f32, 0f32, 0f32);
        // Filled column range on each side (columns at least half filled).
        let (mut ra, mut rb) = ((u32::MAX, 0u32), (u32::MAX, 0u32));
        for c in 0..nc {
            let ca = &above[c * K..c * K + K];
            let ct = &total[c * K..c * K + K];
            let fa: u32 = ca.iter().sum();
            let fb: u32 = ct.iter().sum::<u32>() - fa;
            if fa == 0 && fb == 0 {
                continue;
            }
            let (pa, pb) = (fa as f32 / na, fb as f32 / nb);
            let col = sigs.col_of[c];
            if pa >= 0.5 {
                ra = (ra.0.min(col), ra.1.max(col));
            }
            if pb >= 0.5 {
                rb = (rb.0.min(col), rb.1.max(col));
            }
            let de = pb - pa;
            let (mut fine, mut cs) = (de.abs(), [de, 0.0, 0.0, 0.0]);
            let (mut sfine, mut scs) = (0f32, [0f32; 4]);
            for k in 1..K {
                let (xa, xb) = (ca[k] as f32, (ct[k] - ca[k]) as f32);
                let d = xa / na - xb / nb;
                fine += d.abs();
                cs[coarse(k)] += d;
                if fa > 0 && fb > 0 {
                    let d = xa / fa as f32 - xb / fb as f32;
                    sfine += d.abs();
                    scs[coarse(k)] += d;
                }
            }
            let tv = |cs: [f32; 4], fine: f32| {
                let coarse_tv = 0.5 * cs.iter().map(|x| x.abs()).sum::<f32>();
                CP_COARSE_WEIGHT * coarse_tv + (1.0 - CP_COARSE_WEIGHT) * 0.5 * fine
            };
            let w = pa.max(pb);
            num += w * tv(cs, fine);
            den += w;
            let ws = pa.min(pb);
            if ws >= CP_SHARED_MIN_FILL {
                snum += ws * tv(scs, sfine);
                sden += ws;
            }
        }
        if den == 0.0 {
            continue;
        }
        let m = da.min(db) as f32;
        let shrink = if m >= CP_SHRINK_ROWS { 1.0 } else { m / (m + CP_SHRINK) };
        let dist = num / den * shrink;
        let shared = if sden > 0.0 { snum / sden * shrink } else { dist };
        ranges.push((t, ra.0 != u32::MAX && rb.0 != u32::MAX && (ra.0 != rb.0 || ra.1 != rb.1)));
        cands.push((t, dist, shared));
    }
    // The cut goes where the profiles differ most; a header row or blank gap just next to it
    // may take it (headers sit on the boundary and blur it by a row or two).
    let &(t0, d0, _) = cands.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
    if d0 < CP_WEAK {
        return None;
    }
    // A blank gap is a cue only when it is wider than the block's usual spacing (tables with a
    // blank row after every row or group gain nothing from it).
    let mut gaps: Vec<u32> = (lo + 1..hi).map(|t| gap_len(rows, t)).collect();
    gaps.sort_unstable();
    let usual = gaps[(gaps.len() * 3) / 4];
    let mut best: Option<(f32, usize)> = None;
    for &(t, dist, shared) in cands.iter().filter(|c| c.0.abs_diff(t0) <= CP_SNAP) {
        let r = &rows[t];
        if dist < CP_WEAK || is_total_row(sheet, r) {
            continue;
        }
        let gap = gap_len(rows, t) > usual;
        let repeated = !header.is_empty() && r.n() >= 2 && signature(sheet, r) == *header;
        if repeated && !gap {
            continue;
        }
        let head = (r.n() >= 2 && !same_types(r, &rows[t - 1]) && is_header_row(r, &rows[t + 1..hi]))
            || (is_title_row_simple(sheet, r) && t + 1 < hi && is_header_row(&rows[t + 1], &rows[t + 2..hi]));
        let total_above = is_total_row(sheet, &rows[t - 1]);
        let range_change = ranges.iter().find(|x| x.0 == t).is_some_and(|x| x.1);
        let weak_cue = gap || total_above || range_change;
        // A new header row explains a shape change; otherwise the shared columns must change
        // content too (optional columns filling in later are not a new table).
        let ok = if head {
            dist >= CP_WEAK
        } else if weak_cue {
            dist.min(shared) >= CP_WEAK
        } else {
            dist.min(shared) >= CP_STRONG
        };
        // Prefer cued cuts (header/gap) over the raw maximum.
        let score = dist + if head || gap { 1.0 } else { 0.0 };
        if ok && best.is_none_or(|(bs, _)| score > bs) {
            best = Some((score, t));
        }
    }
    best.map(|(_, t)| t)
}

/// Every filled cell of `r` has a cell of the same fine type in the same column of `prev`
/// (a data row like the one above, not a header).
fn same_types(r: &Row, prev: &Row) -> bool {
    r.n() == prev.n()
        && r.filled().all(|e| prev.filled().any(|p| p.left == e.left && p.ty() == e.ty()))
}

/// Blank rows between `rows[t - 1]` and `rows[t]`.
fn gap_len(rows: &[Row], t: usize) -> u32 {
    rows[t].row.saturating_sub(rows[t - 1].bottom() + 1)
}
