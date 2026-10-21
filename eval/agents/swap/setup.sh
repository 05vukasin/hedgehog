#!/usr/bin/env bash
# Install the tool-swap arms' third-party tools, pinned and isolated under $SWAP_DIR
# (default /tmp/claude-1000/w1swap); nothing is installed globally and no install scripts run.
#   - venv-pandas:     the pandas arm's interpreter (sandboxed at run time)
#   - venv-markitdown: MarkItDown + tiktoken for the markdown arm (sandboxed at run time)
#   - excelmcp:        excel-csv-mcp-server 1.0.2 (github.com/ishayoyo/excel-mcp, MIT), npm, --ignore-scripts
set -euo pipefail
dir=${SWAP_DIR:-/tmp/claude-1000/w1swap}
mkdir -p "$dir"
cd "$dir"
[ -x venv-pandas/bin/python ] || uv venv -q --python 3.12 venv-pandas
uv pip install -q --python venv-pandas/bin/python pandas==2.2.3 numpy==2.2.6 openpyxl==3.1.5 python-calamine==0.4.0 xlrd==2.0.1
[ -x venv-markitdown/bin/python ] || uv venv -q --python 3.12 venv-markitdown
uv pip install -q --python venv-markitdown/bin/python "markitdown[xlsx,xls]==0.1.3" tiktoken==0.9.0
mkdir -p excelmcp/inst
cd excelmcp
[ -f excel-csv-mcp-server-1.0.2.tgz ] || npm pack excel-csv-mcp-server@1.0.2 --silent
cd inst
[ -f package.json ] || echo '{"name":"swap-excelmcp","private":true}' > package.json
npm install --ignore-scripts --no-audit --no-fund --save-exact ../excel-csv-mcp-server-1.0.2.tgz
