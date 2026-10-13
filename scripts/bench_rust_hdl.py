#!/usr/bin/env python3
"""Compare `vhone lint` with rust_hdl's `vhdl_lang` on VHDL projects.

For each project directory, vhone analyzes it with zero configuration.
rust_hdl gets a `vhdl_ls.toml` generated from vhone's inferred libraries
(`vhone project --vhdl-ls`), since it cannot infer them, and its own
standard libraries. Both run their full analysis plus all lint checks.

Reports the median wall time, peak memory, and the number of errors each
tool reports inside the project (on valid code, every error is a false
error).

    scripts/bench_rust_hdl.py --vhone target/release/vhone \\
        --vhdl-lang path/to/vhdl_lang --libraries path/to/vhdl_libraries \\
        [--runs 5] PROJECT...
"""

import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import tempfile
import time


def run(cmd):
    """Runs `cmd`; returns (seconds, peak RSS in MiB, stdout)."""
    start = time.perf_counter()
    with tempfile.TemporaryFile() as out:
        proc = subprocess.Popen(cmd, stdout=out, stderr=subprocess.DEVNULL)
        _, _, usage = os.wait4(proc.pid, 0)
        elapsed = time.perf_counter() - start
        out.seek(0)
        text = out.read().decode("utf-8", "replace")
    rss = usage.ru_maxrss / 1024
    if sys.platform == "darwin":
        rss /= 1024
    return elapsed, rss, text


def measure(cmd, runs):
    times, peaks, text = [], [], ""
    for _ in range(runs):
        elapsed, rss, text = run(cmd)
        times.append(elapsed)
        peaks.append(rss)
    return statistics.median(times), max(peaks), text


def vhdl_ls_config(vhone, project, path):
    out = subprocess.run(
        [vhone, "project", "--vhdl-ls", project], check=True, capture_output=True, text=True
    ).stdout
    root = os.path.abspath(project)
    # Make the file lists absolute so the config can live anywhere.
    out = re.sub(r'"([^"]+\.vhdl?)"', lambda m: json.dumps(os.path.join(root, m.group(1))), out)
    with open(path, "w") as f:
        f.write(out)


def rust_hdl_errors(text, root):
    errors = 0
    lines = text.splitlines()
    for i, line in enumerate(lines):
        if not line.startswith("error"):
            continue
        for follow in lines[i + 1 : i + 4]:
            m = re.match(r"\s*--> (.+?):\d+", follow)
            if m:
                if os.path.abspath(m.group(1)).startswith(root):
                    errors += 1
                break
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--vhone", required=True)
    parser.add_argument("--vhdl-lang", required=True)
    parser.add_argument("--libraries", required=True, help="directory with rust_hdl's vhdl_ls.toml")
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("projects", nargs="+")
    args = parser.parse_args()

    rows = []
    with tempfile.TemporaryDirectory() as tmp:
        for project in args.projects:
            root = os.path.abspath(project)
            name = os.path.basename(root.rstrip("/"))
            config = os.path.join(tmp, f"{name}.toml")
            vhdl_ls_config(args.vhone, project, config)
            files = sum(
                1
                for d, _, fs in os.walk(root)
                for f in fs
                if f.lower().endswith((".vhd", ".vhdl"))
            )
            v_time, v_mem, v_out = measure(
                [args.vhone, "lint", "--output-format", "json", root], args.runs
            )
            v_errors = sum(1 for d in json.loads(v_out) if d["level"] == "error")
            r_time, r_mem, r_out = measure(
                [args.vhdl_lang, "-l", args.libraries, "-c", config], args.runs
            )
            r_errors = rust_hdl_errors(r_out, root)
            rows.append((name, files, v_time, r_time, v_mem, r_mem, v_errors, r_errors))
            print(f"{name}: done", file=sys.stderr)

    print("| Project | Files | vhone time | rust_hdl time | vhone memory | rust_hdl memory | vhone errors | rust_hdl errors |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|")
    for name, files, vt, rt, vm, rm, ve, re_ in rows:
        print(
            f"| {name} | {files} | {vt * 1000:.0f} ms | {rt * 1000:.0f} ms "
            f"| {vm:.0f} MiB | {rm:.0f} MiB | {ve} | {re_} |"
        )


if __name__ == "__main__":
    main()
