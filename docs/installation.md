# Installation

How to install the Python package (prebuilt wheels, no Rust needed), the optional LLM extras, the
`hedgehog` command, and how to build everything from source.

| You want | Install |
|---|---|
| The Python library | a prebuilt wheel, [below](#python-package) |
| An LLM to phrase summaries | an [extra](#optional-extras), such as `hedgehog-sheets[openai]` |
| The `hedgehog` command | [build the CLI](#the-hedgehog-command) (needs Rust) |
| To change the code | [from source](#from-source), then [development.md](development.md) |

Requirements: CPython 3.9 or newer (one `abi3` wheel covers every version) on Linux (x86_64,
aarch64), macOS (Intel and Apple Silicon) or Windows (x64).

## Python package

The package is called **`hedgehog-sheets`** on the package index; you import it as `hedgehog`.
(The name `hedgehog` on PyPI belongs to an unrelated project.)

### From GitHub Releases

Prebuilt wheels are attached to every GitHub Release, starting with the first one (v0.1.0). Point pip
at the release page:

```sh
pip install hedgehog-sheets --find-links https://github.com/05vukasin/hedgehog/releases/expanded_assets/v0.1.0
```

pip picks the wheel for your platform from that page. No Rust toolchain is needed. Replace `v0.1.0`
with a later tag to get a later release; the [releases page](https://github.com/05vukasin/hedgehog/releases)
lists them. You can also download the `.whl` file for your platform and run `pip install` on it.

### From PyPI

Coming soon. Once published, it will be:

```sh
pip install hedgehog-sheets
```

### Check the install

```python
import hedgehog
print(hedgehog.__all__)
```

```text
['NormalizeResult', 'Summary', 'Workbook', 'check_citations', 'facts', 'facts_json', 'normalize', 'open', 'summarize']
```

For the examples in these docs you also want pandas and openpyxl, which hedgehog does not require:

```sh
pip install pandas openpyxl
```

## Optional extras

hedgehog itself has no Python dependencies. LLM providers are optional extras:

| Extra | Installs | For |
|---|---|---|
| `hedgehog-sheets[openai]` | `openai` | OpenAI and OpenAI-compatible servers: vLLM, Ollama, LM Studio, OpenRouter, Groq |
| `hedgehog-sheets[anthropic]` | `anthropic` | Anthropic |
| `hedgehog-sheets[bedrock]` | `boto3` | AWS Bedrock |
| `hedgehog-sheets[llm]` | all three | Comparing providers |

```sh
pip install "hedgehog-sheets[openai]" --find-links https://github.com/05vukasin/hedgehog/releases/expanded_assets/v0.1.0
```

(Quote the name in shells such as zsh, where `[` is special.) See [llm.md](llm.md) for API keys.

## The `hedgehog` command

The wheel contains the Python library only. The command-line tool is a separate Rust binary; build
it with a [Rust toolchain](https://rustup.rs/):

```sh
cargo install --git https://github.com/05vukasin/hedgehog hh_cli
hedgehog describe report.xlsx
```

`cargo install` builds the `hedgehog` binary with the release profile and puts it in `~/.cargo/bin`.
From a clone, `cargo build --release -p hh_cli` builds `target/release/hedgehog`.

## From source

You need Rust (stable) and Python 3.9 or newer.

```sh
git clone https://github.com/05vukasin/hedgehog
cd hedgehog
python -m venv .venv && . .venv/bin/activate
pip install ./py                  # builds the extension with maturin, release profile
```

`pip install ./py` compiles the Rust core, which takes a few minutes the first time. For development
(editable install, faster debug builds) use `maturin develop` as described in
[development.md](development.md#build).

## See also

- [quickstart.md](quickstart.md): first steps after installing
- [troubleshooting.md](troubleshooting.md#installation): installation problems
- [development.md](development.md): building, testing, releasing
