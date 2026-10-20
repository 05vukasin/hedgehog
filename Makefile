# Developer shortcuts. Python and the CLI come from this repo (.venv, target-master).
#
#   make inspect FILE=path/to/file.xlsx            what hedgehog makes of a file, what an agent gets
#   make llm     FILE=...                          same, summary phrased by the LLM in .env
#   make ask     FILE=... Q="What is the total?"   an LLM agent answers with the workbook tools
#   make compare FILE=...                          rules + model vs rules alone
#   make manifest FILE=...                         the full manifest JSON
#   make batch   DIR=path/to/folder                one line per file
#   make tables  FILE=... ROWS=10                  only the tables, more rows
#
# Extra options for the inspector: ARGS="--rules-only --locale de-DE --open --tables all"
# (hedgehog-inspect --help lists them all).

PY      ?= .venv/bin/python
INSPECT  = $(PY) -m hedgehog.inspect
ROWS    ?= 5
DATA    ?= $(abspath ../testdata)
CAP      = ../scripts/run_capped.sh 4G /tmp/hedgehog-make.log
export HEDGEHOG_DATA ?= $(DATA)
export CARGO_TARGET_DIR ?= $(abspath target-master)

.PHONY: help inspect llm ask compare manifest batch tables build cli test bench check-file

help:
	@sed -n '1,/^$$/p' Makefile | sed 's/^# \{0,1\}//'

check-file:
	@test -n "$(FILE)" || { echo 'usage: make $(MAKECMDGOALS) FILE=path/to/file.xlsx'; exit 2; }

inspect: check-file
	@$(INSPECT) "$(FILE)" --rows $(ROWS) $(ARGS)

llm: check-file
	@$(INSPECT) "$(FILE)" --rows $(ROWS) --llm $(ARGS)

ask: check-file
	@test -n "$(Q)" || { echo 'usage: make ask FILE=... Q="your question"'; exit 2; }
	@$(INSPECT) "$(FILE)" --show overview --ask "$(Q)" $(ARGS)

compare: check-file
	@$(INSPECT) "$(FILE)" --show overview,tables --compare --rows $(ROWS) $(ARGS)

manifest: check-file
	@$(INSPECT) "$(FILE)" --show manifest $(ARGS)

tables: check-file
	@$(INSPECT) "$(FILE)" --show tables --rows $(ROWS) $(ARGS)

batch:
	@test -n "$(DIR)" || { echo 'usage: make batch DIR=path/to/folder'; exit 2; }
	@$(INSPECT) "$(DIR)" $(ARGS)

# Rebuild the Python extension into .venv (after Rust changes); memory-capped.
build:
	@$(CAP) sh -c 'cd py && ../.venv/bin/maturin develop --release -q' && echo "built (log: /tmp/hedgehog-make.log)"

# Rebuild the hedgehog command-line tool (target-master/release/hedgehog).
cli:
	@$(CAP) cargo build --release -p hh_cli && echo "built target-master/release/hedgehog"

test:
	@$(CAP) cargo test --release --workspace --exclude hh_py && $(PY) -m pytest -q py/tests

bench:
	@$(CAP) $(CARGO_TARGET_DIR)/release/hedgehog benchmark --analyzer model --threads 3; tail -40 /tmp/hedgehog-make.log
