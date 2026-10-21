"""Phase 1: row-segment classification, leave-one-corpus-out, scored on cells.

    python rows.py rows_deco.csv rows_statsheets.csv rows_strudel.csv rows_synthetic.csv

Input: `hedgehog features --level row` (one row per row segment). For each held-out corpus a coarse
(title / header / data / other) XGBoost model is trained on the other corpora; the segment's
prediction is given to all its labelled cells and scored exactly with the same per-file macro F1 as
`loco.py` (the `n_<role>` counts give the truth per cell, `pairs` the rules' cell predictions).

Variants:
- rules: the rule layout's cell roles;
- rows: the row model, one role per segment;
- rows+seq: the row model's probabilities decoded per sheet with a Viterbi pass over the segments in
  reading order (transition probabilities between coarse roles learned on the training corpora;
  hybrid HMM: emission = p(role | segment) / p(role)).
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import numpy as np
import pandas as pd

ROLES = ["title", "header", "data", "group_header", "total", "note", "other"]
COARSE = np.array([0, 1, 2, 1, 2, 3, 3])
NAMES = ["title", "header", "data", "other"]
K = len(NAMES)


FEATURE_PREFIXES = ("g_", "m_", "f_", "u_", "d_")


def parquet_of(p: Path) -> Path:
    """The parquet copy of a features CSV (written on first use, refreshed when the CSV is newer)."""
    pq = p.with_suffix(".parquet")
    if p.suffix == ".csv" and (not pq.exists() or pq.stat().st_mtime < p.stat().st_mtime):
        head = pd.read_csv(p, nrows=1)
        strings = {c: str for c in ("source", "file", "sheet", "split", "pairs")}
        dtypes = {c: np.float32 for c in head.columns if c[:2] in FEATURE_PREFIXES}
        pd.read_csv(p, dtype={**strings, **dtypes}).to_parquet(pq, compression="zstd")
    return pq


def load(paths: list[Path]) -> pd.DataFrame:
    return pd.concat([pd.read_parquet(parquet_of(p)) for p in paths], ignore_index=True)


def load_xy(paths: list[Path], keep: tuple[str, ...] = ()) -> tuple[pd.DataFrame, np.ndarray, list[str]]:
    """(labels and metadata, features as one float32 matrix, feature names), filled file by file
    so peak memory stays near one copy of the features. Feature columns named in `keep` also stay
    in the metadata frame."""
    import pyarrow.parquet as papq

    pqs = [parquet_of(p) for p in paths]
    names = papq.ParquetFile(pqs[0]).schema_arrow.names
    feats = [c for c in names if c[:2] in FEATURE_PREFIXES]
    n = sum(papq.ParquetFile(q).metadata.num_rows for q in pqs)
    X = np.empty((n, len(feats)), dtype=np.float32)
    metas, at = [], 0
    for q in pqs:
        d = pd.read_parquet(q)
        f = [c for c in d.columns if c[:2] in FEATURE_PREFIXES]
        if f != feats:
            raise SystemExit(f"{q}: feature columns differ from {pqs[0]}")
        X[at:at + len(d)] = d[feats].to_numpy(np.float32)
        at += len(d)
        metas.append(d.drop(columns=[c for c in feats if c not in keep]))
        del d
    return pd.concat(metas, ignore_index=True), X, feats


def style_columns(feats: list[str]) -> list[int]:
    """Positions of the formatting features (`m_s_*`, `f_s_*`, `u_s_*`, `d_s_*`)."""
    return [i for i, f in enumerate(feats) if f[:2] != "g_" and f[2:].startswith("s_")]


def style_dropout(df: pd.DataFrame, rate: float) -> np.ndarray:
    """Sheets (a deterministic share `rate`) whose formatting features are zeroed in training, as
    if they came from a format hedgehog reads no styles from (xls, ods, csv): most training sheets
    are xlsx, many production files are not."""
    key = df["source"] + "\x1f" + df["file"] + "\x1f" + df["sheet"]
    h = pd.util.hash_pandas_object(key, index=False).to_numpy()
    return (h % 10_000) < rate * 10_000


def without_styles(X: np.ndarray, rows: np.ndarray, drop: np.ndarray, cols: list[int]) -> np.ndarray:
    """X[rows] (a copy) with the formatting features of the `drop` rows zeroed."""
    Xr = X[rows]
    if cols:
        sel = np.flatnonzero(drop[rows])
        for c in cols:
            Xr[sel, c] = 0.0
    return Xr


def cell_entries(df: pd.DataFrame, pred: np.ndarray | None) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    """(truth, prediction, count, file key) per (segment, true role) — or per (truth, rules) pair
    when `pred` is None."""
    keys = (df["source"] + "\x1f" + df["file"] + "\x1f" + df["sheet"]).to_numpy()
    ys, ps, ws, ks = [], [], [], []
    if pred is not None:
        for r, name in enumerate(ROLES):
            n = df[f"n_{name}"].to_numpy()
            m = n > 0
            ys.append(np.full(m.sum(), COARSE[r]))
            ps.append(pred[m])
            ws.append(n[m])
            ks.append(keys[m])
    else:
        for key, pairs in zip(keys, df["pairs"].to_numpy()):
            for t in str(pairs).split("|"):
                g, q, n = (int(x) for x in t.split("."))
                ys.append([COARSE[g]])
                ps.append([COARSE[q if q >= 0 else 6]])
                ws.append([n])
                ks.append([key])
    return np.concatenate(ys), np.concatenate(ps), np.concatenate(ws).astype(float), np.concatenate(ks)


def per_file_macro(y, p, w, keys) -> float:
    order = np.argsort(keys, kind="stable")
    y, p, w, keys = y[order], p[order], w[order], keys[order]
    bounds = np.flatnonzero(np.r_[True, keys[1:] != keys[:-1], True])
    scores = []
    for a, b in zip(bounds[:-1], bounds[1:]):
        yy, pp, ww = y[a:b], p[a:b], w[a:b]
        f = []
        for k in np.union1d(np.unique(yy), np.unique(pp)):
            tp = ww[(yy == k) & (pp == k)].sum()
            fp = ww[(yy != k) & (pp == k)].sum()
            fn = ww[(yy == k) & (pp != k)].sum()
            f.append(2 * tp / (2 * tp + fp + fn) if tp + fp + fn else 1.0)
        scores.append(np.mean(f))
    return 100 * float(np.mean(scores))


def pooled(y, p, w) -> dict:
    out = {}
    for k, name in enumerate(NAMES):
        tp = w[(y == k) & (p == k)].sum()
        fp = w[(y != k) & (p == k)].sum()
        fn = w[(y == k) & (p != k)].sum()
        if tp + fp + fn:
            out[name] = 100 * 2 * tp / (2 * tp + fp + fn)
    out["macro"] = float(np.mean(list(out.values())))
    return out


def score(df, pred) -> dict:
    y, p, w, k = cell_entries(df, pred)
    return {"file_macro": per_file_macro(y, p, w, k), **pooled(y, p, w)}


def sheet_order(df: pd.DataFrame) -> list[np.ndarray]:
    key = (df["source"] + "\x1f" + df["file"] + "\x1f" + df["sheet"]).to_numpy()
    order = np.lexsort((df["left"].to_numpy(), df["row"].to_numpy(), key))
    ks = key[order]
    bounds = np.flatnonzero(np.r_[True, ks[1:] != ks[:-1], True])
    return [order[a:b] for a, b in zip(bounds[:-1], bounds[1:])]


def transitions(df: pd.DataFrame, y: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    T = np.ones((K, K))
    start = np.ones(K)
    for idx in sheet_order(df):
        s = y[idx]
        start[s[0]] += 1
        np.add.at(T, (s[:-1], s[1:]), 1)
    return np.log(start / start.sum()), np.log(T / T.sum(1, keepdims=True))


def viterbi(logp: np.ndarray, log_start: np.ndarray, log_T: np.ndarray, weight: float) -> np.ndarray:
    n = len(logp)
    dp = log_start * weight + logp[0]
    back = np.zeros((n, K), dtype=np.int64)
    for i in range(1, n):
        cand = dp[:, None] + weight * log_T
        back[i] = cand.argmax(0)
        dp = cand.max(0) + logp[i]
    path = np.zeros(n, dtype=np.int64)
    path[-1] = dp.argmax()
    for i in range(n - 1, 0, -1):
        path[i - 1] = back[i, path[i]]
    return path


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("data", type=Path, nargs="+")
    ap.add_argument("--threads", type=int, default=8)
    ap.add_argument("--rounds", type=int, default=400)
    ap.add_argument("--seq-weight", type=float, default=1.0)
    ap.add_argument("--out", type=Path, default=Path("rows_report.json"))
    ap.add_argument("--exclude", default="", help="comma-separated cell-feature prefixes to leave out (e.g. `s_`)")
    ap.add_argument("--seed", type=int, default=0, help="XGBoost seed (row and column sampling)")
    ap.add_argument("--style-dropout", type=float, default=0.0,
                    help="share of training sheets whose formatting features are zeroed")
    args = ap.parse_args()
    import xgboost as xgb

    df, X, feats = load_xy(args.data)
    excl = tuple(e for e in args.exclude.split(",") if e)
    if excl:
        cols = [i for i, f in enumerate(feats) if not (f[:2] != "g_" and f[2:].startswith(excl))]
        X, feats = X[:, cols], [feats[i] for i in cols]
    drop = style_dropout(df, args.style_dropout)
    scols = style_columns(feats) if args.style_dropout > 0 else []
    y = COARSE[df["label"].to_numpy()]
    src = df["source"].to_numpy()
    n_lab = df["n_labelled"].to_numpy().astype(np.float64)
    print(f"{len(df):,} segments, {len(feats)} features; {dict(df['source'].value_counts())}")

    report = {}
    for held in sorted(np.unique(src)):
        t = time.time()
        test = src == held
        if held in ("deco", "synthetic"):
            test &= df["split"].to_numpy() == "test"
        train = src != held
        files = df.loc[train, ["source", "file"]].drop_duplicates()
        valid_files = set(files.sample(frac=0.1, random_state=0).agg("\x1f".join, axis=1))
        is_valid = train & (df["source"] + "\x1f" + df["file"]).isin(valid_files).to_numpy()
        fit = train & ~is_valid

        def weights(mask):
            d = df[mask]
            key = (d["source"] + "\x1f" + d["file"] + "\x1f" + d["sheet"]).to_numpy()
            _, inv, cnt = np.unique(key, return_inverse=True, return_counts=True)
            w = n_lab[mask]
            sheet_w = np.bincount(inv, weights=w)
            w = w / sheet_w[inv] * np.sqrt(np.bincount(inv, weights=n_lab[mask]))[inv]
            freq = np.bincount(y[mask], weights=w, minlength=K)
            cw = (freq.sum() / np.maximum(freq, 1e-9)) ** 0.5
            return (w * (cw / cw[2])[y[mask]]).astype(np.float32)

        params = {"objective": "multi:softprob", "num_class": K, "tree_method": "hist", "max_depth": 8,
                  "eta": 0.08, "subsample": 0.8, "colsample_bytree": 0.5, "min_child_weight": 2,
                  "eval_metric": "mlogloss", "nthread": args.threads, "seed": args.seed}
        dfit = xgb.QuantileDMatrix(without_styles(X, np.flatnonzero(fit), drop, scols), y[fit], weight=weights(fit))
        dvalid = xgb.QuantileDMatrix(without_styles(X, np.flatnonzero(is_valid), drop, scols), y[is_valid],
                                     weight=weights(is_valid), ref=dfit)
        bst = xgb.train(params, dfit, args.rounds, evals=[(dvalid, "v")],
                        early_stopping_rounds=40, verbose_eval=False)
        it = (0, bst.best_iteration + 1)
        dtest = df[test].reset_index(drop=True)
        del dfit, dvalid  # free the training matrices before the next held-out corpus
        P = bst.predict(xgb.DMatrix(X[test]), iteration_range=it)
        p_rows = P.argmax(1)

        # Sequence decoding per sheet.
        log_start, log_T = transitions(df[train].reset_index(drop=True), y[train])
        prior = np.bincount(y[train], minlength=K) / train.sum()
        logp = np.log(np.clip(P, 1e-6, 1)) - np.log(prior)
        p_seq = p_rows.copy()
        for idx in sheet_order(dtest):
            p_seq[idx] = viterbi(logp[idx], log_start, log_T, args.seq_weight)

        r = {
            "train_corpora": sorted(set(np.unique(src)) - {held}),
            "segments": int(test.sum()),
            "rules": score(dtest, None),
            "rows": score(dtest, p_rows),
            "rows+seq": score(dtest, p_seq),
            "rounds": int(bst.best_iteration + 1),
            "seconds": round(time.time() - t, 1),
        }
        report[held] = r
        print(f"\nheld out {held} ({r['segments']:,} segments; {r['rounds']} rounds, {r['seconds']} s)")
        print(f"  {'':10}{'file-macro':>11}{'pooled':>8}" + "".join(f"{n:>8}" for n in NAMES))
        for k in ("rules", "rows", "rows+seq"):
            m = r[k]
            print(f"  {k:10}{m['file_macro']:11.1f}{m['macro']:8.1f}" + "".join(f"{m.get(n, float('nan')):8.1f}" for n in NAMES))
    args.out.write_text(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
