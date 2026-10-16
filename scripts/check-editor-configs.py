#!/usr/bin/env python3
"""Checks the JSON and TOML files under editors/ parse, and that the setups
for LSP clients that no editor test runs (Kate, Sublime Text, coc.nvim,
JetBrains, Helix) start `vfl lsp`.

The Neovim, Vim, Emacs and Helix setups also run in their editors
(editors/test/run.sh); the VS Code and Zed extensions have checks of their
own. Usage: scripts/check-editor-configs.py (Python 3.11 or later).
"""

import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EDITORS = ROOT / "editors"
SKIP = {"node_modules", "target", "grammars", "syntaxes", "fixture"}

# Files read by programs that accept comments and trailing commas.
JSONC = {
    "sublime/LSP.sublime-settings",
    "vscode/language-configuration.json",
    "vscode/language-configuration-verilog.json",
    "vscode/tsconfig.json",
    "zed/keymap.json",
    "zed/tasks.json",
}


def strip_jsonc(text: str) -> str:
    """Removes comments and trailing commas, leaving strings alone."""
    out = []
    i = 0
    while i < len(text):
        c = text[i]
        if c == '"':
            end = i + 1
            while text[end] != '"':
                end += 2 if text[end] == "\\" else 1
            out.append(text[i : end + 1])
            i = end + 1
        elif text.startswith("//", i):
            i = text.find("\n", i)
            i = len(text) if i < 0 else i
        elif text.startswith("/*", i):
            i = text.index("*/", i) + 2
        else:
            out.append(c)
            i += 1
    return re.sub(r",(\s*[}\]])", r"\1", "".join(out))


def load(path: Path):
    rel = path.relative_to(EDITORS).as_posix()
    text = path.read_text(encoding="utf-8")
    if path.suffix == ".toml":
        return tomllib.loads(text)
    return json.loads(strip_jsonc(text) if rel in JSONC else text)


def starts_vfl_lsp(command) -> bool:
    if isinstance(command, str):
        command = command.split()
    return list(command)[:2] == ["vfl", "lsp"]


# What each LSP client setup must hold.
CLIENTS = {
    "kate/lspclient.json": lambda c: all(
        starts_vfl_lsp(s["command"]) for s in c["servers"].values()
    ),
    "sublime/LSP.sublime-settings": lambda c: starts_vfl_lsp(c["clients"]["vfl"]["command"]),
    "vim/coc-settings.json": lambda c: starts_vfl_lsp(
        [c["languageserver"]["vfl"]["command"], *c["languageserver"]["vfl"]["args"]]
    ),
    "jetbrains/vfl/template.json": lambda c: starts_vfl_lsp(c["programArgs"]["default"]),
    "helix/languages.toml": lambda c: starts_vfl_lsp(
        [c["language-server"]["vfl"]["command"], *c["language-server"]["vfl"]["args"]]
    ),
}


def main() -> int:
    failures = []
    checked = 0
    for path in sorted(EDITORS.rglob("*")):
        dirs = path.relative_to(EDITORS).parts[:-1]
        # Dependencies, build output, and hidden directories such as the
        # VS Code the end-to-end tests download.
        if SKIP.intersection(dirs) or any(d.startswith(".") for d in dirs) or not path.is_file():
            continue
        if path.suffix not in (".json", ".toml", ".sublime-settings") or path.name.endswith(
            "-lock.json"
        ):
            continue
        rel = path.relative_to(EDITORS).as_posix()
        checked += 1
        try:
            config = load(path)
        except (ValueError, tomllib.TOMLDecodeError) as err:
            failures.append(f"editors/{rel}: {err}")
            continue
        check = CLIENTS.get(rel)
        try:
            if check and not check(config):
                failures.append(f"editors/{rel}: does not start `vfl lsp`")
        except (KeyError, TypeError) as err:
            failures.append(f"editors/{rel}: no server command ({err!r})")
    missing = [rel for rel in CLIENTS if not (EDITORS / rel).is_file()]
    failures += [f"editors/{rel}: missing" for rel in missing]
    for failure in failures:
        print(f"FAIL: {failure}", file=sys.stderr)
    if not failures:
        print(f"ok: {checked} editor config files parse")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
