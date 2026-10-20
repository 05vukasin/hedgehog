"""Convert a trained XGBoost multi-class model into hedgehog's compact tree format.

    python export_trees.py xgb_cell_roles.json cell_roles.json [--rounds N] [--check features.parquet]

Format (`hh-trees-1`, read by `hh_core::learned`):

    {"format": "hh-trees-1", "classes": [...], "features": [...], "base": [margin per class],
     "trees": [{"class": k, "f": [feature per node], "t": [threshold], "l": [left child or -1],
                "r": [right child], "v": [leaf value]}, ...]}

A node goes left when `x < t` (XGBoost's rule; features are never missing). The class score is
`base[k]` plus the leaf values of the class's trees; the predicted role is the highest score.
`--check` recomputes the scores in numpy on a sample and compares them with XGBoost's margins.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np

ROLES = ["title", "header", "data", "group_header", "total", "note", "other"]


def convert(model_path: Path, rounds: int | None, classes: list[str] | None = None) -> dict:
    m = json.loads(model_path.read_text())
    learner = m["learner"]
    n_class = int(learner["learner_model_param"]["num_class"])
    base = json.loads(learner["learner_model_param"]["base_score"])
    base = base if isinstance(base, list) else [base] * n_class
    gb = learner["gradient_booster"]["model"]
    trees, info = gb["trees"], gb["tree_info"]
    if rounds is not None:
        trees, info = trees[: rounds * n_class], info[: rounds * n_class]
    def f32(x) -> float:
        """The shortest decimal that reads back as the same float32 (smaller files, same model)."""
        return float(str(np.float32(x)))

    out = []
    for tree, cls in zip(trees, info):
        left = tree["left_children"]
        out.append({
            "class": cls,
            "f": [int(x) if left[i] != -1 else 0 for i, x in enumerate(tree["split_indices"])],
            "t": [f32(x) if left[i] != -1 else 0.0 for i, x in enumerate(tree["split_conditions"])],
            "l": left,
            "r": tree["right_children"],
            # A leaf's value is stored in split_conditions.
            "v": [f32(x) if left[i] == -1 else 0.0 for i, x in enumerate(tree["split_conditions"])],
        })
    classes = classes or ROLES[:n_class]
    if len(classes) != n_class:
        raise ValueError(f"model has {n_class} classes, {len(classes)} names given")
    return {"format": "hh-trees-1", "classes": classes, "features": learner["feature_names"],
            "base": [f32(b) for b in base], "trees": out}


def scores(model: dict, X: np.ndarray) -> np.ndarray:
    X = X.astype(np.float32)  # XGBoost compares in float32, as hh_core does
    s = np.tile(np.array(model["base"], dtype=np.float64), (len(X), 1))
    for t in model["trees"]:
        f, l, r, v = (np.array(t[k]) for k in ("f", "l", "r", "v"))
        th = np.array(t["t"], dtype=np.float32)
        node = np.zeros(len(X), dtype=np.int64)
        while True:
            inner = l[node] != -1
            if not inner.any():
                break
            go_left = X[np.arange(len(X)), f[node]] < th[node]
            node = np.where(inner, np.where(go_left, l[node], r[node]), node)
        s[:, t["class"]] += v[node]
    return s


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("model", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument("--rounds", type=int, help="keep the first N boosting rounds (default: best_iteration + 1)")
    ap.add_argument("--check", type=Path, help="features parquet to verify the scores on")
    ap.add_argument("--classes", help="comma-separated role names in class order (default: the 7 roles)")
    args = ap.parse_args()
    import xgboost as xgb

    bst = xgb.Booster()
    bst.load_model(args.model)
    rounds = args.rounds
    if rounds is None:
        best = bst.attributes().get("best_iteration")
        rounds = int(best) + 1 if best is not None else None
    model = convert(args.model, rounds, args.classes.split(",") if args.classes else None)
    args.out.write_text(json.dumps(model, separators=(",", ":")))
    print(f"{len(model['trees'])} trees, {sum(len(t['l']) for t in model['trees']):,} nodes, "
          f"{args.out.stat().st_size / 1e6:.2f} MB → {args.out}")
    if args.check:
        import pandas as pd

        df = pd.read_parquet(args.check).sample(n=5000, random_state=0)
        X = df[model["features"]].to_numpy(np.float32)
        ours = scores(model, X)
        theirs = bst.predict(xgb.DMatrix(X, feature_names=model["features"]), output_margin=True,
                             iteration_range=(0, rounds or 0))
        diff = np.abs(ours - theirs).max()
        agree = (ours.argmax(1) == theirs.argmax(1)).mean()
        print(f"check on 5,000 cells: max |score diff| {diff:.2e}, same prediction {100 * agree:.2f}%")


if __name__ == "__main__":
    main()
