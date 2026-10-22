# Developer shortcuts. Python and the CLI come from this repo (.venv, target-master).
#
# Testing in tiers (docs/development.md#testing-in-tiers):
#   make quick                     tier 0, after every edit: feature snapshots, mini golden, noise smoke
#   make quick-update              accept tier 0 changes (then review the diff of the snapshot files)
#   make merge-check AREA=agent    tier 2, when merging a branch: AREA=agent|core|docs (~5-10 min)
#   make full                      tier 3, once per batch or before a release (~1-2 h)
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

# Heavy tier steps: each under the memory cap, its output in $(LOGS)/<step>.log (the tail is shown).
RUN_CAPPED ?= ../scripts/run_capped.sh
LOGS       ?= $(abspath target-logs)
HH          = $(CARGO_TARGET_DIR)/release/hedgehog
# $(call capped,MEM,STEP,command...)
capped = mkdir -p $(LOGS) && : > $(LOGS)/$(2).log && echo "== $(2)" && \
	{ $(RUN_CAPPED) $(1) $(LOGS)/$(2).log $(3); rc=$$?; tail -n 12 $(LOGS)/$(2).log; \
	  [ $$rc -eq 0 ] || { echo "$(2) FAILED (log: $(LOGS)/$(2).log)"; exit $$rc; }; }

# One step at a time (memory), even under make -j.
.NOTPARALLEL:

.PHONY: help inspect llm ask compare manifest batch tables build cli test bench check-file \
        quick quick-update merge-check check-area full clippy golden benchmarks node-test py-test

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

# --- testing in tiers -----------------------------------------------------------------------------

# Tier 0: instant and deterministic, no LLM or network (scripts/quick.sh; its own target-quick/).
quick:
	@scripts/quick.sh

quick-update:
	@scripts/quick.sh --update

clippy:
	@$(call capped,2500M,clippy,cargo clippy --workspace -- -D warnings)

# Output hashes of 50 Enron files must not change (release CLI).
golden: cli
	@$(call capped,2500M,golden,$(HH) golden core/benches/enron50.txt --check core/benches/golden_enron50.txt)

# Both analyzers on all benchmark corpora; compare with docs/evaluation.md.
benchmarks: cli
	@$(call capped,2500M,benchmark-model,$(HH) benchmark --analyzer model --threads 2)
	@$(call capped,2500M,benchmark-layout,$(HH) benchmark --analyzer layout --threads 2)

node-test:
	@$(call capped,2500M,node,sh -c 'cd node && npm run build && npm test && npm run typecheck')

py-test: build
	@$(call capped,2500M,pytest,$(PY) -m pytest -q py/tests)

check-area:
	@case "$(AREA)" in core|agent|docs) ;; *) echo 'usage: make merge-check AREA=agent|core|docs'; exit 2;; esac

# Tier 2: when merging a branch; AREA is what it touched.
#   core  (core/: layout, values, totals, normalize) -> core + agent + cli tests, clippy, golden, both analyzers
#   agent (agent/, cli/)                              -> agent + cli tests, clippy, Node
#   docs  (docs/, eval notes, README)                 -> tier 0 only
merge-check: check-area
	@$(MAKE) --no-print-directory quick
ifeq ($(AREA),core)
	@$(call capped,4G,test-core,cargo test --release -p hh_core -p hh_agent -p hh_cli)
	@$(MAKE) --no-print-directory clippy golden benchmarks
endif
ifeq ($(AREA),agent)
	@$(call capped,4G,test-agent,cargo test --release -p hh_agent -p hh_cli)
	@$(MAKE) --no-print-directory clippy node-test
endif
	@echo "merge-check $(AREA): OK"

# Tier 3: everything except the LLM agent comparison (run that by hand, see eval/agents/README.md).
full:
	@$(MAKE) --no-print-directory quick
	@$(call capped,4G,test-workspace,cargo test --release --workspace --exclude hh_py)
	@$(MAKE) --no-print-directory clippy golden benchmarks
	@echo "== noise-sweep (caps its own jobs)" && eval/noise/sweep.sh 60 $(LOGS)/noise
	@$(call capped,2500M,file-sweep,python3 -I fuzz/sweep.py $(HH) $(HEDGEHOG_DATA) $(LOGS)/sweep.tsv)
	@$(call capped,4G,check-cases,env HEDGEHOG_BIN=$(HH) python3 -I eval/agents/check_cases.py)
	@$(MAKE) --no-print-directory node-test py-test
	@echo "full: OK; run the agent comparison too if the change affects answers (eval/agents/README.md)"
