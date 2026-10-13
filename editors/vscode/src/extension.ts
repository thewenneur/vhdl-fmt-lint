// VS Code client for the vfl language server (`vfl lsp`).
//
// The server does the work: diagnostics, quick fixes, formatting,
// navigation, rename, completion and code generation. This file finds the
// `vfl` binary, starts it over stdio and restarts it when its settings
// change, asks it for generated code, and drops format on type unless
// `vfl.format.onType` is set.

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
    vscode.commands.registerCommand("vfl.insertInstance", (entity?: string) =>
      insertGenerated("instance", entity),
    ),
    vscode.commands.registerCommand("vfl.insertComponentInstance", (entity?: string) =>
      insertGenerated("component-instance", entity),
    ),
    vscode.commands.registerCommand("vfl.insertComponent", (entity?: string) =>
      insertGenerated("component", entity),
    ),
    vscode.commands.registerCommand("vfl.generateTestbench", generateTestbench),
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
    middleware: {
      // `editor.formatOnType` is on for VHDL by default; this setting decides.
      provideOnTypeFormattingEdits: (document, position, ch, options, token, next) =>
        vscode.workspace.getConfiguration("vfl", document).get<boolean>("format.onType", false)
          ? next(document, position, ch, options, token)
          : [],
    },
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

// Code generation.

type GenerateKind = "testbench" | "instance" | "component" | "component-instance";

interface EntityRef {
  library: string;
  name: string;
  path: string;
}

interface Generated {
  text: string;
  /** For a testbench, the file it is meant for. */
  path?: string;
}

/** Asks which entity to generate from, those of the active file first. */
async function pickEntity(placeHolder: string): Promise<string | undefined> {
  if (!client?.isRunning()) {
    void vscode.window.showErrorMessage("vfl: the language server is not running.");
    return undefined;
  }
  const entities = await client.sendRequest<EntityRef[]>("vfl/entities", {});
  if (entities.length === 0) {
    void vscode.window.showInformationMessage("vfl: no entity in this project.");
    return undefined;
  }
  const active = vscode.window.activeTextEditor?.document.uri.fsPath;
  const here = (e: EntityRef) =>
    active !== undefined && path.resolve(e.path) === path.resolve(active) ? 0 : 1;
  const items = [...entities]
    .sort((a, b) => here(a) - here(b))
    .map((e) => ({
      label: e.name,
      description: e.library,
      detail: vscode.workspace.asRelativePath(e.path),
      entity: `${e.library}.${e.name}`,
    }));
  if (items.length === 1) {
    return items[0].entity;
  }
  const picked = await vscode.window.showQuickPick(items, { placeHolder, matchOnDetail: true });
  return picked?.entity;
}

async function generate(kind: GenerateKind, entity: string): Promise<Generated | undefined> {
  if (!client?.isRunning()) {
    void vscode.window.showErrorMessage("vfl: the language server is not running.");
    return undefined;
  }
  try {
    return await client.sendRequest<Generated>("vfl/generate", { kind, entity });
  } catch (err) {
    void vscode.window.showErrorMessage(`vfl: ${err instanceof Error ? err.message : err}`);
    return undefined;
  }
}

/** Inserts an instantiation or component declaration at the cursor. */
async function insertGenerated(kind: GenerateKind, entity?: string): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    return;
  }
  entity ??= await pickEntity("Entity to insert");
  if (!entity) {
    return;
  }
  const generated = await generate(kind, entity);
  if (!generated) {
    return;
  }
  // A snippet, so that the lines after the first follow the cursor's indentation.
  const snippet = new vscode.SnippetString().appendText(generated.text.replace(/\n$/, ""));
  await editor.insertSnippet(snippet);
}

/** Writes a testbench next to the entity and opens it. */
async function generateTestbench(entity?: string): Promise<void> {
  entity ??= await pickEntity("Entity to test");
  if (!entity) {
    return;
  }
  const generated = await generate("testbench", entity);
  if (!generated?.path) {
    return;
  }
  const uri = vscode.Uri.file(generated.path);
  if (isFile(generated.path)) {
    const choice = await vscode.window.showWarningMessage(
      `${path.basename(generated.path)} exists. Replace it?`,
      { modal: true },
      "Replace",
    );
    if (choice !== "Replace") {
      return;
    }
  }
  await vscode.workspace.fs.writeFile(uri, Buffer.from(generated.text, "utf8"));
  await vscode.window.showTextDocument(uri);
}
