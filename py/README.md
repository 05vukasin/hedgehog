# hedgehog-sheets

Turns messy Excel workbooks into clean, typed tables and a manifest that AI agents can work with.

hedgehog finds the tables on every sheet, flattens multi-row headers, parses numbers and dates
written in US or European formats, verifies totals and writes a normalized copy: one typed sheet per
table, a `Totals` sheet, a `Notes` sheet and a JSON manifest that traces every value back to its
source cell. The analysis is deterministic Rust; LLMs are optional. Pre-alpha.

```sh
pip install hedgehog-sheets                 # prebuilt wheels, no Rust needed
pip install "hedgehog-sheets[openai]"       # optional LLM providers: [openai] [anthropic] [bedrock] [llm]
```

```python
import hedgehog

r = hedgehog.normalize("report.xlsx", "out/")    # out/report.normalized.xlsx + out/report.manifest.json
df = r.read("T1")                                 # first table as a pandas DataFrame (needs pandas, openpyxl)
print(hedgehog.summarize("report.xlsx").text)     # deterministic summary, wrong totals first
```

Documentation, CLI, roadmap and source: <https://github.com/05vukasin/hedgehog>
(start with the [quickstart](https://github.com/05vukasin/hedgehog/blob/master/docs/quickstart.md)).
MIT license.
