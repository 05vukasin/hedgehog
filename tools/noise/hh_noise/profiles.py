"""Noise profiles: operator probabilities and size/language distributions.

Probability scopes: stage A/B operators are drawn per table; stage C/D per sheet; `multi_sheet`
and `empty_sheet` per workbook. `enron_like` is calibrated against DECO (§0 of
docs/research/irregularities.md) and the Enron profile; see `python -m hh_noise calibrate`.
"""
from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class Profile:
    name: str
    p: dict[str, float]
    lang: dict[str, float]
    num_style: dict[str, float] = field(default_factory=lambda: {"us": 1.0})
    rows_median: float = 18.0
    rows_sigma: float = 0.8
    cols: tuple[int, int] = (2, 8)
    max_tables: int = 4
    # arrangement weights when a sheet holds several tables
    arrangement: dict[str, float] = field(default_factory=lambda: {"stacked": 0.45, "stack_schema_change": 0.35, "side_by_side": 0.2})


BASE = {
    # stage A (per table)
    "missing_tokens": 0.35, "numbers_as_text": 0.10, "locale_numbers": 0.10, "currency_units": 0.10,
    "percent_text": 0.15, "negative_formats": 0.15, "dates_as_text": 0.25, "date_serials": 0.05,
    "whitespace_noise": 0.30, "case_noise": 0.15, "categorical_variants": 0.10, "typos": 0.10,
    "scale_thousands": 0.06, "value_drift": 0.05, "footnote_markers": 0.05, "excel_errors": 0.04,
    "bool_variants": 0.7, "header_rename": 0.30,
    # stage B (per table)
    "crosstab": 0.08, "row_label_column": 0.05, "group_header_rows": 0.12, "group_label_column": 0.05,
    "subtotals": 0.15, "grand_total_after_subtotals": 0.7, "grand_total": 0.30, "blank_repeats": 0.10,
    "vertical_merge_keys": 0.05, "total_column": 0.08, "wrong_total": 0.06, "totals_as_values": 0.35,
    "multirow_header": 0.26, "wrapped_header": 0.08, "transpose": 0.03, "spacer_rows": 0.06,
    "spacer_cols": 0.05, "repeat_header": 0.03,
    # stage C (per sheet)
    "offset": 0.7, "title_block": 0.50, "table_caption": 0.5, "notes_block": 0.20, "multi_table": 0.20,
    "stray_cells": 0.12,
    # stage D (per sheet)
    "style_header": 0.75, "style_totals": 0.6, "number_formats": 0.7, "column_widths": 0.6,
    "hidden_rows_cols": 0.08, "freeze_panes": 0.15, "autofilter": 0.08,
    # workbook
    "multi_sheet": 0.3, "empty_sheet": 0.15,
}


def _mk(name, overrides, **kw) -> Profile:
    p = dict(BASE)
    p.update(overrides)
    return Profile(name=name, p=p, **kw)


PROFILES = {
    "enron_like": _mk("enron_like", {
        # calibrated against DECO §0 + Enron profile (see docs/noise.md, calibration table)
        "multirow_header": 0.42, "wrapped_header": 0.12, "grand_total": 0.35, "subtotals": 0.38,
        "group_header_rows": 0.25, "group_label_column": 0.12, "row_label_column": 0.12,
        "numbers_as_text": 0.04, "locale_numbers": 0.03, "currency_units": 0.04, "percent_text": 0.05,
        "footnote_markers": 0.03, "whitespace_noise": 0.5, "totals_as_values": 0.15, "hidden_rows_cols": 0.2,
        "stray_cells": 0.2, "dates_as_text": 0.08,
    }, lang={"en": 1.0}, num_style={"us": 0.9, "plain": 0.1},
                      arrangement={"stacked": 0.4, "stack_schema_change": 0.4, "side_by_side": 0.2}),
    "gov_stats": _mk("gov_stats", {
        "title_block": 0.95, "notes_block": 0.8, "multirow_header": 0.55, "wrapped_header": 0.10,
        "footnote_markers": 0.30, "scale_thousands": 0.30, "group_header_rows": 0.25, "group_label_column": 0.10,
        "subtotals": 0.25, "grand_total": 0.45, "crosstab": 0.25, "missing_tokens": 0.5, "multi_table": 0.08,
        "row_label_column": 0.10, "stray_cells": 0.03, "header_rename": 0.4, "spacer_rows": 0.08,
    }, lang={"en": 0.45, "sr": 0.2, "de": 0.1, "fr": 0.1, "es": 0.05, "it": 0.05, "nl": 0.05},
        num_style={"us": 0.45, "eu": 0.3, "fr": 0.1, "fr_space": 0.1, "ch": 0.05},
        arrangement={"stacked": 0.3, "stack_schema_change": 0.6, "side_by_side": 0.1}),
    "eu_locale": _mk("eu_locale", {
        "locale_numbers": 0.45, "currency_units": 0.25, "dates_as_text": 0.45, "percent_text": 0.3,
        "numbers_as_text": 0.15, "scale_thousands": 0.1,
    }, lang={"de": 0.25, "fr": 0.2, "es": 0.15, "it": 0.15, "nl": 0.1, "sr": 0.15},
        num_style={"eu": 0.55, "fr": 0.15, "fr_space": 0.1, "fr_nnbsp": 0.05, "ch": 0.1, "plain_comma": 0.05}),
    "clean_ish": _mk("clean_ish", {k: v * 0.25 for k, v in BASE.items() if k not in ("bool_variants", "style_header", "number_formats", "grand_total_after_subtotals")}
                     | {"offset": 0.3, "title_block": 0.15, "multi_table": 0.05, "multi_sheet": 0.1},
                     lang={"en": 0.8, "de": 0.1, "fr": 0.1}, num_style={"us": 0.9, "eu": 0.1}),
}

# Corpus mix used by `generate --mix default`.
DEFAULT_MIX = {"enron_like": 0.5, "gov_stats": 0.2, "eu_locale": 0.2, "clean_ish": 0.1}

# Calibration targets (DECO, docs/research/irregularities.md §0). Units: share of sheets with ≥1
# table, or share of tables.
TARGETS = {
    "sheet_title": 0.548,
    "table_multirow_header": 0.347,
    "sheet_multi_table": 0.200,
    "sheet_total": 0.427,
    "table_midtable_total": 0.123,
    "sheet_notes": 0.233,
    "sheet_group_header": 0.169,
}
