#!/usr/bin/env python3
"""Measures the memory of `vfl lsp` on a project, as an editor uses it.

Starts the server over stdio and, on the project:

- opens it (`initialize`, then waits for the answer to a first request);
- opens 8 files spread over it, asking for their symbols, folding ranges and
  semantic tokens, as VS Code does;
- edits each 3 times (a comment appended at the end), each edit followed by a
  hover, and times the edit to the hover's answer.

It prints the resident memory after opening the project and at the end
(steady), the peak resident memory, and the median and slowest edit.
Usage: scripts/bench_memory.py --vfl target/release/vfl [--runs N] PROJECT...
"""

import argparse
import pathlib
import statistics
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from bench_vhdl_ls import Server, uri  # noqa: E402

SUFFIXES = {".vhd", ".vhdl", ".vho", ".vht", ".v", ".sv", ".vh", ".svh"}


def memory(server):
    """The current and peak resident memory in MiB, from /proc (Linux)."""
    values = {}
    with open(f"/proc/{server.proc.pid}/status") as f:
        for line in f:
            key, _, rest = line.partition(":")
            if key in ("VmRSS", "VmHWM"):
                values[key] = int(rest.split()[0]) / 1024
    return values["VmRSS"], values["VmHWM"]


def run(vfl, project):
    project = pathlib.Path(project).resolve()
    server = Server([vfl, "lsp"])
    start = time.perf_counter()
    server.request(
        "initialize",
        {"processId": None, "rootUri": uri(project), "capabilities": {}},
    )
    server.notify("initialized", {})
    server.request("workspace/symbol", {"query": "zzzz_none"})
    open_time = time.perf_counter() - start
    open_rss, _ = memory(server)

    files = sorted(
        p for p in project.rglob("*") if p.suffix.lower() in SUFFIXES and p.is_file()
    )
    step = max(len(files) // 8, 1)
    files = files[::step][:8]
    texts = {}
    for path in files:
        text = path.read_text(errors="replace")
        texts[path] = text
        doc = {"uri": uri(path)}
        server.notify(
            "textDocument/didOpen",
            {"textDocument": {**doc, "languageId": "vhdl", "version": 1, "text": text}},
        )
        server.request("textDocument/documentSymbol", {"textDocument": doc})
        server.request("textDocument/foldingRange", {"textDocument": doc})
        server.request("textDocument/semanticTokens/full", {"textDocument": doc})

    edits = []
    for path in files:
        text = texts[path]
        doc = {"uri": uri(path)}
        for round in range(3):
            line = text.count("\n")
            end = len(text) - (text.rfind("\n") + 1)
            added = f"\n-- edit {round}\n"
            change = {
                "range": {
                    "start": {"line": line, "character": end},
                    "end": {"line": line, "character": end},
                },
                "text": added,
            }
            text += added
            begin = time.perf_counter()
            server.notify(
                "textDocument/didChange",
                {"textDocument": {**doc, "version": round + 2}, "contentChanges": [change]},
            )
            server.request(
                "textDocument/hover",
                {"textDocument": doc, "position": {"line": 0, "character": 0}},
            )
            edits.append(time.perf_counter() - begin)
    steady, peak = memory(server)
    server.close()
    edits.sort()
    return {
        "open": open_time,
        "open_rss": open_rss,
        "steady": steady,
        "peak": peak,
        "median": statistics.median(edits) if edits else float("nan"),
        "max": edits[-1] if edits else float("nan"),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--vfl", required=True)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("projects", nargs="+")
    args = parser.parse_args()
    print(
        "| Project | Open | RSS after open | RSS after edits | Peak RSS | Edit (median / max) |"
    )
    print("|---|---:|---:|---:|---:|---:|")
    for project in args.projects:
        results = [run(args.vfl, project) for _ in range(args.runs)]

        def median(key):
            return statistics.median(r[key] for r in results)

        print(
            f"| {pathlib.Path(project).name} | {median('open') * 1000:.0f} ms "
            f"| {median('open_rss'):.0f} MiB | {median('steady'):.0f} MiB "
            f"| {median('peak'):.0f} MiB "
            f"| {median('median') * 1000:.0f} / {median('max') * 1000:.0f} ms |",
            flush=True,
        )


if __name__ == "__main__":
    main()
