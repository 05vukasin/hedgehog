"""Phase 2: production row-segment models trained on every corpus, tested on every test split.

    python train_rows.py rows_deco.csv rows_statsheets.csv rows_strudel.csv rows_synthetic.csv \\
        --out models/ [--depth 8] [--threads 8]

Two stages:
1. coarse model (title / header / data / other) on all corpora's `dev` files — the label set every
   corpus shares, where generalization comes from;
2. fine model (the seven hedgehog roles) on the corpora labelled with all of them (DECO, Strudel,
   synthetic); at prediction it only chooses among the fine roles inside the coarse role of stage 1.

Post-processing (each scored separately):
- totals: a segment whose cells the rules all call `total` and that holds a detected total stays
  `total` (verified arithmetic beats learning);
- gate: a segment the rules call data whose rows above and below are data too keeps the rules'
  answer without running the model (the speed path).

Every corpus's `test` files (file-hash split, never trained on) are scored at cell level, coarse and
full role space, next to the rules; then a size sweep picks the smallest model within 0.3 points of
the best. Writes `coarse.json` / `fine.json` (hh-trees-1) and `report.json`.
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import numpy as np
import pandas as pd

from export_trees import convert, scores
from rows import COARSE, ROLES, load_xy, per_file_macro, style_columns, style_dropout, without_styles

NAMES = ["title", "header", "data", "other"]
FULL_SOURCES = {"deco", "strudel", "synthetic"}


def entries(df: pd.DataFrame, pred: np.ndarray | None, full: bool):
    """(truth, prediction, count, sheet key) for every labelled cell group; rules when pred is None."""
    m = (lambda r: r) if full else (lambda r: COARSE[r])
    keys = (df["source"] + "\x1f" + df["file"] + "\x1f" + df["sheet"]).to_numpy()
    ys, ps, ws, ks = [], [], [], []
    if pred is None:
        for key, pairs in zip(keys, df["pairs"].to_numpy()):
            for t in str(pairs).split("|"):
                g, q, n = (int(x) for x in t.split("."))
                ys.append(m(g)); ps.append(m(q if q >= 0 else 6)); ws.append(n); ks.append(key)
        return np.array(ys), np.array(ps), np.array(ws, float), np.array(ks)
    for r, name in enumerate(ROLES):
        n = df[f"n_{name}"].to_numpy()
        sel = n > 0
        ys.append(np.full(sel.sum(), m(r))); ps.append(pred[sel] if full else COARSE[pred[sel]])
        ws.append(n[sel]); ks.append(keys[sel])
    return np.concatenate(ys), np.concatenate(ps), np.concatenate(ws).astype(float), np.concatenate(ks)


def metrics(df, pred, full: bool) -> dict:
    y, p, w, k = entries(df, pred, full)
    names = ROLES if full else NAMES
    per = {}
    for c, name in enumerate(names):
        tp = w[(y == c) & (p == c)].sum(); fp = w[(y != c) & (p == c)].sum(); fn = w[(y == c) & (p != c)].sum()
        if tp + fp + fn:
            per[name] = 100 * 2 * tp / (2 * tp + fp + fn)
    return {"file_macro": per_file_macro(y, p, w, k), "pooled": float(np.mean(list(per.values()))), **per}


def weights(df: pd.DataFrame, y: np.ndarray, k: int) -> np.ndarray:
    n = df["n_labelled"].to_numpy(np.float64)
    key = (df["source"] + "\x1f" + df["file"] + "\x1f" + df["sheet"]).to_numpy()
    _, inv = np.unique(key, return_inverse=True)
    sheet = np.bincount(inv, weights=n)
    w = n / sheet[inv] * np.sqrt(sheet)[inv]
    freq = np.bincount(y, weights=w, minlength=k)
    cw = (freq.sum() / np.maximum(freq, 1e-9)) ** 0.5
    return (w * (cw / cw[np.argmax(freq)])[y]).astype(np.float32)


def train(X, rows, y, w, valid, k, args, feats, drop):
    """Trains on the rows `rows` of X (y, w, valid aligned with `rows`), without copying X whole."""
    import xgboost as xgb

    params = {"objective": "multi:softprob", "num_class": k, "tree_method": "hist", "max_depth": args.depth,
              "eta": args.eta, "subsample": 0.8, "colsample_bytree": 0.5, "min_child_weight": 2,
              "eval_metric": "mlogloss", "nthread": args.threads, "seed": args.seed}
    fit = ~valid
    cols = style_columns(feats) if args.style_dropout > 0 else []
    dfit = xgb.QuantileDMatrix(without_styles(X, rows[fit], drop, cols), y[fit], weight=w[fit], feature_names=feats)
    dvalid = xgb.QuantileDMatrix(without_styles(X, rows[valid], drop, cols), y[valid], weight=w[valid],
                                 feature_names=feats, ref=dfit)
    return xgb.train(params, dfit, args.rounds, evals=[(dvalid, "v")],
                     early_stopping_rounds=40, verbose_eval=False)


def combine(pc: np.ndarray, pf: np.ndarray) -> np.ndarray:
    """Fine role inside the predicted coarse role."""
    allowed = COARSE[None, :] == pc.argmax(1)[:, None]
    return np.where(allowed, pf, -1.0).argmax(1)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("data", type=Path, nargs="+")
    ap.add_argument("--out", type=Path, default=Path("models"))
    ap.add_argument("--threads", type=int, default=8)
    ap.add_argument("--rounds", type=int, default=600)
    ap.add_argument("--depth", type=int, default=8)
    ap.add_argument("--eta", type=float, default=0.08)
    ap.add_argument("--style-dropout", type=float, default=0.0,
                    help="share of training sheets whose formatting features are zeroed")
    ap.add_argument("--seed", type=int, default=0, help="XGBoost seed (row and column sampling)")
    ap.add_argument("--exclude", default="", help="comma-separated cell-feature prefixes to leave out (e.g. `s_`)")
    args = ap.parse_args()
    import xgboost as xgb

    args.out.mkdir(parents=True, exist_ok=True)
    df, X, feats = load_xy(args.data, keep=("m_l_role", "m_x_total_cand", "u_l_role", "d_l_role",
                                            "m_x_row_vtotals", "m_t_in_keyword_total_row"))
    excl = tuple(e for e in args.exclude.split(",") if e)
    if excl:
        cols = [i for i, f in enumerate(feats) if not (f[:2] != "g_" and f[2:].startswith(excl))]
        X, feats = X[:, cols], [feats[i] for i in cols]
    label = df["label"].to_numpy()
    src = df["source"].to_numpy()
    test = df["split"].to_numpy() == "test"
    files = df.loc[~test, ["source", "file"]].drop_duplicates()
    valid_files = set(files.sample(frac=0.1, random_state=1).agg("\x1f".join, axis=1))
    valid = (df["source"] + "\x1f" + df["file"]).isin(valid_files).to_numpy() & ~test
    print(f"{len(df):,} segments, {len(feats)} features; train {(~test).sum():,} (valid {valid.sum():,}), test {test.sum():,}")

    t = time.time()
    drop = style_dropout(df, args.style_dropout)
    tr = ~test
    yc = COARSE[label]
    coarse = train(X, np.flatnonzero(tr), yc[tr], weights(df[tr], yc[tr], 4), valid[tr], 4, args, feats, drop)
    full_tr = tr & np.isin(src, list(FULL_SOURCES))
    fine = train(X, np.flatnonzero(full_tr), label[full_tr], weights(df[full_tr], label[full_tr], 7), valid[full_tr], 7,
                 args, feats, drop)
    print(f"coarse {coarse.best_iteration + 1} rounds, fine {fine.best_iteration + 1} rounds, {time.time() - t:.0f} s")

    dt = df[test].reset_index(drop=True)
    Xt = xgb.DMatrix(X[test], feature_names=feats)

    def predict(rc: int, rf: int) -> np.ndarray:
        pc = coarse.predict(Xt, iteration_range=(0, rc))
        pf = fine.predict(Xt, iteration_range=(0, rf))
        return combine(pc, pf)

    rc_best, rf_best = coarse.best_iteration + 1, fine.best_iteration + 1
    p = predict(rc_best, rf_best)
    totals = ((dt["m_l_role"] == 4) & (dt["m_x_total_cand"] > 0)).to_numpy()
    p_tot = np.where(totals, 4, p)
    gate = ((dt["m_l_role"] == 2) & (dt["u_l_role"] == 2) & (dt["d_l_role"] == 2)
            & (dt["m_x_row_vtotals"] == 0) & (dt["m_t_in_keyword_total_row"] == 0)).to_numpy()
    p_gate = np.where(gate, 2, p_tot)

    report: dict = {"rounds": {"coarse": rc_best, "fine": rf_best}, "gate_share": {}, "corpora": {}}
    variants = {"rules": None, "model": p, "model+totals": p_tot, "model+totals+gate": p_gate}
    for c in sorted(np.unique(src)):
        sel = (dt["source"] == c).to_numpy()
        d = dt[sel]
        n = d["n_labelled"].to_numpy()
        report["gate_share"][c] = float(n[gate[sel]].sum() / n.sum())
        r = {}
        for name, pred in variants.items():
            q = None if pred is None else pred[sel]
            r[name] = {"coarse": metrics(d, q, False)}
            if c in FULL_SOURCES:
                r[name]["full"] = metrics(d, q, True)
        report["corpora"][c] = r
        print(f"\n{c} test ({sel.sum():,} segments; gate skips {100 * report['gate_share'][c]:.0f}% of cells)")
        print(f"  {'':20}{'coarse file':>12}{'coarse pool':>12}{'full file':>10}{'full pool':>10}  full per role")
        for name in variants:
            m = r[name]
            fu = m.get("full", {})
            roles = " ".join(f"{k[:5]} {fu[k]:.0f}" for k in ROLES if k in fu)
            print(f"  {name:20}{m['coarse']['file_macro']:12.1f}{m['coarse']['pooled']:12.1f}"
                  f"{fu.get('file_macro', float('nan')):10.1f}{fu.get('pooled', float('nan')):10.1f}  {roles}")

    # Size sweep: smallest (coarse, fine) rounds within 0.3 of the best mean file-macro (coarse, all corpora).
    def mean_score(rc, rf):
        pp = predict(rc, rf)
        pp = np.where(gate, 2, np.where(totals, 4, pp))
        return float(np.mean([metrics(dt[(dt["source"] == c).to_numpy()], pp[(dt["source"] == c).to_numpy()], False)["file_macro"]
                              for c in np.unique(src)]))
    sweep = {}
    for frac in (0.15, 0.25, 0.4, 0.6, 1.0):
        rc, rf = max(5, round(rc_best * frac)), max(5, round(rf_best * frac))
        sweep[f"{rc}/{rf}"] = mean_score(rc, rf)
        print(f"  size {rc:4d} coarse / {rf:4d} fine rounds: mean coarse file-macro {sweep[f'{rc}/{rf}']:.2f}")
    best = max(sweep.values())
    pick = min((k for k, v in sweep.items() if v >= best - 0.3), key=lambda k: sum(map(int, k.split("/"))))
    rc, rf = map(int, pick.split("/"))
    report["size_sweep"] = sweep
    report["picked"] = {"coarse": rc, "fine": rf}
    print(f"picked {rc} coarse + {rf} fine rounds ({rc * 4 + rf * 7} trees)")

    coarse.save_model(args.out / "xgb_coarse.json")
    fine.save_model(args.out / "xgb_fine.json")
    for name, path, rounds, classes in (("coarse", "xgb_coarse.json", rc, NAMES), ("fine", "xgb_fine.json", rf, ROLES)):
        m = convert(args.out / path, rounds, classes)
        (args.out / f"{name}.json").write_text(json.dumps(m, separators=(",", ":")))
        # Self-check: our evaluator matches XGBoost on test segments.
        bst = coarse if name == "coarse" else fine
        sample = X[test][:3000]
        ours = scores(m, sample)
        theirs = bst.predict(xgb.DMatrix(sample, feature_names=feats), output_margin=True, iteration_range=(0, rounds))
        print(f"{name}.json: {len(m['trees'])} trees, max |diff| {np.abs(ours - theirs).max():.1e}, "
              f"same argmax {100 * (ours.argmax(1) == theirs.argmax(1)).mean():.2f}%")
    (args.out / "report.json").write_text(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
