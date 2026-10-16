#!/usr/bin/env python3
"""Measures what `vfl lsp` answers while it analyzes.

While the server re-analyzes UVVM after an edit of `types_pkg` (most of the
project depends on it), times: formatting another file, a hover in another
file, a hover cancelled 5 ms after it is sent, and then a hover in the edited
file ("sync", which waits for whatever is left of the analysis).
Usage: scripts/bench_busy.py target/release/vfl UVVM_DIR [ROUNDS]
"""

import pathlib
import statistics
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from bench_vhdl_ls import Server, uri  # noqa: E402

vfl, project = sys.argv[1], pathlib.Path(sys.argv[2]).resolve()
rounds = int(sys.argv[3]) if len(sys.argv) > 3 else 10
server = Server([vfl, "lsp"])
server.request("initialize", {"processId": None, "rootUri": uri(project), "capabilities": {}})
server.notify("initialized", {})
server.request("workspace/symbol", {"query": "zzzz_none"})

pkg = project / "uvvm_util/src/types_pkg.vhd"
other = project / "uvvm_util/src/string_methods_pkg.vhd"
texts = {}
for path in (pkg, other):
    texts[path] = path.read_text(errors="replace")
    server.notify(
        "textDocument/didOpen",
        {"textDocument": {"uri": uri(path), "languageId": "vhdl", "version": 1, "text": texts[path]}},
    )
server.request("textDocument/hover", {"textDocument": {"uri": uri(pkg)}, "position": {"line": 0, "character": 0}})

version = 1


def edit(round):
    global version
    version += 1
    text = texts[pkg]
    new = text.replace("integer", "natural" if round % 2 == 0 else "positive", 1)
    server.notify(
        "textDocument/didChange",
        {"textDocument": {"uri": uri(pkg), "version": version}, "contentChanges": [{"text": new}]},
    )


def send(method, params):
    server.next_id += 1
    server._send({"jsonrpc": "2.0", "id": server.next_id, "method": method, "params": params})
    return server.next_id


def wait(id):
    while True:
        response = server.responses.get(timeout=600)
        if response.get("id") == id:
            return response


def timed(method, params, delay=0.005, cancel=False):
    begin = time.perf_counter()
    id = send(method, params)
    if cancel:
        time.sleep(delay)
        server.notify("$/cancelRequest", {"id": id})
    response = wait(id)
    return time.perf_counter() - begin, response


results = {"format": [], "hover": [], "cancelled": [], "sync": []}
sync = lambda: timed("textDocument/hover",
                     {"textDocument": {"uri": uri(pkg)}, "position": {"line": 0, "character": 0}})
n = 0
for round in range(rounds):
    for phase in ("format", "hover", "cancelled"):
        n += 1
        edit(n)
        time.sleep(0.02)  # the server starts analyzing
        if phase == "format":
            t, _ = timed("textDocument/formatting",
                         {"textDocument": {"uri": uri(other)}, "options": {"tabSize": 2, "insertSpaces": True}})
        else:
            t, r = timed("textDocument/hover",
                         {"textDocument": {"uri": uri(other)}, "position": {"line": 30, "character": 10}},
                         delay=0.005, cancel=phase == "cancelled")
            if phase == "cancelled":
                assert "error" in r, r
        results[phase].append(t)
        t, _ = sync()
        results["sync"].append(t)
for k, v in results.items():
    v.sort()
    print(f"{k}: median {statistics.median(v)*1000:.1f} ms, max {v[-1]*1000:.1f} ms")
server.close()
