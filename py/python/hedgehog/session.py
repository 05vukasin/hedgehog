r"""The agent tools in-process: the catalog and read-only SQL of `hedgehog mcp`, without a server.

```python
s = hedgehog.Session("/data/spreadsheets")
print(s.open_file("sales.xlsx", question="North revenue in Q1?"))
print(s.query("sales.xlsx", "SELECT SUM(\"Revenue / Q1\") FROM T1 WHERE Region = 'North'"))
```
"""

from __future__ import annotations

import json
import os
from typing import Any, Sequence, Union

from ._native import NativeSession, tool_specs_json

PathLike = Union[str, "os.PathLike[str]"]


class Session:
    """One agent's tool session: file access bounded by `root` (plus `allow`), normalized files and
    their SQLite tables cached by content. Every method returns the tool's compact text (at most
    ~12,000 characters); a tool error raises `ValueError` with a one-line message the model can act on.

    Limits (all optional): `max_cells` per workbook (default 10,000,000), `max_seconds` to read and
    normalize one file (default 120), `max_file_mb` (default 256), `query_timeout_ms` per SQL query
    (default 2,000).

    Opened files are kept in a disk cache by content hash (`cache_dir`, default from
    `HEDGEHOG_CACHE_DIR` or the user's cache folder), so a second session opens them in
    milliseconds; `cache=False` turns it off. A file found in the cache skips the limits, since it
    was read once already. A session is safe to share between threads; calls run one at a time and release
    the GIL.
    """

    def __init__(
        self,
        root: PathLike,
        allow: Sequence[PathLike] = (),
        *,
        max_cells: int | None = None,
        max_seconds: float | None = None,
        max_file_mb: int | None = None,
        query_timeout_ms: int | None = None,
        cache: bool = True,
        cache_dir: PathLike | None = None,
    ) -> None:
        self._s = NativeSession(
            os.fspath(root),
            [os.fspath(p) for p in allow],
            max_cells=max_cells,
            max_seconds=max_seconds,
            max_file_mb=max_file_mb,
            query_timeout_ms=query_timeout_ms,
            cache=cache,
            cache_dir=None if cache_dir is None else os.fspath(cache_dir),
        )

    @property
    def root(self) -> str:
        """The canonical root folder."""
        return self._s.root

    def allow(self, path: PathLike) -> None:
        """Allows one more file or folder outside the root."""
        self._s.allow(os.fspath(path))

    def open_file(self, path: PathLike, question: str | None = None) -> str:
        """The file's catalog. With the user's `question`, the tables it is about come first."""
        return self._s.open_file(os.fspath(path), question)

    def describe_table(self, path: PathLike, table: str) -> str:
        """Every column of one table (`T1`, …) with its source cells, and its first rows."""
        return self._s.describe_table(os.fspath(path), table)

    def query(self, path: PathLike, sql: str, limit: int | None = None) -> str:
        """One read-only SQLite `SELECT`/`WITH` as CSV (default 50 rows, at most 200)."""
        return self._s.query(os.fspath(path), sql, limit)

    def cells(self, path: PathLike, sheet: str, range: str) -> str:  # noqa: A002 - the tool's argument name
        """Source cells of a range (at most 200), with formulas."""
        return self._s.cells(os.fspath(path), sheet, range)

    def totals(self, path: PathLike, table: str | None = None, status: str | None = None) -> str:
        """Total rows and whether they add up (`status`: ok, mismatch or unknown)."""
        return self._s.totals(os.fspath(path), table, status)

    def list_files(self, folder: PathLike | None = None, pattern: str | None = None) -> str:
        """Spreadsheets under a folder of the root."""
        return self._s.list_files(None if folder is None else os.fspath(folder), pattern)

    def open_files(self) -> list[tuple[str, str]]:
        """`(prefix, path)` of the files opened so far; `f2.T1` names table T1 of the second file."""
        return self._s.open_files()

    def call(self, name: str, args: dict[str, Any] | None = None) -> str:
        """Runs a tool by its name in `tool_specs()` with the model's arguments."""
        return self._s.call(name, json.dumps(args or {}))


def tool_specs() -> list[dict[str, Any]]:
    """The tools as MCP `tools/list` entries: `{name, title, description, inputSchema, annotations}`.
    Map them to your model provider's tool format and dispatch calls with `Session.call`."""
    return json.loads(tool_specs_json())
