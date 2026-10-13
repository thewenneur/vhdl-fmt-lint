// VS Code client for the vfl language server (`vfl lsp`).
//
// The server does the work: diagnostics, quick fixes, formatting,
// navigation, rename and completion. This file finds the `vfl` binary,
// starts it over stdio and restarts it when its settings change.

import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

const INSTALL_URL = "https://github.com/thewenneur/vhdl-fmt-lint#installation";

let client: LanguageClient | undefined;
let output: vscode.LogOutputChannel;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  output = vscode.window.createOutputChannel("vfl", { log: true });
  context.subscriptions.push(
    output,
    vscode.commands.registerCommand("vfl.restartServer", restart),
    vscode.commands.registerCommand("vfl.showOutput", () =>
      (client?.outputChannel ?? output).show(true),
    ),
    vscode.commands.registerCommand("vfl.fixAll", () =>
      vscode.commands.executeCommand("editor.action.codeAction", {
        kind: "source.fixAll.vfl",
        apply: "first",
      }),
    ),
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("vfl.server")) {
        void restart();
      }
    }),
  );
  await start();
}

export async function deactivate(): Promise<void> {
  await stop();
}

async function start(): Promise<void> {
  const command = findServer();
  if (!command) {
    const choice = await vscode.window.showErrorMessage(
      "vfl: could not find the `vfl` executable. Install it, or set `vfl.server.path`.",
      "Open Settings",
      "How to Install",
    );
    if (choice === "Open Settings") {
      void vscode.commands.executeCommand("workbench.action.openSettings", "vfl.server.path");
    } else if (choice === "How to Install") {
      void vscode.env.openExternal(vscode.Uri.parse(INSTALL_URL));
    }
    return;
  }

  const config = vscode.workspace.getConfiguration("vfl");
  const extraEnv = config.get<Record<string, string>>("server.extraEnv", {});
  // No `transport`: with one, the client appends `--stdio`, which `vfl lsp`
  // rejects. Without, it speaks over stdio all the same.
  const run = {
    command,
    args: ["lsp"],
    options: { env: { ...process.env, ...extraEnv } },
  };
  const serverOptions: ServerOptions = { run, debug: run };
  const clientOptions: LanguageClientOptions = {
    // The server reads files from disk by path, so only real files.
    documentSelector: [{ scheme: "file", language: "vhdl" }],
    outputChannel: output,
  };

  output.info(`Starting ${command} lsp`);
  client = new LanguageClient("vfl", "vfl Language Server", serverOptions, clientOptions);
  try {
    await client.start();
  } catch (err) {
    client = undefined;
    void vscode.window.showErrorMessage(`vfl: the language server failed to start: ${err}`);
  }
}

async function stop(): Promise<void> {
  const running = client;
  client = undefined;
  if (running?.isRunning()) {
    await running.stop();
  }
}

async function restart(): Promise<void> {
  await stop();
  await start();
}

/** The `vfl` executable to run: the setting, else `PATH`, else `~/.cargo/bin`. */
function findServer(): string | undefined {
  const exe = process.platform === "win32" ? "vfl.exe" : "vfl";
  const setting = vscode.workspace.getConfiguration("vfl").get<string>("server.path", "").trim();
  if (setting) {
    const resolved = expand(setting);
    if (isFile(resolved)) {
      return resolved;
    }
    output.info(`vfl.server.path points to ${resolved}, which is not a file`);
    return undefined;
  }
  const dirs = (process.env.PATH ?? "").split(path.delimiter).filter(Boolean);
  dirs.push(path.join(os.homedir(), ".cargo", "bin"));
  return dirs.map((dir) => path.join(dir, exe)).find(isFile);
}

/** Expands `~` and `${workspaceFolder}`; a relative path is taken from the workspace folder. */
function expand(value: string): string {
  const folder = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  let result = value.replace(/\$\{workspaceFolder\}/g, folder ?? "");
  if (result === "~" || result.startsWith("~/") || result.startsWith("~\\")) {
    result = path.join(os.homedir(), result.slice(1));
  }
  if (!path.isAbsolute(result) && folder) {
    result = path.join(folder, result);
  }
  return result;
}

function isFile(file: string): boolean {
  try {
    return fs.statSync(file).isFile();
  } catch {
    return false;
  }
}
