"""Shared download helper for the eval/public fetch scripts (stdlib only, no execution of data).

Each fetch script lists (url, relative destination) pairs at a pinned revision. `fetch()` downloads
what is missing, checks every file against `checksums/<dataset>.sha256` and, the first time a file
is seen (no line in the checksum file yet), records its sha256 there (`--record`). Re-running is a
no-op when every file is present and matches. Archives are extracted with path checks (no absolute
paths, no `..`, no links) into the dataset folder; nothing in the data is ever executed.
"""

from __future__ import annotations

import hashlib
import os
import sys
import tarfile
import time
import urllib.request
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
UA = "hedgehog-public-bench/1 (+https://github.com/05vukasin/hedgehog)"


def data_root() -> Path:
    root = os.environ.get("HEDGEHOG_DATA") or str(HERE.parent.parent.parent / "testdata")
    return Path(root) / "public"


def sha256(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def _load_sums(name: str) -> dict[str, str]:
    p = HERE / "checksums" / f"{name}.sha256"
    if not p.exists():
        return {}
    out = {}
    for line in p.read_text().splitlines():
        if line.strip():
            h, rel = line.split("  ", 1)
            out[rel] = h
    return out


def _save_sums(name: str, sums: dict[str, str]) -> None:
    p = HERE / "checksums" / f"{name}.sha256"
    p.parent.mkdir(exist_ok=True)
    p.write_text("".join(f"{h}  {rel}\n" for rel, h in sorted(sums.items())))


def _download(url: str, dst: Path, tries: int = 4) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp = dst.with_name(dst.name + ".part")
    for i in range(tries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=120) as r, open(tmp, "wb") as f:
                while chunk := r.read(1 << 20):
                    f.write(chunk)
            tmp.replace(dst)
            return
        except Exception as e:  # noqa: BLE001
            if i == tries - 1:
                raise RuntimeError(f"{url}: {e}") from e
            time.sleep(2 * (i + 1))


def fetch(name: str, items: list[tuple[str, str]], record: bool) -> Path:
    """Download `items` [(url, rel)] into <data>/public/<name>/ and verify checksums."""
    root = data_root() / name
    root.mkdir(parents=True, exist_ok=True)
    sums = _load_sums(name)
    changed = bad = 0
    for k, (url, rel) in enumerate(items, 1):
        dst = root / rel
        if not dst.exists():
            print(f"[{k}/{len(items)}] {rel}", file=sys.stderr)
            _download(url, dst)
        h = sha256(dst)
        if rel not in sums:
            if not record:
                raise SystemExit(f"{rel}: no recorded checksum (run with --record once to pin it)")
            sums[rel] = h
            changed += 1
        elif sums[rel] != h:
            print(f"CHECKSUM MISMATCH {rel}: {h} != {sums[rel]}", file=sys.stderr)
            bad += 1
    if changed:
        _save_sums(name, sums)
    if bad:
        raise SystemExit(f"{bad} file(s) failed the checksum")
    print(f"{name}: {len(items)} files ok in {root}", file=sys.stderr)
    return root


def _safe(member: str, dest: Path) -> Path:
    p = (dest / member).resolve()
    if member.startswith("/") or ".." in Path(member).parts or not str(p).startswith(str(dest.resolve())):
        raise ValueError(f"unsafe path in archive: {member!r}")
    return p


def extract(archive: Path, dest: Path, strip: int = 0) -> int:
    """Extract regular files only (zip or tar.gz) under `dest`, dropping `strip` leading parts."""
    dest.mkdir(parents=True, exist_ok=True)
    n = 0
    if zipfile.is_zipfile(archive):
        with zipfile.ZipFile(archive) as z:
            for info in z.infolist():
                if info.is_dir():
                    continue
                name = info.filename
                try:  # zips made on Windows/macOS with non-UTF-8 names
                    name = name.encode("cp437").decode("utf-8") if not info.flag_bits & 0x800 else name
                except UnicodeError:
                    pass
                parts = Path(name).parts[strip:]
                if not parts or "__MACOSX" in parts:
                    continue
                out = _safe(str(Path(*parts)), dest)
                out.parent.mkdir(parents=True, exist_ok=True)
                with z.open(info) as src, open(out, "wb") as f:
                    while chunk := src.read(1 << 20):
                        f.write(chunk)
                n += 1
    else:
        with tarfile.open(archive) as t:
            for m in t.getmembers():
                if not m.isfile():
                    continue
                parts = Path(m.name).parts[strip:]
                if not parts:
                    continue
                out = _safe(str(Path(*parts)), dest)
                out.parent.mkdir(parents=True, exist_ok=True)
                with t.extractfile(m) as src, open(out, "wb") as f:  # type: ignore[union-attr]
                    while chunk := src.read(1 << 20):
                        f.write(chunk)
                n += 1
    return n
