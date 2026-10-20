"""Train the learned cell-role model (PLAN.md M6) on `hedgehog features` output.

    hedgehog features --out cells.csv                 # DECO + synthetic, labelled cells
    python train_cell_roles.py cells.csv --out models/ [--device cuda] [--threads 8]

Train: DECO `dev` split + all synthetic sheets, minus a group holdout of DECO-dev files used for
early stopping. Test: DECO `test` split (files never seen in training). Data cells were sampled at
export time; their `weight` restores the true class balance in every metric printed here.

Prints, on the test split, the rule layout (`l_role`, what `normalize` uses today) next to XGBoost
and LightGBM: pooled F1 per role and macro, plus file-macro F1 (mean over files of the per-file
macro F1 over the roles present in that file, as `hedgehog eval` reports it). Writes the models as
JSON (`xgb_cell_roles.json`, `lgb_cell_roles.json`) and `report.json`.
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import numpy as np
import pandas as pd

ROLES = ["title", "header", "data", "group_header", "total", "note", "other"]
META = ["source", "file", "sheet", "row", "col", "split", "label", "weight"]


def load(path: Path) -> pd.DataFrame:
    pq = path.with_suffix(".parquet")
    if path.suffix == ".parquet":
        return pd.read_parquet(path)
    if pq.exists() and pq.stat().st_mtime >= path.stat().st_mtime:
        return pd.read_parquet(pq)
    df = pd.read_csv(path, dtype={"file": str, "sheet": str, "source": str, "split": str})
    feats = [c for c in df.columns if c not in META]
    df[feats] = df[feats].astype(np.float32)
    df.to_parquet(pq, compression="zstd")
    return df


def f1_table(y: np.ndarray, p: np.ndarray, w: np.ndarray) -> dict:
    out = {}
    for k, name in enumerate(ROLES):
        tp = w[(y == k) & (p == k)].sum()
        fp = w[(y != k) & (p == k)].sum()
        fn = w[(y == k) & (p != k)].sum()
        prec = tp / (tp + fp) if tp + fp else 0.0
        rec = tp / (tp + fn) if tp + fn else 0.0
        out[name] = float(100 * (2 * prec * rec / (prec + rec) if prec + rec else 0.0))
    out["macro"] = float(np.mean([out[r] for r in ROLES]))
    return out


def file_macro(df: pd.DataFrame, p: np.ndarray) -> float:
    scores = []
    for _, idx in df.groupby("file").indices.items():
        y, q, w = df["label"].values[idx], p[idx], df["weight"].values[idx]
        per = []
        for k in np.unique(y):
            tp = w[(y == k) & (q == k)].sum()
            fp = w[(y != k) & (q == k)].sum()
            fn = w[(y == k) & (q != k)].sum()
            per.append(2 * tp / (2 * tp + fp + fn) if tp else 0.0)
        scores.append(np.mean(per))
    return 100 * float(np.mean(scores))


def class_weights(y: np.ndarray, w: np.ndarray, power: float) -> np.ndarray:
    """Inverse class frequency to the given power: rare roles (title, note) count more."""
    freq = np.array([w[y == k].sum() for k in range(len(ROLES))])
    cw = (freq.sum() / np.maximum(freq, 1.0)) ** power
    return cw / cw[2]  # data = 1


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("csv", type=Path)
    ap.add_argument("--out", type=Path, default=Path("models"))
    ap.add_argument("--device", default="cpu", help="xgboost device: cpu or cuda")
    ap.add_argument("--threads", type=int, default=6)
    ap.add_argument("--rounds", type=int, default=2000)
    ap.add_argument("--class-power", type=float, default=0.5)
    ap.add_argument("--no-lgb", action="store_true")
    ap.add_argument("--eta", type=float, default=0.08)
    ap.add_argument("--depth", type=int, default=8)
    ap.add_argument("--no-synthetic", action="store_true", help="train on DECO only")
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)

    t = time.time()
    df = load(args.csv)
    feats = [c for c in df.columns if c not in META]
    print(f"{len(df):,} rows, {len(feats)} features, loaded in {time.time() - t:.1f}s")

    deco = df["source"] == "deco"
    test = deco & (df["split"] == "test")
    dev_files = df.loc[deco & (df["split"] == "dev"), "file"].unique()
    rng = np.random.default_rng(0)
    valid_files = set(rng.choice(dev_files, size=max(1, len(dev_files) // 7), replace=False))
    valid = deco & df["file"].isin(valid_files) & ~test
    train = ~test & ~valid
    if args.no_synthetic:
        train &= deco
    print(f"train {train.sum():,} rows · valid {valid.sum():,} ({len(valid_files)} DECO files) · test {test.sum():,}")

    X = df[feats].to_numpy(np.float32)
    y = df["label"].to_numpy(np.int32)
    w = df["weight"].to_numpy(np.float32)
    cw = class_weights(y[train], w[train], args.class_power)
    sw = w * cw[y]
    print("class weights:", {r: round(float(c), 2) for r, c in zip(ROLES, cw)})

    report: dict = {"rows": int(len(df)), "features": feats, "test_rows": int(test.sum())}
    dt = df[test].reset_index(drop=True)
    rules = df.loc[test, "l_role"].to_numpy().astype(int)
    rules = np.where(rules < 0, ROLES.index("other"), rules)
    report["rules"] = {**f1_table(y[test], rules, w[test]), "file_macro": file_macro(dt, rules)}

    import xgboost as xgb

    t = time.time()
    dtrain = xgb.DMatrix(X[train], label=y[train], weight=sw[train], feature_names=feats)
    dvalid = xgb.DMatrix(X[valid], label=y[valid], weight=sw[valid], feature_names=feats)
    params = {
        "objective": "multi:softprob",
        "num_class": len(ROLES),
        "tree_method": "hist",
        "device": args.device,
        "max_depth": args.depth,
        "eta": args.eta,
        "subsample": 0.8,
        "colsample_bytree": 0.7,
        "min_child_weight": 2,
        "eval_metric": "mlogloss",
        "nthread": args.threads,
    }
    bst = xgb.train(params, dtrain, args.rounds, evals=[(dvalid, "valid")], early_stopping_rounds=60, verbose_eval=200)
    pxgb = bst.predict(xgb.DMatrix(X[test], feature_names=feats), iteration_range=(0, bst.best_iteration + 1)).argmax(1)
    bst.save_model(args.out / "xgb_cell_roles.json")
    report["xgboost"] = {**f1_table(y[test], pxgb, w[test]), "file_macro": file_macro(dt, pxgb),
                         "trees": int(bst.best_iteration + 1), "train_s": round(time.time() - t, 1)}
    gain = bst.get_score(importance_type="gain")
    report["xgboost_top_features"] = sorted(gain, key=gain.get, reverse=True)[:20]

    if not args.no_lgb:
        import lightgbm as lgb

        t = time.time()
        m = lgb.train(
            {"objective": "multiclass", "num_class": len(ROLES), "learning_rate": 0.08, "num_leaves": 127,
             "min_data_in_leaf": 20, "feature_fraction": 0.7, "bagging_fraction": 0.8, "bagging_freq": 1,
             "num_threads": args.threads, "verbose": -1},
            lgb.Dataset(X[train], y[train], weight=sw[train], feature_name=feats),
            args.rounds,
            valid_sets=[lgb.Dataset(X[valid], y[valid], weight=sw[valid], feature_name=feats)],
            callbacks=[lgb.early_stopping(60, verbose=False), lgb.log_evaluation(200)],
        )
        plgb = m.predict(X[test], num_iteration=m.best_iteration).argmax(1)
        (args.out / "lgb_cell_roles.json").write_text(json.dumps(m.dump_model()))
        report["lightgbm"] = {**f1_table(y[test], plgb, w[test]), "file_macro": file_macro(dt, plgb),
                              "trees": int(m.best_iteration), "train_s": round(time.time() - t, 1)}

    (args.out / "report.json").write_text(json.dumps(report, indent=2))
    cols = ["rules", "xgboost"] + ([] if args.no_lgb else ["lightgbm"])
    print(f"\nDECO test split ({test.sum():,} cells, weighted), F1 %")
    print(f"{'':14}" + "".join(f"{c:>10}" for c in cols))
    for k in ROLES + ["macro", "file_macro"]:
        print(f"{k:14}" + "".join(f"{report[c][k]:10.1f}" for c in cols))
    for c in cols[1:]:
        print(f"{c}: {report[c]['trees']} trees, {report[c]['train_s']} s")
    print("top features:", ", ".join(report["xgboost_top_features"][:10]))


if __name__ == "__main__":
    main()
