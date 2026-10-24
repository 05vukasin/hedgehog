# Test data: where it is and where it came from

All corpora live **outside git** in `../testdata/` (i.e. `/home/vukasin/projects/excell-lib-project/testdata`),
about 4.6 GB. **Everything below is already downloaded — check here before fetching anything.**
Untrusted data: parse only, run Python that reads it with `python3 -I`, keep scripts outside the data folders,
never execute or enable macros in downloaded files. Default data root for tools: `$HEDGEHOG_DATA` (defaults
to `../testdata`).

| Folder | What | Provenance (per file) |
|---|---|---|
| `testdata/irregular/` (≈ 25.6k files, 3.9 GB) | Real messy spreadsheets: annotated research corpora (DECO, Mondrian/FUSTE, TableSense annotations, VEnron2/VEUSES/VFUSE, Strudel SAUS/CIUS/DeEx/Troy, StatSheets, Auto-Tables) and raw government/open-data files from ~50 publishers (US BLS/BEA/Census/EIA/SEC/USDA, UK ONS/HMT/gov, Eurostat, Destatis, INSEE, ISTAT, INE, CBS, SSB, SCB, GUS, Statistik Austria, BFS, StatCan, Serbian budgets, …) | `irregular/MANIFEST.csv` (path, source, url, license, format, size, description, has_annotations) and `irregular/README_sources.md` |
| `testdata/clean/` (96 files) | Well-structured datasets (Northwind, Superstore, Microsoft financial sample, World Bank, OWID, Chicago/NYC open data, …): input to the noise generator and eval cases | `clean/MANIFEST.csv` |
| `testdata/synthetic/` (≈ 4.1k files) | Generated: clean tables + injected noise with ground truth. Never edit by hand; regenerate with the noise generator | generated |
| `testdata/reference/` | Dictionaries for semantic typing (GeoNames, ISO 3166/4217/639, Italy, Serbia) | per-folder files |
| `testdata/public/` (≈ 13.4k files, 287 MB) | Public QA benchmarks: MiMoTable, RealHiTBench, HiTab (+ TableSense pointer, seattle-weather) | `public/README.md`; fetch scripts + checksums in `eval/public/` |

## Which corpus is used where

| Use | Corpora |
|---|---|
| Structure benchmarks (`hedgehog benchmark`) | DECO, StatSheets, Strudel, synthetic, EU labelled sample |
| Cell fidelity (`make fidelity`) | public (MiMoTable, RealHiTBench, HiTab), TableSense/VEnron, eval/agents cases, golden files |
| Accuracy scoreboard (`make score`) | public, dev split only (`eval/accuracy/split/`) |
| Agent eval cases | `eval/agents/cases.json` (files under irregular/ and clean/) |
| Golden / quick slices | `eval/quick/`, `core/benches/golden_enron50.txt` |

## Licenses that restrict use

- RealHiTBench: CC BY-NC 4.0 — evaluation only; never train a shipped model on it.
- Mondrian: GPL-3.0 — annotations used for evaluation only; no code copied.
- HiTab: C-UDA — computational use (evaluation and training) allowed.
- Spreadsheets themselves: never committed to git (see .gitignore).
