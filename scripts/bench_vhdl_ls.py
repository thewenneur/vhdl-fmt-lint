#!/usr/bin/env python3
"""Compare the `vhone lsp` and rust_hdl's `vhdl_ls` language servers on VHDL projects.

Both servers run over stdio, driven by the same LSP client, on the same
project. For each project and each server, a fresh server is started
`--runs` times; each run measures:

- open: from `initialize` to the answer to the first request after
  `initialized`, which both servers only send once they have analyzed the
  whole project and published its diagnostics;
- edits: for 8 files spread over the project, 3 edits each that append a
  comment, then 3 that change a type (the first `integer` to `natural` or
  `positive`), timed from `didChange` to the answer to a request that
  follows it (both servers analyze, and publish diagnostics, before they
  answer it);
- requests: hover, completion and go to definition on the first
  identifier of every 7th line of 16 files spread over the project, and
  how many of them get a non-empty answer;
- the peak resident memory of the server process (Linux only);
- the errors and warnings published for files of the project once it is
  open.

vhone runs with no configuration. vhdl_ls cannot infer libraries: it uses
the project's own `vhdl_ls.toml` when there is one, and otherwise one
generated from vhone's inference (`vhone project --vhdl-ls`), given through
`VHDL_LS_CONFIG`. Its standard libraries come from `--libraries`.

    scripts/bench_vhdl_ls.py --vhone target/release/vhone \\
        --vhdl-ls path/to/vhdl_ls --libraries path/to/vhdl_libraries \\
        [--runs 3] PROJECT...
"""

import argparse
import json
import os
import pathlib
import queue
import re
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import tomllib

KEYWORDS = set(
    """abs access after alias all and architecture array assert assume attribute begin
    block body buffer bus case component configuration constant context cover default
    disconnect downto else elsif end entity exit fairness file for force function generate
    generic group guarded if impure in inertial inout is label library linkage literal loop
    map mod nand new next nor not null of on open or others out package parameter port
    postponed private procedure process property protected pure range record register
    reject release rem report restrict return rol ror select sequence severity shared signal
    sla sll sra srl strong subtype then to transport type unaffected units until use
    variable view vmode vprop vunit wait when while with xnor xor""".split()
)


class Server:
    """A language server on stdio, with a client that answers its requests."""

    def __init__(self, cmd, env=None):
        self.proc = subprocess.Popen(
            cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env
        )
        self.next_id = 0
        self.responses = queue.Queue()
        self.diagnostics = {}
        self.lock = threading.Lock()
        threading.Thread(target=self._read, daemon=True).start()

    def _send(self, message):
        body = json.dumps(message).encode()
        with self.lock:
            self.proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
            self.proc.stdin.flush()

    def _read(self):
        out = self.proc.stdout
        while True:
            length = None
            while True:
                line = out.readline()
                if not line:
                    return
                line = line.strip()
                if not line:
                    break
                if line.lower().startswith(b"content-length:"):
                    length = int(line.split(b":")[1])
            message = json.loads(out.read(length))
            if "method" in message and "id" in message:
                # A request from the server (registrations, configuration).
                self._send({"jsonrpc": "2.0", "id": message["id"], "result": None})
            elif "method" in message:
                if message["method"] == "textDocument/publishDiagnostics":
                    p = message["params"]
                    self.diagnostics[p["uri"]] = p["diagnostics"]
            else:
                self.responses.put(message)

    def notify(self, method, params):
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def request(self, method, params):
        self.next_id += 1
        self._send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        while True:
            response = self.responses.get(timeout=600)
            if response.get("id") == self.next_id:
                return response.get("result")

    def peak_memory(self):
        """Peak resident memory in MiB, from /proc (Linux)."""
        try:
            with open(f"/proc/{self.proc.pid}/status") as f:
                for line in f:
                    if line.startswith("VmHWM:"):
                        return int(line.split()[1]) / 1024
        except OSError:
            pass
        return float("nan")

    def close(self):
        try:
            self.request("shutdown", None)
            self.notify("exit", None)
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()


def uri(path):
    return pathlib.Path(path).absolute().as_uri()


def position(text, index):
    """The LSP position (UTF-16) of a character index."""
    line = text.count("\n", 0, index)
    start = text.rfind("\n", 0, index) + 1
    return {"line": line, "character": len(text[start:index].encode("utf-16-le")) // 2}


def identifier_positions(text):
    """The middle of the first identifier of every 7th line, outside comments."""
    positions = []
    for number, line in enumerate(text.split("\n")):
        if number % 7:
            continue
        code = line.split("--", 1)[0]
        code = re.sub(r'"[^"]*"', lambda m: " " * len(m.group()), code)
        for m in re.finditer(r"(?<!['\w])[A-Za-z]\w*", code):
            if m.group().lower() not in KEYWORDS:
                middle = m.start() + len(m.group()) // 2
                character = len(line[:middle].encode("utf-16-le")) // 2
                positions.append({"line": number, "character": character})
                break
    return positions


def spread(items, count):
    step = max(len(items) // count, 1)
    return items[::step][:count]


def project_files(config):
    """The project's files listed in a vhdl_ls.toml, which both servers analyze."""
    with open(config, "rb") as f:
        data = tomllib.load(f)
    base = os.path.dirname(os.path.abspath(config))
    files = set()
    for library in data.get("libraries", {}).values():
        if library.get("is_third_party"):
            continue
        for pattern in library.get("files", []):
            pattern = os.path.expandvars(pattern)
            root = pathlib.Path(pattern if os.path.isabs(pattern) else os.path.join(base, pattern))
            anchor = pathlib.Path(root.anchor)
            for path in anchor.glob(str(root.relative_to(anchor))):
                if path.is_file():
                    files.add(str(path))
    return sorted(files)


def vhdl_ls_config(vhone, project, path):
    out = subprocess.run(
        [vhone, "project", "--vhdl-ls", project], check=True, capture_output=True, text=True
    ).stdout
    root = os.path.abspath(project)
    # Make the file lists absolute so the config can live anywhere.
    out = re.sub(r'"([^"]+\.vhdl?)"', lambda m: json.dumps(os.path.join(root, m.group(1))), out)
    with open(path, "w") as f:
        f.write(out)


EDITS = [
    ("comment", lambda t, r: f"{t}\n-- edit {r}\n"),
    ("type", lambda t, r: t.replace("integer", "natural" if r % 2 == 0 else "positive", 1)),
]
REQUESTS = ["textDocument/hover", "textDocument/completion", "textDocument/definition"]


def bench_run(cmd, env, root, files):
    """One run of a fresh server: returns a dict of measurements."""
    result = {"edits": {name: [] for name, _ in EDITS}, "requests": {m: [] for m in REQUESTS}}
    result["answered"] = {m: 0 for m in REQUESTS}
    server = Server(cmd, env)
    root_uri = uri(root)
    start = time.perf_counter()
    server.request(
        "initialize",
        {
            "processId": os.getpid(),
            "rootUri": root_uri,
            "rootPath": root,
            "workspaceFolders": [{"uri": root_uri, "name": os.path.basename(root)}],
            "capabilities": {"textDocument": {"hover": {"contentFormat": ["markdown", "plaintext"]}}},
        },
    )
    server.notify("initialized", {})
    first = files[0]
    server.request("textDocument/documentSymbol", {"textDocument": {"uri": uri(first)}})
    result["open"] = time.perf_counter() - start

    published = []
    for u, diagnostics in server.diagnostics.items():
        if not u.startswith(root_uri + "/"):
            continue
        for d in diagnostics:
            line = d["range"]["start"]["line"] + 1
            published.append((u[len(root_uri) + 1 :], line, d.get("severity", 1), d["message"]))
    result["diagnostics"] = sorted(published)
    result["errors"] = sum(1 for d in published if d[2] == 1)
    result["warnings"] = sum(1 for d in published if d[2] == 2)

    # Editor requests.
    for path in spread(files, 16):
        text = pathlib.Path(path).read_text(encoding="utf-8", errors="replace")
        for pos in identifier_positions(text):
            params = {"textDocument": {"uri": uri(path)}, "position": pos}
            for method in REQUESTS:
                t = time.perf_counter()
                answer = server.request(method, params)
                result["requests"][method].append(time.perf_counter() - t)
                if isinstance(answer, dict) and "items" in answer:
                    answer = answer["items"]
                if answer:
                    result["answered"][method] += 1

    # Edits.
    for path in spread(files, 8):
        text = pathlib.Path(path).read_text(encoding="utf-8", errors="replace")
        doc = {"uri": uri(path)}
        server.notify(
            "textDocument/didOpen",
            {"textDocument": {**doc, "languageId": "vhdl", "version": 0, "text": text}},
        )
        server.request("textDocument/documentSymbol", {"textDocument": doc})
        version = 0
        for name, edit in EDITS:
            for r in range(3):
                version += 1
                t = time.perf_counter()
                server.notify(
                    "textDocument/didChange",
                    {
                        "textDocument": {**doc, "version": version},
                        "contentChanges": [{"text": edit(text, r)}],
                    },
                )
                server.request("textDocument/documentSymbol", {"textDocument": doc})
                result["edits"][name].append(time.perf_counter() - t)
            version += 1
            server.notify(
                "textDocument/didChange",
                {"textDocument": {**doc, "version": version}, "contentChanges": [{"text": text}]},
            )
            server.request("textDocument/documentSymbol", {"textDocument": doc})
        server.notify("textDocument/didClose", {"textDocument": doc})

    result["memory"] = server.peak_memory()
    server.close()
    return result


def bench(cmd, env, root, files, runs):
    results = [bench_run(cmd, env, root, files) for _ in range(runs)]
    pooled = lambda key, sub: sorted(x for r in results for x in r[key][sub])  # noqa: E731
    return {
        "open": statistics.median(r["open"] for r in results),
        "memory": max(r["memory"] for r in results),
        "errors": results[-1]["errors"],
        "warnings": results[-1]["warnings"],
        "diagnostics": results[-1]["diagnostics"],
        "edits": {name: pooled("edits", name) for name, _ in EDITS},
        "requests": {m: pooled("requests", m) for m in REQUESTS},
        "answered": results[-1]["answered"],
        "asked": len(results[-1]["requests"][REQUESTS[0]]),
    }


def ms(seconds):
    value = seconds * 1000
    return f"{value:.0f}" if value >= 10 else f"{value:.1f}" if value >= 1 else f"{value:.2f}"


def median_max(times):
    return f"{ms(times[len(times) // 2])} / {ms(times[-1])} ms"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--vhone", required=True)
    parser.add_argument("--vhdl-ls", required=True)
    parser.add_argument("--libraries", required=True, help="directory with rust_hdl's vhdl_ls.toml")
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--json", help="also write the raw results to this file")
    parser.add_argument("projects", nargs="+")
    args = parser.parse_args()

    rows = []
    with tempfile.TemporaryDirectory() as tmp:
        for project in args.projects:
            root = os.path.abspath(project)
            name = os.path.basename(root.rstrip("/"))
            env = dict(os.environ)
            config = os.path.join(root, "vhdl_ls.toml")
            if not os.path.exists(config):
                config = os.path.join(tmp, f"{name}.toml")
                vhdl_ls_config(args.vhone, root, config)
                env["VHDL_LS_CONFIG"] = config
            files = project_files(config)
            vhone = bench([os.path.abspath(args.vhone), "lsp"], None, root, files, args.runs)
            print(f"{name}: vhone done", file=sys.stderr)
            ls = bench(
                [os.path.abspath(args.vhdl_ls), "--silent", "-l", os.path.abspath(args.libraries)],
                env,
                root,
                files,
                args.runs,
            )
            print(f"{name}: vhdl_ls done", file=sys.stderr)
            rows.append((name, len(files), vhone, ls))

    if args.json:
        with open(args.json, "w") as f:
            json.dump([{"project": n, "files": c, "vhone": v, "vhdl_ls": s} for n, c, v, s in rows], f)

    print("| Project | Files | Open (vhone / vhdl_ls) | Peak memory (vhone / vhdl_ls) | Errors (vhone / vhdl_ls) | Warnings (vhone / vhdl_ls) |")
    print("|---|---:|---:|---:|---:|---:|")
    for name, files, v, s in rows:
        print(
            f"| {name} | {files} | {ms(v['open'])} / {ms(s['open'])} ms "
            f"| {v['memory']:.0f} / {s['memory']:.0f} MiB | {v['errors']} / {s['errors']} "
            f"| {v['warnings']} / {s['warnings']} |"
        )
    print()
    print("| Project | Server | Edit: comment (median / max) | Edit: type change (median / max) |")
    print("|---|---|---:|---:|")
    for name, _, v, s in rows:
        for server, r in (("vhone", v), ("vhdl_ls", s)):
            print(
                f"| {name} | {server} | {median_max(r['edits']['comment'])} "
                f"| {median_max(r['edits']['type'])} |"
            )
    print()
    print("| Project | Server | Hover (median / max, answered) | Completion | Definition |")
    print("|---|---|---:|---:|---:|")
    for name, _, v, s in rows:
        for server, r in (("vhone", v), ("vhdl_ls", s)):
            cells = [
                f"{median_max(r['requests'][m])}, {r['answered'][m]}/{r['asked']}" for m in REQUESTS
            ]
            print(f"| {name} | {server} | " + " | ".join(cells) + " |")


if __name__ == "__main__":
    main()
