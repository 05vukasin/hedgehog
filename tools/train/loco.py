"""Leave-one-corpus-out (LOCO): how well does a cell-role model generalize to a corpus it never saw?

    python loco.py cells_v2.parquet cells_statsheets.parquet cells_strudel.parquet [--threads 8]

For each corpus C in the data (deco, statsheets, strudel, synthetic): train XGBoost on every other
corpus, test on C — DECO and synthetic on their `test` split (as `hedgehog benchmark` does), the
external corpora whole. Everything is scored in the coarse role space (title / header / data /
other; group headers count as headers, totals as data, notes as other), the label set all corpora
share. The rule layout (`l_role`) is scored on the same cells with the same metric.

Weights: each sheet's cells are rescaled so a sheet weighs `sqrt(its labelled cells)` (huge sheets
do not dominate, small ones still count), then rare classes are boosted (inverse frequency^0.5).
Metrics use the export weights only (data cells were sampled), so they estimate the full-sheet
scores `hedgehog benchmark` reports.
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import numpy as np
import pandas as pd

COARSE = np.array([0, 1, 2, 1, 2, 3, 3])  # full role id → title, header, data, other
NAMES = ["title", "header", "data", "other"]
META = ["source", "file", "sheet", "row", "col", "split", "label", "weight"]


def load(paths: list[Path]) -> pd.DataFrame:
    parts = []
    for p in paths:
        pq = p.with_suffix(".parquet")
        if p.suffix == ".csv" and (not pq.exists() or pq.stat().st_mtime < p.stat().st_mtime):
            df = pd.read_csv(p, dtype={"file": str, "sheet": str, "source": str, "split": str})
            feats = [c for c in df.columns if c not in META]
            df[feats] = df[feats].astype(np.float32)
            df.to_parquet(pq, compression="zstd")
        parts.append(pd.read_parquet(pq))
    return pd.concat(parts, ignore_index=True)


def per_file_macro(y: np.ndarray, p: np.ndarray, w: np.ndarray, keys: np.ndarray) -> float:
    """Mean over files of the macro F1 over the classes present in (truth or prediction of) the file."""
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


def pooled(y: np.ndarray, p: np.ndarray, w: np.ndarray) -> dict:
    out = {}
    for k, name in enumerate(NAMES):
        tp = w[(y == k) & (p == k)].sum()
        fp = w[(y != k) & (p == k)].sum()
        fn = w[(y == k) & (p != k)].sum()
        if tp + fp + fn:
            out[name] = 100 * 2 * tp / (2 * tp + fp + fn)
    out["macro"] = float(np.mean(list(out.values())))
    return out


def train_weights(df: pd.DataFrame, y: np.ndarray, power: float = 0.5) -> np.ndarray:
    w = df["weight"].to_numpy(np.float64)
    key = (df["source"] + "\x1f" + df["file"] + "\x1f" + df["sheet"]).to_numpy()
    _, inv, counts = np.unique(key, return_inverse=True, return_counts=True)
    sheet_w = np.bincount(inv, weights=w)
    w = w / sheet_w[inv] * np.sqrt(counts)[inv]
    freq = np.bincount(y, weights=w, minlength=len(NAMES))
    cw = (freq.sum() / np.maximum(freq, 1e-9)) ** power
    return (w * (cw / cw[2])[y]).astype(np.float32)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("data", type=Path, nargs="+")
    ap.add_argument("--threads", type=int, default=8)
    ap.add_argument("--rounds", type=int, default=400)
    ap.add_argument("--out", type=Path, default=Path("loco_report.json"))
    args = ap.parse_args()
    import xgboost as xgb

    df = load(args.data)
    feats = [c for c in df.columns if c not in META]
    X = df[feats].to_numpy(np.float32)
    y = COARSE[df["label"].to_numpy()]
    rules = COARSE[np.where(df["l_role"].to_numpy() < 0, 6, df["l_role"].to_numpy()).astype(int)]
    src = df["source"].to_numpy()
    print(f"{len(df):,} rows, {len(feats)} features; corpora: {dict(df['source'].value_counts())}")

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
        sw = np.zeros(len(df), np.float32)
        sw[fit] = train_weights(df[fit], y[fit])
        sw[is_valid] = train_weights(df[is_valid], y[is_valid])
        params = {"objective": "multi:softprob", "num_class": len(NAMES), "tree_method": "hist", "max_depth": 8,
                  "eta": 0.08, "subsample": 0.8, "colsample_bytree": 0.7, "min_child_weight": 2,
                  "eval_metric": "mlogloss", "nthread": args.threads}
        bst = xgb.train(params, xgb.DMatrix(X[fit], y[fit], weight=sw[fit], feature_names=feats), args.rounds,
                        evals=[(xgb.DMatrix(X[is_valid], y[is_valid], weight=sw[is_valid], feature_names=feats), "v")],
                        early_stopping_rounds=40, verbose_eval=False)
        pm = bst.predict(xgb.DMatrix(X[test], feature_names=feats), iteration_range=(0, bst.best_iteration + 1)).argmax(1)
        w = df.loc[test, "weight"].to_numpy()
        keys = (df.loc[test, "file"] + "\x1f" + df.loc[test, "sheet"]).to_numpy()
        # Hybrid: rules keep easy cells (data with data above and below, no total context) and
        # cells they call `other` (a corpus-specific notion the model cannot learn elsewhere).
        d = df.loc[test]
        easy = ((d["l_role"] == 2) & (d["l_role_up"] == 2) & (d["l_role_down"] == 2)
                & (d["x_row_vtotals"] == 0) & (d["t_in_keyword_total_row"] == 0)).to_numpy()
        ph = np.where(easy, rules[test], pm)
        ph_other = np.where(rules[test] == 3, 3, ph)
        r = {
            "hybrid": {"file_macro": per_file_macro(y[test], ph, w, keys), **pooled(y[test], ph, w)},
            "hybrid_other": {"file_macro": per_file_macro(y[test], ph_other, w, keys), **pooled(y[test], ph_other, w)},
            "easy_share": float(w[easy].sum() / w.sum()),
            "train_corpora": sorted(set(np.unique(src)) - {held}),
            "test_cells": int(test.sum()),
            "rules": {"file_macro": per_file_macro(y[test], rules[test], w, keys), **pooled(y[test], rules[test], w)},
            "model": {"file_macro": per_file_macro(y[test], pm, w, keys), **pooled(y[test], pm, w)},
            "rounds": int(bst.best_iteration + 1),
            "seconds": round(time.time() - t, 1),
        }
        report[held] = r
        print(f"\nheld out {held} ({r['test_cells']:,} cells; trained on {', '.join(r['train_corpora'])}; "
              f"{r['rounds']} rounds, {r['seconds']} s)")
        print(f"  model skipped on {100 * r['easy_share']:.0f}% of cells (easy)")
        print(f"  {'':13}{'file-macro':>11}{'pooled':>8}" + "".join(f"{n:>8}" for n in NAMES))
        for k in ("rules", "model", "hybrid", "hybrid_other"):
            m = r[k]
            print(f"  {k:13}{m['file_macro']:11.1f}{m['macro']:8.1f}" + "".join(f"{m.get(n, float('nan')):8.1f}" for n in NAMES))
    args.out.write_text(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
