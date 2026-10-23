#!/usr/bin/env bash
# The pinned readers of the cell-fidelity gate (eval/fidelity/check.py): openpyxl, xlrd, odfpy in
# their own venv, never installed globally. Default place: ../scratch-archive/fidelity/venv next to
# the repository (or $HH_FIDELITY_VENV). Idempotent.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
VENV=${HH_FIDELITY_VENV:-$(dirname "$ROOT")/scratch-archive/fidelity/venv}
if [ -x "$VENV/bin/python" ] && "$VENV/bin/python" -c "import openpyxl, xlrd, odf" 2>/dev/null; then
  echo "$VENV"; exit 0
fi
mkdir -p "$(dirname "$VENV")"
uv venv -q "$VENV"
uv pip install -q --python "$VENV/bin/python" openpyxl==3.1.5 xlrd==2.0.2 odfpy==1.4.1
echo "$VENV"
