"""Tiny dependency-free ``.env`` support. Values are never logged."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Dict, List, Mapping, Optional, Union

PathLike = Union[str, "os.PathLike[str]"]


def parse_env(text: str) -> Dict[str, str]:
    """``KEY=VALUE`` lines; ``#`` comments, ``export`` prefix, single/double quotes. Empty values are
    skipped (an empty ``OPENAI_API_KEY=`` in a template means "not set")."""
    out: Dict[str, str] = {}
    for line in text.splitlines():
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        if s.startswith("export "):
            s = s[7:].lstrip()
        key, sep, value = s.partition("=")
        key = key.strip()
        if not sep or not key or not key.replace("_", "").isalnum():
            continue
        value = value.strip()
        if value[:1] in ("'", '"'):
            q = value[0]
            end = value.find(q, 1)
            value = value[1:end] if end != -1 else value[1:]
            if q == '"':
                value = value.replace("\\n", "\n").replace('\\"', '"')
        else:
            hash_at = value.find(" #")
            if hash_at != -1:
                value = value[:hash_at]
            value = value.strip()
        if value:
            out[key] = value
    return out


def find_env_file(path: Optional[PathLike] = None) -> Optional[Path]:
    """``path`` if given; else ``./.env``, then the nearest ``.env`` above the installed package
    (the repo root in a development checkout)."""
    if path is not None:
        p = Path(path)
        return p if p.is_file() else None
    cwd = Path.cwd() / ".env"
    if cwd.is_file():
        return cwd
    for parent in Path(__file__).resolve().parents:
        cand = parent / ".env"
        if cand.is_file():
            return cand
        if (parent / ".git").exists():  # stop at the repository root
            break
    return None


def read_env(path: Optional[PathLike] = None) -> Dict[str, str]:
    """Values from the ``.env`` file (see :func:`find_env_file`) without touching ``os.environ``."""
    f = find_env_file(path)
    if f is None:
        if path is not None:
            raise FileNotFoundError(f"env file not found: {path}")
        return {}
    return parse_env(f.read_text(encoding="utf-8"))


def load_env(path: Optional[PathLike] = None, override: bool = False) -> List[str]:
    """Loads a ``.env`` file into ``os.environ``. Existing variables win unless ``override``.
    Returns the names that were set (never the values)."""
    values = read_env(path)
    set_names = []
    for k, v in values.items():
        if override or not os.environ.get(k):
            os.environ[k] = v
            set_names.append(k)
    return set_names


class Env:
    """Lookup chain: ``os.environ`` first, then an optional ``.env`` dict / mapping."""

    def __init__(self, extra: Optional[Mapping[str, str]] = None, use_os: bool = True) -> None:
        self.extra = dict(extra or {})
        self.use_os = use_os

    def get(self, name: str) -> Optional[str]:
        if not name:
            return None
        if self.use_os:
            v = os.environ.get(name)
            if v and v.strip():
                return v.strip()
        v = self.extra.get(name)
        return v.strip() if v and v.strip() else None
