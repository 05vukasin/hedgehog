//! Merged parameter captions apply to every table in their section, with exact provenance.
use hh_core::{
    normalize::{Options, normalize_workbook},
    open,
};
use rust_xlsxwriter::{Format, Workbook};

#[test]
fn merged_parameter_caption_scopes_all_stacked_tables_until_next_caption() {
    caption_lateral_fixture(true);
}

#[test]
fn unmerged_parameter_caption_does_not_cross_lateral_parameter_sections() {
    caption_lateral_fixture(false);
}

fn caption_lateral_fixture(merged: bool) {
    let path =
        std::env::temp_dir().join(format!("hh_caption_{merged}_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    for (top, time) in [(0, "Time = 0 min"), (18, "Time = 75 min")] {
        if merged {
            ws.merge_range(top, 0, top, 10, time, &Format::new())
                .unwrap();
        } else {
            ws.write_string(top, 0, time).unwrap();
        }
        let lateral = if top == 0 {
            "Time = 5 min"
        } else {
            "Time = 10 min"
        };
        if merged {
            ws.merge_range(top, 13, top, 17, lateral, &Format::new())
                .unwrap();
        } else {
            ws.write_string(top, 13, lateral).unwrap();
        }
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
                    if merged {
                        assert!(
                            t.title.as_ref().is_some_and(|(s, _)| s == expected)
                                || t.notes.iter().any(|(s, _)| s == expected)
                        );
                    }
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

/// RealHiTBench science-table09/10: Time is an unmerged sparse caption, above
/// three pairs of merged ROI captions. The first stub header is empty.
#[test]
fn unmerged_parameter_caption_uses_repeated_child_layout() {
    let path =
        std::env::temp_dir().join(format!("hh_unmerged_caption_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.merge_range(0, 0, 0, 23, "", &Format::new()).unwrap();
    let metrics = [
        "Number of M1 per FOV",
        "Number of M1 phagocytosising",
        "Number of Engulfed Cells",
        "Phagocytosis Index",
    ];
    for (block, top) in [1, 23, 45].into_iter().enumerate() {
        ws.write_string(top, 0, format!("Time = {} min", [0, 30, 75][block]))
            .unwrap();
        for (pair, offset) in [1, 8, 15].into_iter().enumerate() {
            for left in [0u16, 6] {
                let roi = pair * 2 + if left == 0 { 1 } else { 2 };
                ws.merge_range(
                    top + offset,
                    left,
                    top + offset,
                    left + 4,
                    &format!("ROI {roi}"),
                    &Format::new(),
                )
                .unwrap();
                for (c, label) in ["M0", "IFN-y", "LPS", "M2"].into_iter().enumerate() {
                    ws.write_string(top + offset + 1, left + c as u16 + 1, label)
                        .unwrap();
                }
                for (r, metric) in metrics.iter().enumerate() {
                    ws.write_string(top + offset + 2 + r as u32, left, *metric)
                        .unwrap();
                    for c in 1..5 {
                        ws.write_number(
                            top + offset + 2 + r as u32,
                            left + c,
                            (block * 1000 + roi * 100 + r * 10 + c as usize) as f64,
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
        let mut answers = 0;
        for t in &norm.tables {
            let (expected, at) = if t.rect.top < 23 {
                ("Time = 0 min", (1, 0))
            } else if t.rect.top < 45 {
                ("Time = 30 min", (23, 0))
            } else {
                ("Time = 75 min", (45, 0))
            };
            assert!(
                t.title
                    .as_ref()
                    .is_some_and(|(s, rc)| s == expected && *rc == at)
                    || t.notes.iter().any(|(s, rc)| s == expected && *rc == at),
                "rules={rules_only}, table {:?}, title {:?}, notes {:?}: missing {expected}",
                t.rect,
                t.title,
                t.notes
            );
            assert!(
                !t.notes
                    .iter()
                    .any(|(s, rc)| s.starts_with("Time =") && *rc != at)
            );
            if let Some(row) = t.src_rows.iter().position(|&r| r == 57)
                && let Some(col) = t.columns.iter().find(|c| c.source_col == Some(3))
            {
                assert_eq!(col.values[row], hh_core::normalize::Out::Num(2323.0));
                answers += 1;
            }
        }
        assert_eq!(answers, 1, "the Time=75, ROI3, LPS answer must be checked");
    }
}

#[test]
fn unmerged_caption_does_not_spread_without_repeated_child_geometry_and_labels() {
    for change_geometry in [false, true] {
        let path = std::env::temp_dir().join(format!(
            "hh_caption_mismatch_{change_geometry}_{}.xlsx",
            std::process::id()
        ));
        let mut wb = Workbook::new();
        let ws = wb.add_worksheet();
        for top in [0, 20] {
            ws.write_string(top, 0, format!("Time = {top} min"))
                .unwrap();
            for offset in [1, 9] {
                for left in [0u16, 6] {
                    let end = left
                        + if change_geometry && top == 20 && offset == 1 && left == 6 {
                            3
                        } else {
                            4
                        };
                    let label = if !change_geometry && top == 20 && offset == 1 && left == 6 {
                        "Different group"
                    } else if left == 0 {
                        "ROI 1"
                    } else {
                        "ROI 2"
                    };
                    ws.merge_range(top + offset, left, top + offset, end, label, &Format::new())
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
                                (100 + 10 * r + c as u32) as f64,
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
        for rules_only in [false, true] {
            let norm = normalize_workbook(
                &book,
                &Options {
                    rules_only,
                    ..Options::default()
                },
            );
            let later: Vec<_> = norm
                .tables
                .iter()
                .filter(|t| (9..18).contains(&t.rect.top))
                .collect();
            assert!(!later.is_empty(), "negative case must exercise descendants");
            assert!(
                later
                    .iter()
                    .all(|t| !t.notes.iter().any(|(s, _)| s.starts_with("Time ="))),
                "geometry={change_geometry}, rules={rules_only}"
            );
        }
    }
}

#[test]
fn parameter_caption_of_different_width_still_ends_previous_section() {
    let path =
        std::env::temp_dir().join(format!("hh_caption_boundary_{}.xlsx", std::process::id()));
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    for top in [0, 20] {
        ws.write_string(top, 0, format!("Time = {top} min"))
            .unwrap();
        for offset in [1, 9] {
            for left in [0u16, 6] {
                ws.merge_range(
                    top + offset,
                    left,
                    top + offset,
                    left + 4,
                    if left == 0 { "ROI 1" } else { "ROI 2" },
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
                            (100 + 10 * r + c as u32) as f64,
                        )
                        .unwrap();
                    }
                }
            }
        }
    }
    // A valid new caption has a different span and no repeated peer. It still ends Time=0.
    ws.merge_range(8, 0, 8, 5, "Time = 10 min", &Format::new())
        .unwrap();
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
        let later: Vec<_> = norm
            .tables
            .iter()
            .filter(|t| (8..20).contains(&t.rect.top))
            .collect();
        assert!(!later.is_empty());
        assert!(
            later
                .iter()
                .all(|t| !t.notes.iter().any(|(s, _)| s == "Time = 0 min")),
            "rules={rules_only}: stale caption passed a different-width boundary"
        );
    }
}
