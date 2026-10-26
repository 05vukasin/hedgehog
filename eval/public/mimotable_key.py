"""MiMoTable gold keys: a fixed English subset, the checkable key of each free-text answer, and a
model-free matcher that decides whether an answer text contains that key. The accuracy scoreboard
(eval/accuracy/acc.py) reads its keys from here.

    python3 -I eval/public/mimotable_key.py sample [--show]     # the subset + gold keys (for the audit)

Needs `items.jsonl` from convert_mimotable.py.

Subset (fixed, reproducible)
----------------------------
English questions (641) minus those with a Visualize or Edit operation (their answer is a file or a
chart) and those whose gold narrative yields no checkable key (below). Strata = task class x table type,
task class from `meta_operation_list`: `lookup` (only Lookup), `compare` (Compare, no Calculate),
`calculate` (Calculate), `reasoning` (any Reasoning). Within each stratum the items are ordered by
sha256(SEED + id); a stratum gets round(N * share) items, at least 2 when it has 2 (largest remainders fill
up to N). Items listed in AUDIT_EXCLUDE (a hand check of every sampled key against its narrative: the key
was an intermediate number, the narrative itself is wrong or ambiguous, ...) are dropped and the next
item of the same stratum takes their place, so the subset stays N and stratified.

Gold key (from MiMoTable's free-text answer)
--------------------------------------------
The answer, or when it is long (> 260 chars) its last paragraph if that paragraph is a conclusion
("In summary", "Therefore", ...) of <= 260 chars; longer narratives are not checkable. Atoms of that text:
quoted phrases ("", “”, ‘’, backticks; <= 8 words; not in the question), dates (month names, ISO, m/d/Y;
not the question's), and numbers that are not inside a quote or date, not part of a name ("Product 3",
"Q1", "1-1") and not a number of the question. With no atom, a short answer's phrase after its last
" is / are / was / were " (<= 6 words). For "which / who / what is the name ..." questions that do not also
ask for an amount, numbers are dropped when a phrase atom exists (they support the entity). KEY_KEEP
(hand audit, indices only) trims a key to the asked atoms. A key has 1-4 atoms.

Matcher
-------
The candidate is an answer text. Correct when every key atom is found in it:
- number: some number of the candidate (US/European grouping, %, number words zero..twenty, a scale word
  million/billion/thousand/ten thousand/万/亿 applied or not) equals the gold value in absolute value
  (narratives write "a decrease of 41.67 %" for -41.67 %) within 0.5 % relative, or after rounding either
  one to the decimals the other shows; two whole numbers must be equal exactly (counts, ids, years); a
  percent also matches its fraction (41.67 % = 0.4167) and vice versa; a gold "5.64 million" also matches
  5,640,000;
- date: some date of the candidate has the same month and day (and year, when both give one);
- phrase: its normalised text (common.norm_text) occurs at word boundaries in the normalised candidate;
- a list = several atoms: all must occur (a set; order and extra items are not judged).
Unscored (not counted in accuracy, reported): a phrase atom not found exactly while >= 60 % of its words
are in the candidate (a paraphrase the matcher cannot judge).
"""

from __future__ import annotations

import argparse
import hashlib
import math
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import _NUMBER, data_dir, norm_text, parse_number, read_jsonl  # noqa: E402

ROOT = data_dir() / "mimotable"
SEED = "mimotable-agent-2026-10-23"
N = 150
MAX_LEN = 260
CONCLUSION = re.compile(r"^(in summary|in conclusion|to summarize|therefore|thus|so,|overall|in total|finally|based on (the|this|these) (calculation|analysis|data))", re.I)

# Hand check of the sampled keys (id -> why the key is not a fair gold value). Filled after reading
# every sampled item's narrative and key; dropped items are replaced from the same stratum.
U = "the asked entity is not quoted in the narrative; the numeric key only supports it"
AUDIT_EXCLUDE: dict[str, str] = {
    "en-0369": U,
    "en-0362": U,
    "en-0180": U,
    "en-0466": U,
    "en-0317": U,
    "en-0624": "key is a fragment ('reported by ...'), the device is unquoted",
    "en-0366": U,
    "en-0538": U,
    "en-0422": "yes/no question; the key holds only the compared values",
    "en-0500": U,
    "en-0403": U,
    "en-0413": "the entity is a long name the matcher cannot judge; the key holds only its value",
    "en-0102": "open trend answer; extracted phrase is not the answer",
    "en-0328": "open-ended (theme of the table)",
    "en-0490": U,
    "en-0636": "key is a sheet name",
    "en-0458": "key misses half of the answer (an unquoted word)",
    "en-0563": U,
    "en-0618": "extracted phrase includes extra words ('in ... status')",
    "en-0635": U,
    "en-0568": U,
    "en-0596": "the gold narrative does not answer the question",
    "en-0314": U,
    "en-0164": U,
    "en-0223": "yes/no question; the key holds only the compared values",
    "en-0455": U,
    "en-0489": U,
}
# id -> indices of the automatic key atoms to keep (the others are supporting values or names).
KEY_KEEP: dict[str, list[int]] = {
    "en-0464": [0],
    "en-0087": [0, 1],
    "en-0480": [0, 1, 3],
    "en-0487": [1],
    "en-0529": [0],
    "en-0496": [0, 1],
    "en-0067": [1],
    "en-0599": [0],
    "en-0516": [0],
    "en-0629": [2],
    "en-0640": [0, 1],
    "en-0638": [1],
    "en-0549": [1],
    "en-0319": [2],
    "en-0511": [0],
    "en-0492": [1],
    "en-0452": [1],
}


# ------------------------------------------------------------------------------------------------
# subset

def task_class(ops: list[str]) -> str:
    s = set(ops)
    if "Reasoning" in s:
        return "reasoning"
    if "Calculate" in s:
        return "calculate"
    if "Compare" in s:
        return "compare"
    return "lookup"


def key_text(answer: str) -> str | None:
    a = answer.strip()
    if len(a) <= MAX_LEN:
        return a
    paras = [p.strip() for p in re.split(r"\n\s*\n", a) if p.strip()]
    last = paras[-1] if paras else ""
    if len(paras) > 1 and len(last) <= MAX_LEN and CONCLUSION.match(last):
        return last
    return None


_COPULA = re.compile(r"\b(?:is|are|was|were)\s+(?:the\s+)?([^.;:\n]{1,80}?)\s*[.;\n]?\s*$", re.I)
_QUOTE = re.compile(r'"([^"\n]{1,80})"|“([^”\n]{1,80})”|‘([^’\n]{1,80})’|`([^`\n]{1,80})`')
_MONTHS = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october",
           "november", "december"]
_MON = r"(january|february|march|april|may|june|july|august|september|october|november|december|jan|feb|mar|apr|jun|jul|aug|sep|sept|oct|nov|dec)\.?"
_DATE_RX = [
    (re.compile(_MON + r"\s+(\d{1,2})(?:st|nd|rd|th)?(?:,?\s+(\d{4}))?(?![\d])", re.I), ("m", "d", "y")),
    (re.compile(r"(?<!\d)(\d{1,2})(?:st|nd|rd|th)?\s+(?:of\s+)?" + _MON + r"(?:,?\s+(\d{4}))?", re.I), ("d", "m", "y")),
    (re.compile(r"(?<!\d)(\d{4})[-/.年](\d{1,2})[-/.月](\d{1,2})日?(?!\d)"), ("y", "m", "d")),
    (re.compile(r"(?<!\d)(\d{1,2})/(\d{1,2})/(\d{4})(?!\d)"), ("m", "d", "y")),
]
_SCALES = [(re.compile(r"^\s*ten thousand\b", re.I), 1e4), (re.compile(r"^\s*hundred million\b", re.I), 1e8),
           (re.compile(r"^\s*(thousand|k)\b", re.I), 1e3), (re.compile(r"^\s*(million|mn)\b", re.I), 1e6),
           (re.compile(r"^\s*(billion|bn)\b", re.I), 1e9), (re.compile(r"^\s*万"), 1e4), (re.compile(r"^\s*亿"), 1e8)]
WHICH = re.compile(r"^\s*(which|who|whose|what is the (name|product name|material code|color|status|type|invoice type|"
                   r"customer unit|client unit|manufacturer|machinery name|project name))\b", re.I)
ALSO_ASKS_NUMBER = re.compile(r"\b(how many|how much|what (is|was) the (total|sum|score|amount|value|average)|and what|and how)\b", re.I)


def _month(s: str) -> int:
    s = s.lower().rstrip(".")
    return next(i + 1 for i, m in enumerate(_MONTHS) if m.startswith(s[:3]))


def find_dates(text: str) -> list[tuple[int, int, dict]]:
    """(start, end, {y, m, d}) of the dates written in `text` (month names, ISO, m/d/Y)."""
    out = []
    for rx, order in _DATE_RX:
        for m in rx.finditer(text):
            if any(a <= m.start() < b for a, b, _ in out):
                continue
            v = dict(zip(order, m.groups()))
            try:
                d = {"m": _month(v["m"]) if not str(v["m"]).isdigit() else int(v["m"]), "d": int(v["d"]),
                     "y": int(v["y"]) if v.get("y") else None}
            except (StopIteration, ValueError):
                continue
            if 1 <= d["m"] <= 12 and 1 <= d["d"] <= 31:
                out.append((m.start(), m.end(), d))
    return out


def scale_after(text: str, end: int) -> float:
    return next((f for rx, f in _SCALES if rx.match(text[end:])), 1.0)


def gold_key(item: dict) -> list[dict] | None:
    t = key_text(item["answer"])
    if t is None:
        return None
    q = item["question"]
    qn = norm_text(q)
    qdigits = {float(x) for x in re.findall(r"\d+(?:\.\d+)?", q.replace(",", ""))}
    qdates = [(d["m"], d["d"]) for _, _, d in find_dates(q)]
    atoms: list[dict] = []
    spans: list[tuple[int, int]] = []
    for m in _QUOTE.finditer(t):
        ph = next(g for g in m.groups() if g).strip().rstrip(".,;:!?").strip()
        spans.append(m.span())
        n = norm_text(ph)
        if not n or n in qn or len(ph.split()) > 8 or any(norm_text(a["text"]) == n for a in atoms):
            continue
        atoms.append({"text": ph, "kind": "number" if parse_number(ph) else "text"})
    for a, b, d in find_dates(t):
        spans.append((a, b))
        if (d["m"], d["d"]) not in qdates:
            atoms.append({"text": t[a:b], "kind": "date", **d})
    for m in _NUMBER.finditer(t):
        if any(a <= m.start() < b for a, b in spans):
            continue
        if re.search(r"[A-Za-z][A-Za-z_]*[ _-]?$", t[max(0, m.start() - 20):m.start()]) and \
                re.search(r"[A-Z][A-Za-z_]*[ _-]?$", t[max(0, m.start() - 20):m.start()]):
            continue  # part of a name: "Product 3", "Class 1-1", "Q1", "Sheet2"
        if re.match(r"-\d", t[m.end():]) or t[max(0, m.start() - 1):m.start()] == "-":
            continue  # "1-1", "3-1": a code, not a value
        p = parse_number(m.group(0))
        if not p or abs(p[0]) in qdigits or p[0] in qdigits:
            continue
        if re.match(r"^\s*(st|nd|rd|th)\b", t[m.end():]):
            continue
        atoms.append({"text": m.group(0), "kind": "number", "scale": scale_after(t, m.end())})
    if not atoms and len(t) <= 160:
        m = _COPULA.search(t.rstrip())
        if m:
            ph = m.group(1).strip().strip('"“”\'`').strip()
            if ph and len(ph.split()) <= 6 and norm_text(ph) and norm_text(ph) not in qn:
                atoms = [{"text": ph, "kind": "number" if parse_number(ph) else "text"}]
    if WHICH.match(q) and not ALSO_ASKS_NUMBER.search(q) and any(a["kind"] == "text" for a in atoms):
        atoms = [a for a in atoms if a["kind"] != "number"]  # the entity is the answer; its value supports it
    keep = KEY_KEEP.get(item["id"])
    if keep is not None:
        atoms = [atoms[i] for i in keep if i < len(atoms)]
    return atoms if 1 <= len(atoms) <= 4 else None


def order_key(i: str) -> str:
    return hashlib.sha256(f"{SEED}:{i}".encode()).hexdigest()


def candidates() -> list[dict]:
    out = []
    for x in read_jsonl(ROOT / "items.jsonl"):
        if x["lang"] != "english" or x["missing"] or {"Visualize", "Edit"} & set(x["ops"]):
            continue
        k = gold_key(x)
        if k is None:
            continue
        out.append({**x, "key": k, "tclass": task_class(x["ops"])})
    return out


def subset(n: int = N) -> list[dict]:
    pool = defaultdict(list)
    for x in candidates():
        if x["id"] not in AUDIT_EXCLUDE:
            pool[(x["tclass"], x["table_type"])].append(x)
    for v in pool.values():
        v.sort(key=lambda x: order_key(x["id"]))
    total = sum(len(v) for v in pool.values())
    quota = {s: min(len(v), max(min(2, len(v)), math.floor(n * len(v) / total))) for s, v in pool.items()}
    rema = sorted(pool, key=lambda s: -(n * len(pool[s]) / total - math.floor(n * len(pool[s]) / total)))
    while sum(quota.values()) < n:
        grew = False
        for s in rema:
            if sum(quota.values()) >= n:
                break
            if quota[s] < len(pool[s]):
                quota[s] += 1
                grew = True
        if not grew:
            break
    while sum(quota.values()) > n:  # minimums pushed it over: take from the largest strata
        s = max(quota, key=lambda s: quota[s])
        quota[s] -= 1
    out = [x for s in sorted(pool) for x in pool[s][:quota[s]]]
    return sorted(out, key=lambda x: order_key(x["id"]))


# ------------------------------------------------------------------------------------------------
# matching

_SCALE = {"thousand": 1e3, "k": 1e3, "million": 1e6, "mn": 1e6, "m": 1e6, "billion": 1e9, "bn": 1e9,
          "万": 1e4, "亿": 1e8, "千": 1e3, "百万": 1e6}
_CAND_NUM = re.compile(_NUMBER.pattern + r"|(?<![A-Za-z0-9_.])[-−]?\d+(?:[.,]\d+)+%?(?![A-Za-z0-9])")
_SCALE_AFTER = re.compile(r"^\s*(thousand|million|billion|mn|bn|k|m|万|亿|千|百万)\b", re.I)


_WORDS = {w: i for i, w in enumerate("zero one two three four five six seven eight nine ten eleven twelve thirteen "
                                      "fourteen fifteen sixteen seventeen eighteen nineteen twenty".split())}


def cand_numbers(text: str) -> list[tuple[float, int, bool]]:
    """Every number of a candidate answer (digits, or a number word zero..twenty), with scale words applied too."""
    t = text.replace("**", "").replace("`", "")
    out = [(float(_WORDS[w]), 0, False) for w in re.findall(r"[a-z]+", t.lower()) if w in _WORDS]
    for m in re.finditer(r"[-−]?\d[\d,.  ' ]*\d%?|[-−]?\d%?", t):
        s = m.group(0).strip()
        for tok in [s] + s.split():
            p = parse_number(tok.rstrip(".,"))
            if p:
                out.append(p)
                after = _SCALE_AFTER.match(t[m.end():])
                if after and tok == s:
                    out.append((p[0] * _SCALE[after.group(1).lower()], p[1], p[2]))
    return out


def _close(g: float, gd: int, c: float, cd: int) -> bool:
    g, c = abs(g), abs(c)
    if gd == 0 and cd == 0 and g == int(g) and c == int(c):
        return g == c  # two whole numbers (counts, ids, years): exact
    if math.isclose(g, c, rel_tol=0.005, abs_tol=1e-9):
        return True
    if round(c, gd) == round(g, gd) and (gd > 0 or g >= 1):
        return True
    return round(g, cd) == round(c, cd) and cd > 0 and (abs(g) >= 10 ** -cd)


def num_found(gold: tuple[float, int, bool], cands: list[tuple[float, int, bool]], scale: float = 1.0) -> bool:
    gv, gd, gp = gold
    golds = [(gv, gd)] + ([(gv / 100, gd + 2)] if gp else []) + ([(gv * scale, 0)] if scale != 1.0 else [])
    for cv, cd, cp in cands:
        cs = [(cv, cd)] + ([(cv / 100, cd + 2)] if cp else []) + ([(cv * 100, max(cd - 2, 0))] if gp and not cp else [])
        if any(_close(g, d, c, e) for g, d in golds for c, e in cs):
            return True
    return False


def judge(key: list[dict], answer: str) -> tuple[str, list[str]]:
    """('correct' | 'wrong' | 'unscored', per-atom notes)."""
    if not answer.strip():
        return "wrong", ["empty answer"]
    cands = cand_numbers(answer)
    na = " " + norm_text(answer) + " "
    notes, verdicts = [], []
    cdates = [d for _, _, d in find_dates(answer)]
    for at in key:
        if at["kind"] == "date":
            ok = any(c["m"] == at["m"] and c["d"] == at["d"] and (not at["y"] or not c["y"] or c["y"] == at["y"]) for c in cdates)
            verdicts.append("ok" if ok else "miss")
            notes.append(f"{at['text']}:{verdicts[-1]}")
            continue
        p = parse_number(at["text"])
        if at["kind"] == "number" and p:
            ok = num_found(p, cands, at.get("scale", 1.0))
            verdicts.append("ok" if ok else "miss")
            notes.append(f"{at['text']}:{'ok' if ok else 'miss'}")
            continue
        g = norm_text(at["text"])
        if g and f" {g} " in na:
            verdicts.append("ok")
        else:
            ws = g.split()
            share = sum(f" {w} " in na for w in ws) / max(1, len(ws))
            verdicts.append("partial" if share >= 0.6 else "miss")
        notes.append(f"{at['text']}:{verdicts[-1]}")
    if all(v == "ok" for v in verdicts):
        return "correct", notes
    if "miss" not in verdicts:
        return "unscored", notes
    return "wrong", notes


def cmd_sample(a) -> None:
    c = candidates()
    print(f"English, not Visualize/Edit, with a checkable key: {len(c)}")
    print(Counter(x["tclass"] for x in c), Counter(x["table_type"] for x in c))
    s = subset()
    print(f"subset: {len(s)}", Counter(x["tclass"] for x in s))
    print(Counter((x["tclass"], x["table_type"]) for x in s))
    if a.show:
        for x in s:
            print(f"\n== {x['id']} {x['tclass']} | {x['table_type']}\nQ: {x['question']}\nA: {x['answer'][:600]}\nKEY: {[k['text'] for k in x['key']]}")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["sample"])
    ap.add_argument("--show", action="store_true")
    cmd_sample(ap.parse_args())


if __name__ == "__main__":
    main()
