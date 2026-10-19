"""Workbook generation: picks sources, builds tables (stages A/B), lays out sheets (stage C),
styles them (stage D) and hands the drafts to the renderer."""
from __future__ import annotations

import datetime as dt
import random
import re
from dataclasses import dataclass
from pathlib import Path

from . import ops_structure as S
from . import ops_value as V
from .model import Block, Cell, Loose, Placed, SheetDraft
from .profiles import Profile
from .sources import Source, cached_source
from .table import crosstab_block, is_measure, pick_rows, sample_block
from .vocab import (AS_OF, MONTHS, MONTHS_ABBR, NOTE, NOTE_TEXT, ORGS, PEOPLE, PREPARED, SCALE, SOURCE, SOURCES, STRAY,
                    TABLE_CAPTION, TITLE_PREFIX)

GENERATOR_VERSION = "hh_noise/0.1"


@dataclass
class Ctx:
    rng: random.Random
    prof: Profile
    lang: str
    num_style: str

    def p(self, name: str) -> bool:
        return self.rng.random() < self.prof.p.get(name, 0.0)


def _weighted(rng, d: dict):
    keys = list(d)
    return rng.choices(keys, weights=[d[k] for k in keys])[0]


# ---------------------------------------------------------------- tables (stages A + B)

VALUE_ORDER = ["bool_variants", "scale_thousands", "missing_tokens", "excel_errors", "percent_text", "currency_units",
               "negative_formats", "locale_numbers", "numbers_as_text", "dates_as_text", "date_serials",
               "categorical_variants", "case_noise", "typos", "whitespace_noise", "footnote_markers", "value_drift",
               "header_rename"]


def build_table(ctx: Ctx, src: Source, n_rows: int | None = None, n_cols: int | None = None,
                allow: set[str] | None = None, deny: set[str] = frozenset()) -> Block | None:
    rng = ctx.rng
    prof = ctx.prof

    def want(op):
        return op not in deny and (allow is None or op in allow) and ctx.p(op)

    months = MONTHS_ABBR[ctx.lang] if rng.random() < 0.7 else MONTHS[ctx.lang]
    b = None
    if want("crosstab"):
        b = crosstab_block(src, rng, ctx.lang, months)
    if b is None:
        nr = n_rows or pick_rows(rng, prof.rows_median, prof.rows_sigma)
        nc = n_cols or rng.randint(*prof.cols)
        b = sample_block(src, rng, nr, nc, ctx.lang)
    if b is None:
        return None
    is_x = any(o["op"] == "crosstab" for o in b.ops)
    # stage B (structure on clean values)
    if not is_x and want("row_label_column"):
        S.row_label_column(b, rng, ctx)
    if not is_x and "grouping" not in deny:
        S.grouping(b, rng, _Gate(ctx, allow, deny))
    if "total" not in b.row_kind and want("grand_total"):
        S.grand_total(b, rng, ctx)
    if want("total_column"):
        S.total_column(b, rng, ctx)
    if want("multirow_header") or (is_x and rng.random() < 0.35 and "multirow_header" not in deny):
        if is_x and b.n_cols >= 3:
            _crosstab_parent(b)
        else:
            S.multirow_header(b, rng, ctx)
    elif want("wrapped_header"):
        S.wrapped_header(b, rng, ctx)
    # stage A (values)
    for op in VALUE_ORDER:
        if want(op):
            V.VALUE_OPS[op][0](b, rng, ctx)
    # late structure
    for op in ("spacer_rows", "spacer_cols", "repeat_header", "transpose"):
        if want(op):
            S.STRUCT_OPS[op][0](b, rng, ctx)
    if want("totals_as_values"):
        S.totals_as_values(b, rng, ctx)
    if want("wrong_total"):
        S.wrong_total(b, rng, ctx)
    return b


class _Gate:
    """ctx proxy that honours allow/deny lists for the grouping operator."""

    def __init__(self, ctx, allow, deny):
        self.ctx, self.allow, self.deny = ctx, allow, deny
        self.lang, self.num_style, self.rng, self.prof = ctx.lang, ctx.num_style, ctx.rng, ctx.prof

    def p(self, op):
        return op not in self.deny and (self.allow is None or op in self.allow) and self.ctx.p(op)


def _crosstab_parent(b: Block):
    """Parent header row over the period columns of a crosstab (the measure name)."""
    name = b.stats.get("measure_name", "")
    b.insert_row(0, "header")
    b.header_rows += 1
    b.cells[(0, 1)] = Cell(v=name, role="header", ops=["multirow_header"], style={"align": "center"})
    b.merges.append((0, 1, 0, b.n_cols - 1))
    if (1, 0) in b.cells:
        b.cells[(0, 0)] = b.cells.pop((1, 0))
        b.merges.append((0, 0, 1, 0))
    for ci in b.columns[1:]:
        ci.header_path = [name] + ci.header_path
    b.log("multirow_header", levels=2, crosstab=True)


def _drop_header(b: Block):
    """Remove the header rows (stack variant c: second table continues without a header)."""
    h = b.header_rows
    removed = [(k, v) for k, v in b.cells.items() if k[0] < h]
    for (r, c), cell in removed:
        b.implied.append({"r": None, "c": c, "src": cell.src, "orig": cell.orig, "ops": cell.ops + ["stack_schema_change"],
                          "removed": True, "header": True})
    b.cells = {(r - h, c): v for (r, c), v in b.cells.items() if r >= h}
    for v in b.cells.values():
        if v.formula:
            v.formula["refs"] = [(r - h, c) for r, c in v.formula["refs"]]
    b.merges = [(r1 - h, c1, r2 - h, c2) for r1, c1, r2, c2 in b.merges if r1 >= h]
    b.row_kind = b.row_kind[h:]
    b.n_rows -= h
    b.header_rows = 0
    b.hidden_rows = {r - h for r in b.hidden_rows if r >= h}


def _years_header(b: Block, rng):
    """Variant e: the second table's numeric column headers are years (2023 2024 2025)."""
    hr = b.header_rows - 1
    y = rng.randint(1995, 2025)
    n = 0
    for c, ci in enumerate(b.columns):
        cell = b.cells.get((hr, c))
        if cell and (is_measure(ci.stype) or ci.stype == "percent"):
            cell.v = y + n if rng.random() < 0.7 else str(y + n)
            cell.tag("stack_schema_change")
            ci.header_path = [str(y + n)]
            n += 1
    return n


# ---------------------------------------------------------------- sheet layout (stage C)


def _loose(draft: SheetDraft, row, col, v, role, op, merge_to=None, style=None):
    draft.loose.append(Loose(row, col, Cell(v=v, role=role, ops=[op], style=dict(style or {})), merge_to))


def _title_lines(ctx: Ctx, b: Block) -> list[tuple[str, str]]:
    rng, lang = ctx.rng, ctx.lang
    title = rng.choice(TITLE_PREFIX[lang]) + b.title
    if rng.random() < 0.3:
        title = title.upper()
    lines = [("title", title)]
    if rng.random() < 0.3:
        lines.insert(0, ("org", rng.choice(ORGS)))
    if b.scale != 1:
        lines.append(("scale", rng.choice(SCALE[lang][:2])))
    if rng.random() < 0.35:
        d = dt.date(rng.randint(1999, 2025), rng.randint(1, 12), rng.randint(1, 28))
        ds = d.isoformat() if lang == "en" and rng.random() < 0.5 else (f"{d.day:02d}.{d.month:02d}.{d.year}" if lang != "en" else d.strftime("%B %d, %Y"))
        lines.append(("as_of", AS_OF[lang].format(d=ds)))
    if rng.random() < 0.1:
        lines.append(("prepared", PREPARED[lang].format(n=rng.choice(PEOPLE))))
    return lines


def _place_title_block(ctx: Ctx, draft: SheetDraft, top: int, left: int, b: Block) -> int:
    """Writes title lines starting at `top`; returns the row after the block (incl. a gap)."""
    rng = ctx.rng
    lines = _title_lines(ctx, b)
    merge = rng.random() < 0.35 and b.n_cols > 1
    for i, (kind, text) in enumerate(lines):
        style = {"bold": True, "font_size": rng.choice([12, 14, 16])} if kind == "title" and rng.random() < 0.8 else {}
        if merge and kind == "title":
            style["align"] = "center"
        _loose(draft, top + i, left, text, "title", "title_block", merge_to=left + b.n_cols - 1 if merge and kind == "title" else None, style=style)
    draft.log("title_block", lines=[k for k, _ in lines], merged=merge)
    return top + len(lines) + rng.choice([0, 1, 1, 1, 2])


def _place_caption(ctx: Ctx, draft: SheetDraft, top: int, left: int, n: int, b: Block) -> int:
    rng = ctx.rng
    text = rng.choice([TABLE_CAPTION[ctx.lang].format(n=n) + ": " + b.title, b.title, b.title.upper(),
                       TABLE_CAPTION[ctx.lang].format(n=n)])
    _loose(draft, top, left, text, "title", "table_caption", style={"bold": True} if rng.random() < 0.6 else {})
    return top + 1 + (1 if rng.random() < 0.3 else 0)


def _notes_lines(ctx: Ctx, b: Block) -> list[str]:
    rng, lang = ctx.rng, ctx.lang
    lines = []
    if rng.random() < 0.6:
        lines.append(f"{SOURCE[lang]} {rng.choice(SOURCES)}")
    if rng.random() < 0.6 or not lines:
        lines.append(f"{rng.choice(NOTE[lang])} {rng.choice(NOTE_TEXT[lang])}")
    if rng.random() < 0.2:
        lines.append(f"* {rng.choice(NOTE_TEXT[lang])}")
    rng.shuffle(lines)
    return lines


def _block_for(ctx: Ctx, sources: list[Path], src: Source | None = None, **kw) -> Block | None:
    for _ in range(6):
        s = src or cached_source(str(ctx.rng.choice(sources)))
        b = build_table(ctx, s, **kw)
        if b is not None and b.n_rows >= 2:
            return b
        src = None
    return None


def build_sheet(ctx: Ctx, sources: list[Path], name: str) -> SheetDraft:
    rng = ctx.rng
    draft = SheetDraft(name=name)
    multi = ctx.p("multi_table")
    arrangement = _weighted(rng, ctx.prof.arrangement) if multi else "single"
    top = left = 0
    if ctx.p("offset"):
        top = min(6, int(rng.expovariate(0.7)))
        left = min(4, int(rng.expovariate(1.2)))
        draft.log("offset", top=top, left=left)
    first_src = cached_source(str(rng.choice(sources)))

    blocks: list[tuple[Block, dict]] = []
    if arrangement == "stack_schema_change":
        variant = rng.choice("abcde")
        n_first = rng.randint(100, 300)
        for _ in range(6):  # real exports: a long first segment, so prefer a source that has one
            if max(b - a for a, b in first_src.chunks) >= 100:
                break
            first_src = cached_source(str(rng.choice(sources)))
        b1 = _block_for(ctx, sources, first_src, n_rows=n_first, deny={"transpose", "repeat_header", "crosstab"})
        others = [p for p in sources if Path(p).name != Path(first_src.name).name] or sources
        kw = {"deny": {"transpose", "multirow_header", "wrapped_header", "crosstab", "row_label_column"}}
        if variant == "e":
            kw = {"allow": {"crosstab", "missing_tokens", "whitespace_noise", "locale_numbers"}}
        n2 = rng.randint(1, 3) if rng.random() < 0.25 else 1
        if b1 is None:
            arrangement, multi = "single", False
        else:
            blocks.append((b1, {"op": "sheet_start"}))
            for k in range(n2):
                src2 = cached_source(str(rng.choice(others)))
                b2 = _block_for(ctx, sources, src2, **kw)
                if b2 is None:
                    continue
                v = variant if k == 0 else rng.choice("abcd")
                if v == "c":
                    _drop_header(b2)
                if v == "e" and not any(o["op"] == "crosstab" for o in b2.ops):
                    if not _years_header(b2, rng):
                        v = "b"
                gap = {"a": rng.choice([1, 1, 2, 3]), "b": 0, "c": 0, "d": rng.choice([0, 0, 1, 2]), "e": rng.choice([0, 1, 1, 2])}[v]
                shift = rng.choice([-2, -1, 1, 2]) if v == "d" else 0
                blocks.append((b2, {"op": "stack_schema_change", "variant": v, "gap_rows": gap, "shift_cols": shift}))
    elif multi:
        n = min(ctx.prof.max_tables, 2 + int(rng.expovariate(1.2)))
        same = rng.random() < 0.5
        for k in range(n):
            b = _block_for(ctx, sources, first_src if (same or k == 0) else None)
            if b is not None:
                if arrangement == "stacked":
                    blocks.append((b, {"op": "sheet_start"} if not blocks else {"op": "multi_table_stacked", "gap_rows": rng.choice([0, 1, 1, 2, 2, 3, 4])}))
                else:
                    blocks.append((b, {"op": "sheet_start"} if not blocks else {"op": "multi_table_side", "gap_cols": rng.choice([1, 1, 2, 3])}))
    if not blocks:
        b = _block_for(ctx, sources, first_src)
        if b is None:
            draft.kind = "empty"
            return draft
        blocks.append((b, {"op": "sheet_start"}))
        arrangement = "single"

    want_title = ctx.p("title_block")
    want_notes = ctx.p("notes_block")
    row = top
    if want_title:
        row = _place_title_block(ctx, draft, row, left, blocks[0][0])
    caption_n = 1
    want_caps = arrangement in ("stacked", "side_by_side") and ctx.p("table_caption")
    side_left = left
    side_top = row
    side_bottom = row
    for i, (b, boundary) in enumerate(blocks):
        if arrangement == "side_by_side" and i > 0:
            prev = draft.placed[-1]
            tl = prev.left + prev.block.n_cols + boundary["gap_cols"]
            t = side_top
            draft.placed.append(Placed(b, t, tl, i))
            b.stats["boundary"] = boundary
            side_bottom = max(side_bottom, t + b.n_rows)
            continue
        if i > 0:
            row += boundary.get("gap_rows", 0)
        lft = left
        if boundary.get("shift_cols"):
            lft = max(0, left + boundary["shift_cols"])
            boundary["shift_cols"] = lft - left
        if want_caps and arrangement == "stacked" and (i > 0 or not want_title) and boundary.get("gap_rows", 1) > 0:
            row = _place_caption(ctx, draft, row, lft, caption_n + i, b)
            boundary["caption"] = True
        elif want_caps and arrangement == "side_by_side" and i == 0 and not want_title:
            row = _place_caption(ctx, draft, row, lft, caption_n, b)
            side_top = row
        draft.placed.append(Placed(b, row, lft, i))
        b.stats["boundary"] = boundary
        row += b.n_rows
        side_bottom = row
        # footnote lines right below the table that carries the markers
        if b.stats.get("footnote") and (i == len(blocks) - 1 or arrangement != "stack_schema_change"):
            row += rng.choice([0, 1])
            _loose(draft, row, lft, f"{b.stats['footnote']} {rng.choice(NOTE_TEXT[ctx.lang])}", "note", "footnote_markers",
                   style={"italic": True} if rng.random() < 0.4 else {})
            row += 1
            if i < len(blocks) - 1:
                row += 1
    row = max(row, side_bottom)
    if want_notes:
        row += rng.choice([0, 1, 1, 2])
        for line in _notes_lines(ctx, blocks[-1][0]):
            _loose(draft, row, left, line, "note", "notes_block", style={"italic": True} if rng.random() < 0.3 else {})
            row += 1
        draft.log("notes_block")
    if ctx.p("stray_cells"):
        _, max_col = draft.extent()
        for _ in range(rng.randint(1, 3)):
            if rng.random() < 0.6:
                r = rng.randint(top, max(top, row - 1))
                c = max_col + rng.randint(1, 3)
            else:
                r = row + rng.randint(2, 5)
                c = left + rng.randint(0, 3)
            if any(lc.row == r and lc.col == c for lc in draft.loose):
                continue
            v = rng.choice(STRAY) if rng.random() < 0.8 else round(rng.uniform(1, 1e5), 2)
            _loose(draft, r, c, v, "other", "stray_cells")
        draft.log("stray_cells")
    if arrangement != "single":
        draft.log(arrangement, tables=len(draft.placed))
    _style_sheet(ctx, draft)
    return draft


# ---------------------------------------------------------------- presentation (stage D)

NUM_FMTS = {"int": ["#,##0", "0", "#,##0;(#,##0)"], "count": ["#,##0", "0"], "float": ["0.00", "#,##0.00", "0.0", "General"],
            "money": ["#,##0.00", "$#,##0", "$#,##0.00", "#,##0 [$€-407]", "#,##0.00 \"RSD\"", "_($* #,##0_);_($* (#,##0);_($* \"-\"_);_(@_)"],
            "percent": ["0%", "0.0%", "0.00"], "lat": ["0.0000"], "lon": ["0.0000"]}
DATE_FMTS = {"en": ["yyyy-mm-dd", "m/d/yyyy", "d-mmm-yy", "mmm d, yyyy", "mm/dd/yy"], "de": ["dd.mm.yyyy", "d. mmmm yyyy"],
             "fr": ["dd/mm/yyyy"], "es": ["dd/mm/yyyy"], "it": ["dd/mm/yyyy"], "nl": ["d-m-yyyy", "dd-mm-yyyy"], "sr": ["dd.mm.yyyy", "d.m.yyyy."]}


def _style_sheet(ctx: Ctx, draft: SheetDraft):
    rng = ctx.rng
    hdr = ctx.p("style_header")
    hdr_style = {}
    if hdr:
        hdr_style = {"bold": rng.random() < 0.85}
        if rng.random() < 0.4:
            hdr_style["fill"] = rng.choice(["#D9D9D9", "#DDEBF7", "#FFF2CC", "#C6E0B4", "#1F4E78"])
            if hdr_style["fill"] == "#1F4E78":
                hdr_style["font_color"] = "#FFFFFF"
        if rng.random() < 0.4:
            hdr_style["border_bottom"] = 1
        if rng.random() < 0.3:
            hdr_style["align"] = "center"
        if rng.random() < 0.2:
            hdr_style["wrap"] = True
        draft.log("style_header", **{k: v for k, v in hdr_style.items()})
    tot = ctx.p("style_totals")
    tot_style = {}
    if tot:
        tot_style = {"bold": rng.random() < 0.8}
        if rng.random() < 0.5:
            tot_style["border_top"] = rng.choice([1, 6])
        if rng.random() < 0.25:
            tot_style["fill"] = "#F2F2F2"
        draft.log("style_totals")
    nf = ctx.p("number_formats")
    date_fmt = rng.choice(DATE_FMTS[ctx.lang])
    for p in draft.placed:
        b = p.block
        col_fmt = {}
        for c, ci in enumerate(b.columns):
            if nf and ci.stype in NUM_FMTS:
                col_fmt[c] = rng.choice(NUM_FMTS[ci.stype])
        for (r, c), cell in b.cells.items():
            if cell.role == "header" and hdr:
                for k, v in hdr_style.items():
                    cell.style.setdefault(k, v)
            elif cell.role == "total" and tot:
                for k, v in tot_style.items():
                    cell.style.setdefault(k, v)
            if isinstance(cell.v, (dt.date, dt.datetime)):
                cell.style.setdefault("num_format", date_fmt if not isinstance(cell.v, dt.datetime) else date_fmt + " hh:mm")
            elif isinstance(cell.v, (int, float)) and not isinstance(cell.v, bool):
                cc = c if b.orientation == "col" else r
                if cc in col_fmt:
                    cell.style.setdefault("num_format", col_fmt[cc])
                elif cell.formula is not None and nf:
                    cell.style.setdefault("num_format", "#,##0")
    if nf:
        draft.log("number_formats")
    if ctx.p("column_widths"):
        for p in draft.placed:
            for c in range(p.block.n_cols):
                draft.col_widths[p.left + c] = rng.choice([8, 10, 12, 14, 18, 24, 32])
        draft.log("column_widths")
    if ctx.p("hidden_rows_cols") and draft.placed:
        p = rng.choice(draft.placed)
        b = p.block
        if rng.random() < 0.5 and b.data_rows():
            rows = rng.sample(b.data_rows(), min(len(b.data_rows()), rng.randint(1, 3)))
            draft.hidden_rows.update(p.top + r for r in rows)
            b.hidden_rows.update(rows)
            draft.log("hidden_rows_cols", rows=[p.top + r for r in rows])
        elif b.n_cols > 2:
            c = rng.randrange(1, b.n_cols)
            draft.hidden_cols.add(p.left + c)
            b.hidden_cols.add(c)
            draft.log("hidden_rows_cols", cols=[p.left + c])
    if ctx.p("freeze_panes") and draft.placed:
        p = draft.placed[0]
        draft.freeze = (p.top + p.block.header_rows, p.left + (1 if rng.random() < 0.3 else 0))
        draft.log("freeze_panes", at=list(draft.freeze))
    if ctx.p("autofilter") and draft.placed:
        p = draft.placed[0]
        if p.block.header_rows:
            hr = p.top + p.block.header_rows - 1
            draft.autofilter = (hr, p.left, p.top + p.block.n_rows - 1, p.left + p.block.n_cols - 1)
            draft.log("autofilter")


LAYOUT_OPS = {
    "offset": "table starts away from A1 (top ~ Exp, left ~ Exp)",
    "title_block": "1–4 title/metadata lines above (org, title, scale 'in thousands', as-of date, prepared-by), sometimes merged",
    "table_caption": "short caption above each table in multi-table sheets",
    "notes_block": "source/note/footnote lines below the last table",
    "multi_table": "2–4 tables per sheet: stacked (gap 0–4 rows), side by side, or stack_schema_change",
    "stack_schema_change": "different tables stacked: (a) blank+header (b) header no blank (c) no blank no header (d) wider/narrower/shifted (e) year header; first segment 100–300 rows",
    "stray_cells": "loose comments/numbers in the margins (role other)",
    "multi_sheet": "2–4 sheets per workbook",
    "empty_sheet": "an extra empty sheet",
}
PRESENTATION_OPS = {
    "style_header": "bold/fill/border/center/wrap on header cells (unstyled ~25% to avoid style leakage)",
    "style_totals": "bold / top border / fill on total rows",
    "number_formats": "realistic number formats (#,##0, $, €, %, accounting); date formats per language",
    "column_widths": "random column widths",
    "hidden_rows_cols": "hide a few data rows or a column",
    "freeze_panes": "freeze below the header",
    "autofilter": "autofilter on the header row",
}


def sheet_name(rng, i: int, b: Block | None, lang: str) -> str:
    k = rng.random()
    if k < 0.4 or b is None:
        return f"Sheet{i + 1}" if lang == "en" else {"de": "Tabelle", "fr": "Feuil", "es": "Hoja", "it": "Foglio", "nl": "Blad", "sr": "List"}.get(lang, "Sheet") + str(i + 1)
    if k < 0.7:
        return re.sub(r"[\[\]\*\?/\\:]", " ", b.title)[:28].strip() or f"Sheet{i + 1}"
    return rng.choice(["Data", "Summary", "Detail", "Q1", "Q2", "Report", "Raw", "Input", "Calc", "2024", "Budget"])
