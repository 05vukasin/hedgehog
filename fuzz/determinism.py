"""Determinism sweep: `hedgehog normalize` each file several times and compare the outputs byte for byte.

usage: python3 -I determinism.py <hedgehog binary> <dir|filelist> <results.tsv>
                                 [--runs N] [--single N] [--limit N] [--timeout S] [--work DIR]

Each file is normalized `--runs` times (default 3) as is (sheets analyzed on parallel threads) and
`--single` times (default 0) pinned to one CPU with `taskset -c 0` (one analysis thread), each run
into a fresh output folder under `--work` (default: next to the results file; deleted after
hashing). The normalized xlsx and the manifest must be identical across all runs: the xlsx byte for
byte (and every zip member, to name the part that differs) and the manifest after dropping the
fields listed in RUN_METADATA (none today; see docs/normalized-output.md).

Results (resumable TSV): path, outcome, runs, sha256 of the manifest, sha256 of the xlsx, detail.
Outcomes: same, DIFF (with the differing parts), error (normalize failed in every run: not a
determinism finding), MIXED (failed in some runs only), timeout. Exit code 1 if any file is DIFF
or MIXED. A file list holds one path per line; a folder is walked for spreadsheet files.
"""
import hashlib, json, os, resource, shutil, subprocess, sys, zipfile

# Manifest fields (dotted paths) that are run metadata and may differ between runs. Empty: the
# manifest holds no time, host or temp-path field. Keep in sync with docs/normalized-output.md.
RUN_METADATA: list = []

EXTS = {"xlsx", "xlsm", "xls", "xlsb", "ods", "csv", "tsv"}


def args():
    a = sys.argv[1:]
    if len(a) < 3:
        sys.exit(__doc__)
    opts = {"--runs": 3, "--single": 0, "--limit": 0, "--timeout": 300, "--work": None}
    rest = a[3:]
    for i in range(0, len(rest), 2):
        k = rest[i]
        if k not in opts or i + 1 >= len(rest):
            sys.exit(f"unknown or incomplete option {k}\n{__doc__}")
        opts[k] = rest[i + 1] if k == "--work" else int(rest[i + 1])
    return a[0], a[1], a[2], opts


def files_of(src):
    if os.path.isfile(src):
        with open(src) as fh:
            return [l.strip() for l in fh if l.strip() and not l.startswith("#")]
    out = []
    for root, dirs, fs in os.walk(src):
        dirs.sort()
        out += [os.path.join(root, f) for f in sorted(fs) if f.rsplit(".", 1)[-1].lower() in EXTS]
    return out


def cap():
    lim = int(2.3 * (1 << 30))
    resource.setrlimit(resource.RLIMIT_DATA, (lim, lim))


def drop(obj, dotted):
    parts = dotted.split(".")
    for p in parts[:-1]:
        obj = obj.get(p) if isinstance(obj, dict) else None
        if obj is None:
            return
    if isinstance(obj, dict):
        obj.pop(parts[-1], None)


def digest(out_dir):
    """(manifest sha, xlsx sha, {zip member: sha}) of one run's output."""
    man = next((f for f in os.listdir(out_dir) if f.endswith(".manifest.json")), None)
    xl = next((f for f in os.listdir(out_dir) if f.endswith(".normalized.xlsx")), None)
    if not man or not xl:
        return None
    raw = open(os.path.join(out_dir, man), "rb").read()
    if RUN_METADATA:
        j = json.loads(raw)
        for f in RUN_METADATA:
            drop(j, f)
        raw = json.dumps(j, sort_keys=True).encode()
    xb = open(os.path.join(out_dir, xl), "rb").read()
    members = {}
    with zipfile.ZipFile(os.path.join(out_dir, xl)) as z:
        for info in z.infolist():
            members[info.filename] = hashlib.sha256(z.read(info)).hexdigest()
    return hashlib.sha256(raw).hexdigest(), hashlib.sha256(xb).hexdigest(), members


def run_once(binary, f, out_dir, single, timeout):
    shutil.rmtree(out_dir, ignore_errors=True)
    cmd = (["taskset", "-c", "0"] if single else []) + [binary, "normalize", f, "-o", out_dir]
    try:
        p = subprocess.run(cmd, capture_output=True, timeout=timeout, preexec_fn=cap)
    except subprocess.TimeoutExpired:
        return "timeout"
    if p.returncode != 0:
        err = p.stderr.decode("utf-8", "replace").strip().splitlines()
        return "error: " + (err[0] if err else f"rc {p.returncode}")
    d = digest(out_dir)
    shutil.rmtree(out_dir, ignore_errors=True)
    return d if d else "error: no output"


def compare(results):
    ok = [r for r in results if isinstance(r, tuple)]
    if not ok:
        return ("timeout" if all(r == "timeout" for r in results) else "error"), str(results[0])[:200], "", ""
    if len(ok) < len(results):
        return "MIXED", "; ".join(sorted({r if isinstance(r, str) else "ok" for r in results}))[:300], "", ""
    man, xl, mem = ok[0]
    diffs = []
    if any(r[0] != man for r in ok):
        diffs.append("manifest")
    if any(r[1] != xl for r in ok):
        names = sorted(set().union(*(r[2].keys() for r in ok)))
        parts = [n for n in names if len({r[2].get(n) for r in ok}) > 1]
        diffs.append("xlsx[" + ",".join(parts) + "]")
    return ("DIFF" if diffs else "same"), " ".join(diffs), man, xl


def main():
    binary, src, results, o = args()
    binary = os.path.abspath(binary)
    files = files_of(src)
    if o["--limit"]:
        files = files[: o["--limit"]]
    done = set()
    if os.path.exists(results):
        with open(results) as fh:
            done = {l.split("\t", 1)[0] for l in fh}
    work = o["--work"] or os.path.join(os.path.dirname(os.path.abspath(results)), "determinism_out")
    bad = 0
    with open(results, "a") as res:
        for n, f in enumerate(files, 1):
            if f in done:
                continue
            plan = [False] * o["--runs"] + [True] * o["--single"]
            runs = [run_once(binary, f, os.path.join(work, "run"), s, o["--timeout"]) for s in plan]
            outcome, detail, man, xl = compare(runs)
            bad += outcome in ("DIFF", "MIXED")
            res.write(f"{f}\t{outcome}\t{len(plan)}\t{man}\t{xl}\t{detail}\n")
            res.flush()
            if outcome in ("DIFF", "MIXED"):
                print(f"{outcome} {f}: {detail}", flush=True)
            if n % 50 == 0:
                print(f"{n}/{len(files)} files", flush=True)
    shutil.rmtree(work, ignore_errors=True)
    print(f"done: {len(files)} files, {bad} with differences", flush=True)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
