"""See, file by file, what hedgehog produces and what an AI agent would receive.

    hedgehog-inspect FILE [FILE|DIR ...] [options]          (or: python -m hedgehog.inspect ...)

One file: normalizes it (into a temporary folder unless ``--out``) and prints, section by section,
the overview of the source, every table (columns, types, first rows), the totals check, the notes,
the warnings, what the agent gets (files, manifest size, loading code) and the summary — the free
deterministic one, or with ``--llm`` the one an LLM phrases from the facts (key from ``.env``).
``--ask`` lets the LLM answer a question with the workbook tools, like an agent would.

Several files or a folder: one line per file (tables, cells placed, totals, time); ``--full`` prints
every file in full.

Sections (``--show``, comma-separated; default ``overview,tables,totals,notes,warnings,agent,summary``):
``overview`` ``tables`` ``totals`` ``notes`` ``warnings`` ``agent`` ``summary`` ``manifest`` (the
whole manifest JSON) ``all``.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Dict, List, Optional, Sequence

SECTIONS = ["overview", "tables", "totals", "notes", "warnings", "agent", "summary", "manifest"]
DEFAULT_SECTIONS = SECTIONS[:-1]
EXTENSIONS = {".xlsx", ".xlsm", ".xlsb", ".xls", ".ods", ".csv", ".tsv"}


class Out:
    """Terminal output with optional colour (off when piped or with NO_COLOR)."""

    def __init__(self, color: Optional[bool] = None):
        self.color = sys.stdout.isatty() and not os.environ.get("NO_COLOR") if color is None else color
        self.width = shutil.get_terminal_size((120, 40)).columns

    def _c(self, code: str, s: str) -> str:
        return f"\033[{code}m{s}\033[0m" if self.color else s

    def heading(self, s: str) -> None:
        print()
        print(self._c("1;36", f"━━ {s} " + "━" * max(0, min(self.width, 100) - len(s) - 4)))

    def sub(self, s: str) -> None:
        print(self._c("1", s))

    def dim(self, s: str) -> str:
        return self._c("2", s)

    def good(self, s: str) -> str:
        return self._c("32", s)

    def bad(self, s: str) -> str:
        return self._c("1;31", s)

    def warn(self, s: str) -> str:
        return self._c("33", s)


def _fmt(v: Any) -> str:
    if v is None:
        return "—"
    if isinstance(v, float):
        return f"{v:,.4f}".rstrip("0").rstrip(".") if abs(v) < 1e15 else f"{v:g}"
    if isinstance(v, int):
        return f"{v:,}"
    return str(v)


def find_env(start: Path) -> Optional[Path]:
    """The nearest ``.env`` in ``start`` or a parent folder."""
    for d in [start, *start.parents]:
        p = d / ".env"
        if p.is_file():
            return p
    return None


def _files(paths: Sequence[str]) -> List[Path]:
    out: List[Path] = []
    for p in map(Path, paths):
        if p.is_dir():
            out.extend(sorted(x for x in p.rglob("*") if x.is_file() and x.suffix.lower() in EXTENSIONS))
        else:
            out.append(p)
    return out


def _normalize(path: Path, out_dir: Path, args: argparse.Namespace):
    import hedgehog

    t = time.perf_counter()
    r = hedgehog.normalize(
        path,
        out_dir,
        keep_totals=args.keep_totals,
        locale=args.locale,
        rules_only=args.rules_only,
        model_max_cells=args.model_max_cells,
    )
    return r, time.perf_counter() - t


def _one_line(path: Path, r, seconds: float) -> Dict[str, Any]:
    m = r.manifest
    c = m["cells"]
    t = m["totals"]
    return {
        "file": str(path),
        "tables": len(m["tables"]),
        "cells": c["total"],
        "in_tables": c["in_tables"],
        "pct": 100.0 * c["in_tables"] / max(c["total"], 1),
        "notes": c["in_notes"],
        "totals_ok": t["ok"],
        "totals_bad": t["mismatch"],
        "seconds": seconds,
        "roles": m["options"].get("cell_roles", "rules"),
    }


def section_overview(o: Out, path: Path, r, seconds: float) -> None:
    m = r.manifest
    src = m["source"]
    o.heading(f"OVERVIEW · {path.name}")
    print(f"file      {src['path']}  ({_fmt(src['size_bytes'])} bytes, sha256 {src['sha256'][:12]}…)")
    roles = m["options"].get("cell_roles", "rules")
    print(f"run       {seconds * 1000:,.0f} ms · cell roles: {roles}"
          + (f" · locale {m['options']['locale']}" if m["options"].get("locale") else "")
          + (" · keep_totals" if m["options"].get("keep_totals") else ""))
    c = m["cells"]
    pct = 100.0 * c["in_tables"] / max(c["total"], 1)
    print(f"cells     {_fmt(c['total'])} non-empty: {_fmt(c['in_tables'])} in tables ({pct:.1f}%), "
          f"{_fmt(c['in_totals'])} on Totals, {_fmt(c['in_notes'])} on Notes")
    print(f"output    {r.xlsx_path}")
    print(f"          {r.manifest_path}")
    o.sub("sheets")
    for s in src["sheets"]:
        tabs = ", ".join(s["tables"]) or o.dim("no table")
        print(f"  {s['name'][:40]:<40} {s['used_range'] or 'empty':<14} {_fmt(s['cells']):>10} cells"
              f" {_fmt(s['formulas']):>8} formulas {_fmt(s['merges']):>6} merges  → {tabs}")


def _short(name: Any, width: int = 18) -> str:
    s = str(name)
    return s if len(s) <= width else s[: width - 1] + "…"


def section_tables(o: Out, r, rows: int, limit: Optional[int]) -> None:
    o.heading(f"TABLES · {len(r.tables)}")
    if not r.tables:
        print(o.warn("No tables found: every cell is on the Notes sheet."))
        return
    try:
        import pandas as pd
    except ImportError:
        pd = None
    for t in r.tables[:limit]:
        s = t["source"]
        o.sub(f"{t['id']}  sheet {t['sheet']!r}  ←  {s['sheet']}!{s['range']}"
              f"  ({_fmt(t['rows'])} rows, {len(t['columns'])} columns, confidence {t['confidence']:.2f})")
        if s.get("title"):
            print(f"  title   {s['title']!r} ({s['title_cell']})")
        print(f"  header  {s['header_range'] or '—'} ({s['header_rows']} row(s))   data {s['data_range'] or '—'}")
        if t.get("group_column"):
            print(f"  group column: {t['group_column']}")
        cols = []
        for c in t["columns"]:
            extra = "/".join(x for x in (c.get("unit"), c.get("currency")) if x)
            cols.append(f"{c['name']} [{c['dtype']}{(' ' + extra) if extra else ''}]")
        print("  columns " + o.dim(" · ".join(cols)[: 6 * o.width]))
        changed = {k: v for k, v in t.get("transformations", {}).items() if v}
        if changed:
            print("  changes " + o.dim(", ".join(f"{k} {v:,}" for k, v in sorted(changed.items(), key=lambda kv: -kv[1]))))
        for w in t.get("warnings", [])[:5]:
            print("  " + o.warn("warning: " + w))
        if pd is not None and rows > 0:
            df = pd.read_excel(r.xlsx_path, sheet_name=t["sheet"], nrows=rows)
            df.columns = [_short(c) for c in df.columns]
            with pd.option_context("display.max_columns", 50, "display.width", o.width - 2, "display.max_colwidth", 20):
                text = df.to_string(index=False, max_colwidth=20)
            print("\n".join("  " + line for line in text.splitlines()))
            if t["rows"] > rows:
                print(o.dim(f"  … {t['rows'] - rows:,} more rows"))
        print()
    if limit is not None and len(r.tables) > limit:
        print(o.dim(f"… {len(r.tables) - limit} more tables (use --tables all)"))


def section_totals(o: Out, r, rows: int) -> None:
    t = r.manifest["totals"]
    o.heading(f"TOTALS · {_fmt(t['count'])} checked: {_fmt(t['ok'])} ok, {_fmt(t['mismatch'])} mismatch, "
              f"{_fmt(t['unknown'])} could not be checked")
    items = t["items"]
    bad = [x for x in items if x["status"] == "mismatch"]
    if bad:
        o.sub("Totals that do not add up")
        for x in bad[: max(rows, 10)]:
            diff = (x["value"] or 0) - (x["recomputed"] or 0)
            print("  " + o.bad(f"{x['sheet']}!{x['cell']}") + f" {x['label'] or ''!r}: file says {_fmt(x['value'])}, "
                  f"{', '.join(x['sources']) or 'its cells'} add up to {_fmt(x['recomputed'])} "
                  f"(diff {_fmt(diff)}; {x['reason']})")
        if len(bad) > max(rows, 10):
            print(o.dim(f"  … {len(bad) - max(rows, 10)} more"))
    ok = [x for x in items if x["status"] == "ok"][:rows]
    if ok:
        o.sub("Examples that check out")
        for x in ok:
            cell = f"{x['sheet']}!{x['cell']}"
            n = f" ({x['count']:,} totals)" if x.get("count", 1) > 1 else ""
            print("  " + o.good(cell) + f" {x['label'] or ''!r} = {_fmt(x['value'])}{n} ← {', '.join(x['sources'])}")


def section_notes(o: Out, r, rows: int) -> None:
    n = r.manifest["notes"]
    kinds = ", ".join(f"{k} {v:,}" for k, v in n["by_kind"].items()) or "none"
    o.heading(f"NOTES · {_fmt(n['count'])} cells outside tables ({kinds})")
    if n["count"] == 0 or rows <= 0:
        return
    try:
        import pandas as pd

        df = pd.read_excel(r.xlsx_path, sheet_name="Notes", nrows=rows * 3)
        for _, row in df.iterrows():
            text = str(row.get("text", ""))[: o.width - 40]
            print(f"  {row.get('sheet', '')}!{row.get('cell', '')} [{row.get('kind', '')}] {text}")
        if n["count"] > rows * 3:
            print(o.dim(f"  … {n['count'] - rows * 3:,} more on the Notes sheet"))
    except ImportError:
        print(o.dim("  (install pandas to preview the Notes sheet)"))


def section_warnings(o: Out, r) -> None:
    w = list(r.manifest.get("warnings", []))
    per_table = sum(len(t.get("warnings", [])) for t in r.tables)
    o.heading(f"WARNINGS · {len(w)} workbook, {per_table} in tables")
    for x in w:
        print("  " + o.warn(x))
    shown = 0
    for t in r.tables:
        for x in t.get("warnings", []):
            if shown < 10:
                print("  " + o.warn(f"{t['id']}: {x}"))
            shown += 1
    if shown > 10:
        print(o.dim(f"  … {shown - 10} more (manifest tables[].warnings)"))
    if not w and not per_table:
        print(o.good("  none"))


def section_agent(o: Out, r) -> None:
    raw = Path(r.manifest_path).read_text(encoding="utf-8")
    o.heading("WHAT THE AGENT GETS")
    print(f"  {Path(r.xlsx_path).name}   the clean workbook (README, one sheet per table, Totals, Notes)")
    print(f"  {Path(r.manifest_path).name}   {len(raw):,} characters ≈ {len(raw) // 4:,} tokens if read in full")
    o.sub("Loading code it is given (README sheet / manifest load_hints)")
    for name, code in r.manifest.get("load_hints", {}).items():
        print(o.dim(f"  # {name}"))
        print("\n".join("  " + line for line in code.splitlines()))


def section_summary(o: Out, path: Path, args: argparse.Namespace) -> None:
    import hedgehog

    client = _llm(o, args) if args.llm else None
    o.heading("SUMMARY · " + ("LLM-phrased" if client else "deterministic (no LLM, free)"))
    t = time.perf_counter()
    s = hedgehog.summarize(str(path), llm=client)
    print(s.text)
    if client is not None:
        u = s.usage
        meta = f"model {s.model} · {time.perf_counter() - t:.1f} s"
        if u is not None:
            meta += f" · {u.input_tokens:,} input + {u.output_tokens:,} output tokens"
        print(o.dim("  " + meta))
        if s.llm_error:
            print(o.bad("  LLM call failed, deterministic summary shown: " + s.llm_error))
        for w in s.warnings:
            print(o.warn("  citation check: " + w))
        if not s.warnings and not s.llm_error:
            print(o.good("  every cited cell checked out"))
    if args.ask:
        if client is None:
            client = _llm(o, args)
        o.heading(f"AGENT ANSWER · {args.ask}")
        from hedgehog import llm

        a = llm.ask(str(path), args.ask, client)
        print(a.text)
        for c in a.tool_calls_made:
            print(o.dim(f"  tool {c['name']}({json.dumps(c['arguments'], ensure_ascii=False)[:120]})"
                        + (" → error" if c.get("error") else "")))
        cited = ", ".join(a.cited_cells[:8]) + (f" … ({len(a.cited_cells)} cells)" if len(a.cited_cells) > 8 else "")
        print(o.dim(f"  {a.turns} turn(s), {a.seconds:.1f} s, {a.usage.input_tokens:,} + {a.usage.output_tokens:,} tokens; "
                    f"cited {cited or 'no cells'}"))


def _llm(o: Out, args: argparse.Namespace):
    from hedgehog import llm

    env = Path(args.env) if args.env else find_env(Path.cwd()) or find_env(Path(__file__).resolve().parent)
    if env:
        llm.load_env(env)
    provider = args.provider or os.environ.get("AI_PROVIDER") or next(iter(llm.available_providers(env)), None)
    try:
        client = llm.connect(provider, model=args.model, env_file=env)
    except Exception as e:  # noqa: BLE001
        print(o.bad(f"Could not connect to an LLM ({e}). Put a key in .env (see .env.example) or pass --provider."))
        sys.exit(2)
    print(o.dim(f"  LLM: {provider} · {getattr(client, 'model', '')} · keys from {env or 'environment'}"))
    return client


def section_manifest(o: Out, r) -> None:
    o.heading("MANIFEST (full JSON)")
    print(json.dumps(r.manifest, ensure_ascii=False, indent=2))


def inspect_file(o: Out, path: Path, args: argparse.Namespace, show: List[str]) -> None:
    out_dir = Path(args.out) if args.out else Path(tempfile.mkdtemp(prefix="hedgehog-inspect-"))
    r, seconds = _normalize(path, out_dir, args)
    if "overview" in show:
        section_overview(o, path, r, seconds)
    if args.compare:
        rules_args = argparse.Namespace(**{**vars(args), "rules_only": True})
        rr, rs = _normalize(path, Path(tempfile.mkdtemp(prefix="hedgehog-rules-")), rules_args)
        a, b = _one_line(path, r, seconds), _one_line(path, rr, rs)
        o.heading("COMPARE · rules + model vs rules alone")
        for k, label in (("tables", "tables"), ("in_tables", "cells in tables"), ("notes", "cells on Notes"),
                         ("totals_ok", "totals ok"), ("totals_bad", "totals mismatch"), ("seconds", "seconds")):
            va, vb = a[k], b[k]
            mark = "" if va == vb else o.warn("  ←  differs")
            print(f"  {label:<18} {_fmt(round(va, 3) if k == 'seconds' else va):>14}   {_fmt(round(vb, 3) if k == 'seconds' else vb):>14}{mark}")
    if "tables" in show:
        section_tables(o, r, args.rows, None if args.tables == "all" else int(args.tables))
    if "totals" in show:
        section_totals(o, r, args.rows)
    if "notes" in show:
        section_notes(o, r, args.rows)
    if "warnings" in show:
        section_warnings(o, r)
    if "agent" in show:
        section_agent(o, r)
    if "summary" in show or args.llm or args.ask:
        section_summary(o, path, args)
    if "manifest" in show:
        section_manifest(o, r)
    if args.open:
        opener = "open" if sys.platform == "darwin" else "xdg-open"
        os.system(f'{opener} "{r.xlsx_path}" >/dev/null 2>&1 &')  # noqa: S605 (local file we just wrote)
    if not args.out:
        print(o.dim(f"\n(output kept in {out_dir}; use --out DIR to choose the folder)"))


def batch(o: Out, files: List[Path], args: argparse.Namespace) -> int:
    o.heading(f"{len(files)} files")
    print(o.dim(f"  {'tables':>6} {'cells':>11} {'in tables':>9} {'notes':>8} {'tot ok':>7} {'tot bad':>7} {'time':>8}  file"))
    failed = 0
    with tempfile.TemporaryDirectory(prefix="hedgehog-batch-") as tmp:
        for f in files:
            try:
                r, s = _normalize(f, Path(args.out or tmp), args)
            except Exception as e:  # noqa: BLE001
                failed += 1
                print("  " + o.bad(f"FAILED {f}: {e}"))
                continue
            x = _one_line(f, r, s)
            bad = o.bad(f"{x['totals_bad']:>7,}") if x["totals_bad"] else f"{0:>7}"
            print(f"  {x['tables']:>6} {x['cells']:>11,} {x['pct']:>8.1f}% {x['notes']:>8,} {x['totals_ok']:>7,} {bad} "
                  f"{x['seconds'] * 1000:>6,.0f}ms  {f}")
    print(o.dim(f"  {len(files) - failed} ok, {failed} failed · cell roles: {'rules' if args.rules_only else 'rules + model'}"))
    return 1 if failed else 0


def main(argv: Optional[Sequence[str]] = None) -> int:
    p = argparse.ArgumentParser(
        prog="hedgehog-inspect",
        description="Show what hedgehog makes of a spreadsheet and what an AI agent would receive.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="examples:\n"
        "  hedgehog-inspect sales.xlsx                     everything, deterministic summary\n"
        "  hedgehog-inspect sales.xlsx --llm               summary phrased by the LLM in .env\n"
        "  hedgehog-inspect sales.xlsx --ask 'Total Q2 revenue?'\n"
        "  hedgehog-inspect sales.xlsx --show tables,totals --rows 10\n"
        "  hedgehog-inspect sales.xlsx --compare           rules + model vs rules alone\n"
        "  hedgehog-inspect sales.xlsx --show manifest     the full manifest JSON\n"
        "  hedgehog-inspect folder/                        one line per file\n",
    )
    p.add_argument("paths", nargs="+", help="spreadsheet files or folders")
    p.add_argument("--show", default=",".join(DEFAULT_SECTIONS), help=f"sections: {','.join(SECTIONS)},all")
    p.add_argument("--rows", type=int, default=5, help="rows to preview per table / list (default 5)")
    p.add_argument("--tables", default="10", help="tables shown in detail (default 10, or 'all')")
    p.add_argument("--llm", action="store_true", help="phrase the summary with an LLM (key from .env)")
    p.add_argument("--ask", help="question for the LLM agent, answered with the workbook tools")
    p.add_argument("--provider", help="LLM provider (openai, anthropic, ollama, ...); default: AI_PROVIDER in .env")
    p.add_argument("--model", help="LLM model name")
    p.add_argument("--env", help="path to the .env file (default: nearest .env)")
    p.add_argument("--rules-only", action="store_true", help="cell roles from the rules alone (no learned model)")
    p.add_argument("--compare", action="store_true", help="also run --rules-only and compare the counts")
    p.add_argument("--keep-totals", action="store_true", help="keep total rows in the tables")
    p.add_argument("--locale", help="fix number/date conventions, e.g. de-DE, en-US")
    p.add_argument("--model-max-cells", type=int, help="sheets with more cells use the rules alone")
    p.add_argument("--out", help="keep the normalized files in this folder")
    p.add_argument("--open", action="store_true", help="open the normalized workbook afterwards")
    p.add_argument("--full", action="store_true", help="with several files: print every file in full")
    p.add_argument("--no-color", action="store_true")
    args = p.parse_args(argv)

    o = Out(False if args.no_color else None)
    show = SECTIONS if args.show == "all" else [s.strip() for s in args.show.split(",") if s.strip()]
    unknown = [s for s in show if s not in SECTIONS]
    if unknown:
        p.error(f"unknown section(s) {', '.join(unknown)}; choose from {', '.join(SECTIONS)}")
    files = _files(args.paths)
    missing = [f for f in files if not f.is_file()]
    if missing:
        p.error("not found: " + ", ".join(map(str, missing)))
    if len(files) == 1 or args.full:
        for f in files:
            inspect_file(o, f, args, show)
        return 0
    return batch(o, files, args)


if __name__ == "__main__":
    sys.exit(main())
