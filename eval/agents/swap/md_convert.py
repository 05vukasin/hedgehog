"""Convert one spreadsheet to markdown and cut it at a token budget (runs inside the sandbox).

    python md_convert.py REL_PATH BUDGET_TOKENS   -> writes out.md, prints a JSON line with what was done

MarkItDown first (in a child process limited to 3 GB and 15 min); if it fails, a streaming openpyxl
reader writes the same kind of markdown (one table per sheet) and stops once the budget is reached.
Token counts use tiktoken's o200k_base (the gpt-4.1/4o tokenizer).
"""

import json
import resource
import subprocess
import sys
from pathlib import Path


def markitdown(rel: str) -> None:
    from markitdown import MarkItDown
    Path("full.md").write_text(MarkItDown().convert(rel).text_content)


def limit():
    resource.setrlimit(resource.RLIMIT_AS, (3 << 30, 3 << 30))


def fallback(rel: str, max_chars: int) -> tuple[str, bool]:
    import openpyxl
    wb = openpyxl.load_workbook(rel, read_only=True, data_only=True)
    out, n = [], 0
    for ws in wb.worksheets:
        out.append(f"## {ws.title}\n")
        first = True
        for row in ws.iter_rows(values_only=True):
            cells = ["" if v is None else str(v).replace("|", "\\|").replace("\n", " ") for v in row]
            line = "| " + " | ".join(cells) + " |\n"
            out.append(line)
            n += len(line)
            if first:
                out.append("|" + "---|" * len(cells) + "\n")
                first = False
            if n > max_chars:
                return "".join(out), True
        out.append("\n")
    return "".join(out), False


def main() -> None:
    rel, budget = sys.argv[1], int(sys.argv[2])
    max_chars = budget * 8  # more characters than `budget` tokens of any table text
    info = {"budget_tokens": budget}
    try:
        p = subprocess.run([sys.executable, "-I", __file__, "--markitdown", rel], preexec_fn=limit, timeout=900,
                           capture_output=True, text=True)
        if p.returncode:
            raise RuntimeError(p.stderr.strip().splitlines()[-1] if p.stderr.strip() else f"exit {p.returncode}")
        text = Path("full.md").read_text()
        info |= {"converter": "markitdown 0.1.3", "chars_full": len(text)}
        cut_early = False
    except Exception as e:
        info |= {"converter": "openpyxl streaming (MarkItDown failed: " + f"{e.__class__.__name__}: {e}"[:200] + ")"}
        text, cut_early = fallback(rel, max_chars)
        if not cut_early:
            info["chars_full"] = len(text)
    import tiktoken
    enc = tiktoken.get_encoding("o200k_base")
    toks = enc.encode(text[:max_chars], disallowed_special=())
    truncated = cut_early or len(toks) > budget or len(text) > max_chars
    if len(toks) > budget:
        kept = enc.decode(toks[:budget])
        kept = kept[:kept.rfind("\n") + 1]
    else:
        kept = text[:max_chars]
    kept_tokens = len(enc.encode(kept, disallowed_special=()))
    if truncated:
        kept += "\n[truncated here: the rest of the file did not fit in the context window]\n"
        if "chars_full" in info:  # estimate from the characters per token of the kept part
            info["tokens_full_est"] = round(info["chars_full"] * kept_tokens / max(len(kept), 1))
    else:
        info["tokens_full_est"] = kept_tokens
    Path("out.md").write_text(kept)
    info |= {"tokens_kept": kept_tokens, "truncated": truncated}
    print(json.dumps(info))


if __name__ == "__main__":
    if sys.argv[1] == "--markitdown":
        markitdown(sys.argv[2])
    else:
        main()
