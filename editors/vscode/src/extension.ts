// VS Code client for the vfl language server (`vfl lsp`).
//
// The server does the work: diagnostics, quick fixes, formatting,
// navigation, rename, completion and code generation. This file finds the
// `vfl` binary, starts it over stdio and restarts it when its settings
// change, asks it for generated code, and drops format on type unless
// `vfl.format.onType` is set. Documentation and diagrams are generated on
// demand by running `vfl doc`, never on the server's hot path.

import { execFile } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";
import { OPEN_PAGE_COMMAND, pageTitle, webviewHtml } from "./docview";

const INSTALL_URL = "https://github.com/thewenneur/vhdl-fmt-lint#installation";

let client: LanguageClient | undefined;
let output: vscode.LogOutputChannel;
let extensionContext: vscode.ExtensionContext;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  extensionContext = context;
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
    vscode.commands.registerCommand("vfl.generateDocs", generateDocs),
    vscode.commands.registerCommand("vfl.showDiagrams", showDiagrams),
    // The target of the links between pages in the diagrams view.
    vscode.commands.registerCommand(OPEN_PAGE_COMMAND, openDocPage),
    vscode.workspace.onDidSaveTextDocument((doc) => {
      if (diagrams && doc.languageId === "vhdl" && doc.uri.scheme === "file") {
        void refreshDiagrams();
      }
    }),
    vscode.window.onDidChangeActiveColorTheme(() => {
      if (diagrams) {
        renderDiagrams(diagrams);
      }
    }),
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
    void reportMissingServer();
    return;
  }

  // No `transport`: with one, the client appends `--stdio`, which `vfl lsp`
  // rejects. Without, it speaks over stdio all the same.
  const run = {
    command,
    args: ["lsp"],
    options: { env: serverEnv() },
  };
  const serverOptions: ServerOptions = { run, debug: run };
  const clientOptions: LanguageClientOptions = {
    // The server reads files from disk by path, so only real files.
    documentSelector: ["vhdl", "verilog", "systemverilog"].map((language) => ({
      scheme: "file",
      language,
    })),
    outputChannel: output,
    middleware: {
      // `editor.formatOnType` is on for VHDL and Verilog by default; this
      // setting decides.
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

async function reportMissingServer(): Promise<void> {
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
}

/** The environment of `vfl`: the extension host's, plus `vfl.server.extraEnv`. */
function serverEnv(): NodeJS.ProcessEnv {
  const extraEnv = vscode.workspace
    .getConfiguration("vfl")
    .get<Record<string, string>>("server.extraEnv", {});
  return { ...process.env, ...extraEnv };
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

// Documentation and diagrams, by running `vfl doc`.

/** Runs `vfl` with `args` from `cwd`; resolves with its standard output. */
async function runVfl(args: string[], cwd: string): Promise<string | undefined> {
  const command = findServer();
  if (!command) {
    void reportMissingServer();
    return undefined;
  }
  output.info(`Running ${command} ${args.join(" ")}`);
  return new Promise((resolve) => {
    execFile(
      command,
      args,
      { cwd, env: serverEnv(), maxBuffer: 16 * 1024 * 1024 },
      (err, stdout, stderr) => {
        if (stderr.trim()) {
          output.info(stderr.trim());
        }
        if (err) {
          const message = stderr.trim().split(/\r?\n/).pop() || err.message;
          void vscode.window.showErrorMessage(`vfl: ${message}`);
          resolve(undefined);
        } else {
          resolve(stdout);
        }
      },
    );
  });
}

/** The last line `vfl doc` prints: the page to open. */
function printedPath(stdout: string): string | undefined {
  return stdout.trim().split(/\r?\n/).pop()?.trim() || undefined;
}

/** The file or folder to document: the argument, else the active file's folder, else the workspace. */
function docTarget(uri?: vscode.Uri): string | undefined {
  if (uri?.scheme === "file") {
    return uri.fsPath;
  }
  const active = vscode.window.activeTextEditor?.document.uri;
  if (active?.scheme === "file") {
    return vscode.workspace.getWorkspaceFolder(active)?.uri.fsPath ?? active.fsPath;
  }
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

function directoryOf(target: string): string {
  try {
    return fs.statSync(target).isDirectory() ? target : path.dirname(target);
  } catch {
    return path.dirname(target);
  }
}

/**
 * Writes the documentation of the project around `uri` (or the active file,
 * or the workspace folder) where `[doc]` in `vfl.toml` says, then offers to
 * open its index. Returns the index.
 */
async function generateDocs(uri?: vscode.Uri): Promise<string | undefined> {
  const target = docTarget(uri instanceof vscode.Uri ? uri : undefined);
  if (!target) {
    void vscode.window.showErrorMessage("vfl: open a folder or a VHDL file to document.");
    return undefined;
  }
  const stdout = await vscode.window.withProgress(
    { location: vscode.ProgressLocation.Notification, title: "vfl: generating documentation" },
    () => runVfl(["doc", target], directoryOf(target)),
  );
  const index = stdout && printedPath(stdout);
  if (!index) {
    return undefined;
  }
  void vscode.window
    .showInformationMessage(`vfl: documentation written to ${path.dirname(index)}`, "Open")
    .then((choice) => (choice === "Open" ? openIndex(index) : undefined));
  return index;
}

/** Opens an HTML index in the browser, a Markdown or AsciiDoc one in the editor. */
async function openIndex(index: string): Promise<void> {
  const uri = vscode.Uri.file(index);
  if (index.endsWith(".html")) {
    await vscode.env.openExternal(uri);
  } else {
    await vscode.window.showTextDocument(uri);
  }
}

/** The diagrams webview; there is at most one. */
interface Diagrams {
  panel: vscode.WebviewPanel;
  /** The file whose project is drawn. */
  source: string;
  /** The output directory of `vfl doc`. */
  dir: string;
  /** The page shown, as a file name in `dir`. */
  page: string;
}

let diagrams: Diagrams | undefined;
let refreshing: Promise<void> | undefined;
let refreshAgain = false;

/** Writes the HTML documentation of `source`'s project to `dir`; returns the page of `source`. */
async function drawDiagrams(source: string, dir: string): Promise<string | undefined> {
  const args = ["doc", source, "--format", "html", "-o", dir, "--page-for", source];
  const stdout = await vscode.window.withProgress(
    { location: vscode.ProgressLocation.Window, title: "vfl: drawing diagrams" },
    () => runVfl(args, path.dirname(source)),
  );
  return stdout ? printedPath(stdout) : undefined;
}

/**
 * Shows the documentation page of the entity (else package) of `uri` or of
 * the active file, with its diagrams, in a webview beside the editor.
 * Returns the page shown.
 */
async function showDiagrams(uri?: vscode.Uri): Promise<string | undefined> {
  const source = uri instanceof vscode.Uri ? uri : vscode.window.activeTextEditor?.document.uri;
  if (source?.scheme !== "file") {
    void vscode.window.showErrorMessage("vfl: open a VHDL file to show its diagrams.");
    return undefined;
  }
  const storage = extensionContext.storageUri ?? extensionContext.globalStorageUri;
  const page = await drawDiagrams(source.fsPath, path.join(storage.fsPath, "diagrams"));
  if (!page) {
    return undefined;
  }
  const state = { source: source.fsPath, dir: path.dirname(page), page: path.basename(page) };
  if (diagrams) {
    Object.assign(diagrams, state);
    diagrams.panel.reveal(vscode.ViewColumn.Beside, true);
  } else {
    const panel = vscode.window.createWebviewPanel(
      "vfl.diagrams",
      pageTitle(state.page),
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
      {
        enableScripts: false,
        enableCommandUris: [OPEN_PAGE_COMMAND],
        enableFindWidget: true,
        localResourceRoots: [],
      },
    );
    const view = { panel, ...state };
    diagrams = view;
    panel.onDidDispose(() => {
      if (diagrams === view) {
        diagrams = undefined;
      }
    });
  }
  renderDiagrams(diagrams);
  return page;
}

/** Shows another page of the documentation in the diagrams view. */
function openDocPage(page: unknown): void {
  if (!diagrams || typeof page !== "string" || path.basename(page) !== page) {
    return;
  }
  diagrams.page = page;
  renderDiagrams(diagrams);
}

function renderDiagrams(view: Diagrams): void {
  let html: string;
  try {
    html = fs.readFileSync(path.join(view.dir, view.page), "utf8");
  } catch (err) {
    output.error(`Reading ${view.page}: ${err}`);
    return;
  }
  const kind = vscode.window.activeColorTheme.kind;
  const dark = kind === vscode.ColorThemeKind.Dark || kind === vscode.ColorThemeKind.HighContrast;
  view.panel.title = pageTitle(view.page);
  view.panel.webview.html = webviewHtml(html, view.page, dark);
}

/** Draws the diagrams again after a save, one run at a time. */
async function refreshDiagrams(): Promise<void> {
  if (refreshing) {
    refreshAgain = true;
    return refreshing;
  }
  refreshing = (async () => {
    do {
      refreshAgain = false;
      const view = diagrams;
      if (!view) {
        break;
      }
      const page = await drawDiagrams(view.source, view.dir);
      if (page && diagrams === view) {
        if (!isFile(path.join(view.dir, view.page))) {
          view.page = path.basename(page);
        }
        renderDiagrams(view);
      }
    } while (refreshAgain);
  })();
  try {
    await refreshing;
  } finally {
    refreshing = undefined;
  }
}
