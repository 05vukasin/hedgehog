//! Merged parameter captions apply to every table in their section, with exact provenance.
use hh_core::{
    normalize::{Options, normalize_workbook},
    open,
};
use rust_xlsxwriter::{Format, Workbook};

#[test]
fn merged_parameter_caption_scopes_all_stacked_tables_until_next_caption() {
    let path = std::env::temp_dir().join(format!("hh_caption_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    for (top, time) in [(0, "Time = 0 min"), (18, "Time = 75 min")] {
        ws.merge_range(top, 0, top, 10, time, &Format::new())
            .unwrap();
        // Unrelated nonnumeric metadata in a separate lateral block must not spread.
        ws.merge_range(
            top,
            13,
            top,
            17,
            if top == 0 {
                "Time = 5 min"
            } else {
                "Time = 10 min"
            },
            &Format::new(),
        )
        .unwrap();
        ws.merge_range(top, 19, top, 23, "Formula = x + y", &Format::new())
            .unwrap();
        for (offset, roi) in [(1, 1), (9, 3)] {
            for (left, label) in [
                (0u16, roi),
                (6u16, roi + 1),
                (13u16, roi + 6),
                (19u16, roi + 8),
            ] {
                ws.merge_range(
                    top + offset,
                    left,
                    top + offset,
                    left + 4,
                    format!("ROI {label}").as_str(),
                    &Format::new(),
                )
                .unwrap();
                for (c, h) in ["Measure", "M0", "IFN-y", "LPS", "M2"].iter().enumerate() {
                    ws.write_string(top + offset + 1, left + c as u16, *h)
                        .unwrap();
                }
                for r in 0..4 {
                    ws.write_string(top + offset + 2 + r, left, format!("Count {r}"))
                        .unwrap();
                    for c in 1..5 {
                        ws.write_number(
                            top + offset + 2 + r,
                            left + c,
                            (100 * label + 10 * r + c as u32) as f64,
                        )
                        .unwrap();
                    }
                }
            }
        }
    }
    wb.save(&path).unwrap();
    let book = open(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    for rules_only in [true, false] {
        let norm = normalize_workbook(
            &book,
            &Options {
                rules_only,
                ..Options::default()
            },
        );
        assert!(
            norm.tables.len() >= 4,
            "{:?}",
            norm.tables.iter().map(|t| t.rect).collect::<Vec<_>>()
        );
        let mut answers = 0;
        let mut lateral = 0;
        for t in &norm.tables {
            if t.rect.left >= 13 {
                lateral += 1;
                if t.rect.left < 19 {
                    let expected = if t.rect.top < 18 {
                        "Time = 5 min"
                    } else {
                        "Time = 10 min"
                    };
                    assert!(
                        t.title.as_ref().is_some_and(|(s, _)| s == expected)
                            || t.notes.iter().any(|(s, _)| s == expected)
                    );
                    assert!(
                        !t.notes
                            .iter()
                            .any(|(s, _)| s == "Time = 0 min" || s == "Time = 75 min")
                    );
                } else {
                    assert!(!t.notes.iter().any(|(s, _)| s.starts_with("Time =")));
                    if (9..18).contains(&t.rect.top) || t.rect.top >= 27 {
                        assert!(!t.notes.iter().any(|(s, _)| s == "Formula = x + y"));
                    }
                }
                continue;
            }
            let (expected, source) = if t.rect.top < 18 {
                ("Time = 0 min", (0, 0))
            } else {
                ("Time = 75 min", (18, 0))
            };
            assert!(
                t.title
                    .as_ref()
                    .is_some_and(|(s, rc)| s == expected && *rc == source)
                    || t.notes.iter().any(|(s, rc)| s == expected && *rc == source),
                "rules={rules_only}, table {:?}, title {:?}, notes {:?}: missing {expected}",
                t.rect,
                t.title,
                t.notes
            );
            let wrong = if t.rect.top < 18 {
                "Time = 75 min"
            } else {
                "Time = 0 min"
            };
            assert!(!t.notes.iter().any(|(s, _)| s == wrong));
            // The caption identifies the block containing the answer; values stay exact.
            if t.rect.top >= 18 {
                let answer = t.columns.iter().find(|c| c.source_col == Some(3));
                if let Some(answer) = answer {
                    let row = t.src_rows.iter().position(|&r| r == 22 || r == 30);
                    if let Some(row) = row {
                        let roi = if t.src_rows[row] == 22 { 1 } else { 3 };
                        answers += 1;
                        assert_eq!(
                            answer.values[row],
                            hh_core::normalize::Out::Num((100 * roi + 13) as f64)
                        );
                    }
                }
            }
        }
        assert_eq!(
            answers, 2,
            "both caption-scoped ROI answers must be checked"
        );
        assert!(lateral >= 2, "negative lateral tables must be exercised");
    }
}
