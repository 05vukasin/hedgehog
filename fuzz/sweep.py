"""Sweep: `hedgehog normalize` over every spreadsheet file under a folder, one file at a time.

usage: python3 -I sweep.py <hedgehog binary> <data dir> <results.tsv> [--limit N] [--timeout S]

Each file runs in its own process (address space capped at 2.5 GB, so a runaway allocation kills
only that child) with a wall-clock timeout. Results (resumable): path, outcome, seconds, rc, last
stderr line. Outcomes: ok, error (clean one-line error), limit (file refused: …), crash (signal or
panic), timeout.
"""
import os, resource, shutil, subprocess, sys, time

binary, data, results = sys.argv[1], sys.argv[2], sys.argv[3]
limit = None
timeout = 300
args = sys.argv[4:]
for i, a in enumerate(args):
    if a == "--limit":
        limit = int(args[i + 1])
    if a == "--timeout":
        timeout = int(args[i + 1])

EXTS = {"xlsx", "xlsm", "xls", "xlsb", "ods", "csv", "tsv"}
files = []
for root, dirs, fs in os.walk(data):
    dirs.sort()
    for f in sorted(fs):
        if f.rsplit(".", 1)[-1].lower() in EXTS:
            files.append(os.path.join(root, f))
if limit:
    files = files[:limit]

done = set()
if os.path.exists(results):
    with open(results) as fh:
        done = {line.split("\t", 1)[0] for line in fh}

out_dir = os.path.join(os.path.dirname(os.path.abspath(results)), "sweep_out")


def cap():
    # heap (data segment + mmap'd anonymous memory since Linux 4.7), under the unit's 2500M cap
    lim = int(2.3 * (1 << 30))
    resource.setrlimit(resource.RLIMIT_DATA, (lim, lim))


with open(results, "a") as res:
    t_all = time.time()
    n = 0
    for f in files:
        if f in done:
            continue
        shutil.rmtree(out_dir, ignore_errors=True)
        t = time.time()
        try:
            p = subprocess.run([binary, "normalize", f, "-o", out_dir], capture_output=True, timeout=timeout,
                               preexec_fn=cap, cwd=os.path.dirname(os.path.abspath(results)))
            rc = p.returncode
            err = p.stderr.decode("utf-8", "replace").strip().splitlines()
            last = next((l for l in err if l.strip() and not l.startswith("usage:")), "") if err else ""
            if rc == 0:
                outcome = "ok"
            elif rc < 0 or "panicked" in p.stderr.decode("utf-8", "replace") or "memory allocation" in last:
                outcome = "crash"
            elif "file refused" in last:
                outcome = "limit"
            else:
                outcome = "error"
        except subprocess.TimeoutExpired:
            rc, last, outcome = -999, "", "timeout"
        dt = time.time() - t
        last = last.replace("\t", " ")[:300]
        res.write(f"{f}\t{outcome}\t{dt:.2f}\t{rc}\t{last}\n")
        res.flush()
        n += 1
        if n % 500 == 0:
            print(f"{n} files, {time.time() - t_all:.0f} s", flush=True)
shutil.rmtree(out_dir, ignore_errors=True)
print("done", flush=True)
