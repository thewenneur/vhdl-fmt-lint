// VS Code client for the vfl language server (`vfl lsp`).
//
// The server does the work: diagnostics, quick fixes, formatting,
// navigation, rename, completion and code generation. This file finds the
// `vfl` binary (or offers to download the release of its own version),
// starts it over stdio, one server per workspace folder in a multi-root
// workspace, and restarts it when its settings change. It passes it the
// lint preset and format style of projects without a `vfl.toml`, asks it
// for generated code and config files, shows its state and the `vfl.toml`
// in effect in the language status, and drops format on type unless
// `vfl.format.onType` is set. Documentation and diagrams are generated on
// demand by running `vfl doc`, never on the server's hot path.

import { execFile } from "node:child_process";
import * as crypto from "node:crypto";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  State,
} from "vscode-languageclient/node";
import { OPEN_PAGE_COMMAND, pageTitle, webviewHtml } from "./docview";
import { downloadServer, downloadedServer, release, releaseTarget } from "./download";

const INSTALL_URL = "https://github.com/thewenneur/vhdl-fmt-lint#installation";
const LANGUAGES = ["vhdl", "verilog", "systemverilog"];
/** The config file names, in the order the server prefers them. */
const CONFIG_FILES = ["vfl.toml", ".vfl.toml"];

/**
 * The language servers, by the workspace folder they serve (its URI), or
 * by "" for the one server of a workspace with one folder or none.
 */
const clients = new Map<string, LanguageClient>();
/** The servers being started, so that each starts once. */
const starting = new Map<string, Promise<void>>();
let output: vscode.LogOutputChannel;
let extensionContext: vscode.ExtensionContext;
let serverStatus: vscode.LanguageStatusItem;
let configStatus: vscode.LanguageStatusItem;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  extensionContext = context;
  output = vscode.window.createOutputChannel("vfl", { log: true });
  serverStatus = vscode.languages.createLanguageStatusItem("vfl.server", LANGUAGES);
  serverStatus.name = "vfl language server";
  configStatus = vscode.languages.createLanguageStatusItem("vfl.config", LANGUAGES);
  configStatus.name = "vfl config file";
  const configWatcher = vscode.workspace.createFileSystemWatcher("**/{vfl,.vfl}.toml");
  context.subscriptions.push(
    output,
    serverStatus,
    configStatus,
    configWatcher,
    configWatcher.onDidCreate(updateStatus),
    configWatcher.onDidDelete(updateStatus),
    vscode.window.onDidChangeActiveTextEditor(updateStatus),
    vscode.workspace.onDidOpenTextDocument((doc) => void ensureClient(doc)),
    vscode.workspace.onDidChangeWorkspaceFolders(() => void restart()),
    vscode.commands.registerCommand("vfl.restartServer", restart),
    vscode.commands.registerCommand("vfl.showOutput", () =>
      (activeClient()?.outputChannel ?? output).show(true),
    ),
    vscode.commands.registerCommand("vfl.fixAll", () =>
      vscode.commands.executeCommand("editor.action.codeAction", {
        kind: "source.fixAll.vfl",
        apply: "first",
      }),
    ),
    vscode.commands.registerCommand("vfl.toggleFormatOnType", toggleFormatOnType),
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
    vscode.commands.registerCommand("vfl.generateConfig", generateConfig),
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
      } else if (e.affectsConfiguration("vfl.lint.preset") || e.affectsConfiguration("vfl.format.style")) {
        for (const client of clients.values()) {
          if (client.isRunning()) {
            void client.sendNotification("workspace/didChangeConfiguration", {
              settings: editorSettings(),
            });
          }
        }
      }
    }),
  );
  await startAll();
}

export async function deactivate(): Promise<void> {
  await stopAll();
}

// Language servers.

/** Starts the servers the workspace needs now; others start with their first file. */
async function startAll(): Promise<void> {
  const folders = vscode.workspace.workspaceFolders ?? [];
  const keys = new Set<string>();
  if (folders.length <= 1) {
    // As with a single folder, a server from the start.
    keys.add("");
  } else {
    for (const folder of outermostFolders()) {
      if (hasConfig(folder.uri.fsPath)) {
        keys.add(folder.uri.toString());
      }
    }
  }
  for (const doc of vscode.workspace.textDocuments) {
    const key = clientKey(doc);
    if (key !== undefined) {
      keys.add(key);
    }
  }
  await Promise.all([...keys].map(startClient));
  updateStatus();
}

/** Starts the server of `doc`'s folder if it is a VHDL or Verilog file. */
async function ensureClient(doc: vscode.TextDocument): Promise<void> {
  const key = clientKey(doc);
  if (key !== undefined) {
    await startClient(key);
  }
}

/** Which server serves `doc`: see `clients`. */
function clientKey(doc: vscode.TextDocument): string | undefined {
  if (doc.uri.scheme !== "file" || !LANGUAGES.includes(doc.languageId)) {
    return undefined;
  }
  return folderKey(doc.uri);
}

function folderKey(uri: vscode.Uri): string | undefined {
  if ((vscode.workspace.workspaceFolders?.length ?? 0) <= 1) {
    return "";
  }
  const folder = vscode.workspace.getWorkspaceFolder(uri);
  return folder && outermost(folder).uri.toString();
}

/** The workspace folders that are not inside another one. */
function outermostFolders(): vscode.WorkspaceFolder[] {
  const folders = vscode.workspace.workspaceFolders ?? [];
  return folders.filter((f) => outermost(f) === f);
}

/** The outermost workspace folder holding `folder`: nested folders share its server. */
function outermost(folder: vscode.WorkspaceFolder): vscode.WorkspaceFolder {
  const inside = (outer: vscode.WorkspaceFolder) => {
    const rel = path.relative(outer.uri.fsPath, folder.uri.fsPath);
    return outer.uri.scheme === folder.uri.scheme && !rel.startsWith("..") && !path.isAbsolute(rel);
  };
  const outers = (vscode.workspace.workspaceFolders ?? []).filter(inside);
  return outers.sort((a, b) => a.uri.fsPath.length - b.uri.fsPath.length)[0] ?? folder;
}

function folderOf(key: string): vscode.WorkspaceFolder | undefined {
  if (key === "") {
    return vscode.workspace.workspaceFolders?.[0];
  }
  return vscode.workspace.workspaceFolders?.find((f) => f.uri.toString() === key);
}

function startClient(key: string): Promise<void> {
  if (clients.has(key)) {
    return Promise.resolve();
  }
  let pending = starting.get(key);
  if (!pending) {
    pending = createClient(key).finally(() => starting.delete(key));
    starting.set(key, pending);
  }
  return pending;
}

async function createClient(key: string): Promise<void> {
  const folder = folderOf(key);
  const command = await serverCommand(folder);
  if (!command || clients.has(key)) {
    updateStatus();
    return;
  }

  // No `transport`: with one, the client appends `--stdio`, which `vfl lsp`
  // rejects. Without, it speaks over stdio all the same.
  const run = {
    command,
    args: ["lsp"],
    options: { env: serverEnv(folder), cwd: folder?.uri.fsPath },
  };
  const serverOptions: ServerOptions = { run, debug: run };
  // The server reads files from disk by path, so only real files; in a
  // multi-root workspace, only those of its folder.
  const pattern = key === "" ? undefined : `${folder!.uri.fsPath.replace(/\\/g, "/")}/**/*`;
  const clientOptions: LanguageClientOptions = {
    documentSelector: LANGUAGES.map((language) => ({
      scheme: "file",
      language,
      ...(pattern ? { pattern } : {}),
    })),
    workspaceFolder: key === "" ? undefined : folder,
    outputChannel: output,
    initializationOptions: editorSettings,
    middleware: {
      // `editor.formatOnType` is on for VHDL and Verilog by default; this
      // setting decides.
      provideOnTypeFormattingEdits: (document, position, ch, options, token, next) =>
        vscode.workspace.getConfiguration("vfl", document).get<boolean>("format.onType", false)
          ? next(document, position, ch, options, token)
          : [],
    },
  };

  const name = key === "" ? "vfl Language Server" : `vfl Language Server (${folder!.name})`;
  output.info(`Starting ${command} lsp${key === "" ? "" : ` for ${folder!.uri.fsPath}`}`);
  const client = new LanguageClient("vfl", name, serverOptions, clientOptions);
  clients.set(key, client);
  client.onDidChangeState(updateStatus);
  updateStatus();
  try {
    await client.start();
  } catch (err) {
    if (clients.get(key) === client) {
      clients.delete(key);
    }
    void vscode.window.showErrorMessage(`vfl: the language server failed to start: ${err}`);
  }
  updateStatus();
}

/** The server of the file in front, else the only one, else the first running. */
function activeClient(): LanguageClient | undefined {
  const doc = vscode.window.activeTextEditor?.document;
  const key = doc?.uri.scheme === "file" ? folderKey(doc.uri) : undefined;
  if (key !== undefined && clients.get(key)?.isRunning()) {
    return clients.get(key);
  }
  const all = [...clients.values()];
  return all.length === 1 ? all[0] : all.find((c) => c.isRunning());
}

/** The running server for a request, or an error message and undefined. */
function runningClient(): LanguageClient | undefined {
  const client = activeClient();
  if (!client?.isRunning()) {
    void vscode.window.showErrorMessage("vfl: the language server is not running.");
    return undefined;
  }
  return client;
}

/**
 * The settings the server applies to projects without a `vfl.toml`, as
 * `vfl.toml` keys under `config`.
 */
function editorSettings(): { config: object } {
  const settings = vscode.workspace.getConfiguration("vfl");
  return {
    config: {
      lint: { preset: settings.get<string>("lint.preset", "recommended") },
      format: { style: settings.get<string>("format.style", "default") },
    },
  };
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
function serverEnv(folder?: vscode.WorkspaceFolder): NodeJS.ProcessEnv {
  const extraEnv = vscode.workspace
    .getConfiguration("vfl", folder)
    .get<Record<string, string>>("server.extraEnv", {});
  return { ...process.env, ...extraEnv };
}

async function stopAll(): Promise<void> {
  const running = [...clients.values()];
  clients.clear();
  await Promise.all(running.map((c) => (c.isRunning() ? c.stop() : undefined)));
  updateStatus();
}

async function restart(): Promise<void> {
  await stopAll();
  await startAll();
}

// The server binary.

/** The extension's version: the release whose binary it downloads. */
function extensionVersion(): string {
  return extensionContext.extension.packageJSON.version as string;
}

/** Where downloaded binaries go. */
function downloadDir(): string {
  return path.join(extensionContext.globalStorageUri.fsPath, "server");
}

/**
 * The `vfl` executable to run: the setting, else `PATH`, else
 * `~/.cargo/bin`, else the release binary downloaded earlier.
 */
function findServer(folder?: vscode.WorkspaceFolder): string | undefined {
  const exe = process.platform === "win32" ? "vfl.exe" : "vfl";
  const setting = vscode.workspace.getConfiguration("vfl", folder).get<string>("server.path", "").trim();
  if (setting) {
    const resolved = expand(setting, folder);
    if (isFile(resolved)) {
      return resolved;
    }
    output.info(`vfl.server.path points to ${resolved}, which is not a file`);
    return undefined;
  }
  const dirs = (process.env.PATH ?? "").split(path.delimiter).filter(Boolean);
  dirs.push(path.join(os.homedir(), ".cargo", "bin"));
  const found = dirs.map((dir) => path.join(dir, exe)).find(isFile);
  const target = releaseTarget(process.platform, process.arch);
  return found ?? (target && downloadedServer(downloadDir(), extensionVersion(), target));
}

/** The download offered in this session, so that it is offered once. */
let offer: Promise<string | undefined> | undefined;

/**
 * The `vfl` executable to run, after offering to download it when there is
 * none and `vfl.server.path` is not set. Undefined after saying why.
 */
async function serverCommand(folder?: vscode.WorkspaceFolder): Promise<string | undefined> {
  const found = findServer(folder);
  if (found) {
    return found;
  }
  const setting = vscode.workspace.getConfiguration("vfl", folder).get<string>("server.path", "").trim();
  const target = releaseTarget(process.platform, process.arch);
  if (setting || !target) {
    void reportMissingServer();
    return undefined;
  }
  offer ??= offerDownload(target);
  return offer;
}

async function offerDownload(target: string): Promise<string | undefined> {
  const version = extensionVersion();
  const choice = await vscode.window.showInformationMessage(
    `vfl: the \`vfl\` executable was not found. Download vfl ${version} for this platform from ` +
      "its GitHub release? It is checked against the release's SHA256SUMS and kept with the extension.",
    "Download",
    "Open Settings",
    "How to Install",
  );
  if (choice === "Open Settings") {
    void vscode.commands.executeCommand("workbench.action.openSettings", "vfl.server.path");
  } else if (choice === "How to Install") {
    void vscode.env.openExternal(vscode.Uri.parse(INSTALL_URL));
  }
  if (choice !== "Download") {
    return undefined;
  }
  const archive = release(version, target).archive;
  try {
    const binary = await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Notification, title: `vfl: downloading ${archive}` },
      () => downloadServer(downloadDir(), version, target),
    );
    output.info(`Downloaded ${binary}`);
    return binary;
  } catch (err) {
    output.error(`Downloading ${archive}: ${err}`);
    void vscode.window.showErrorMessage(
      `vfl: could not download ${archive}: ${err instanceof Error ? err.message : err}`,
    );
    // Offer it again next time, the network may be back.
    offer = undefined;
    return undefined;
  }
}

/**
 * Expands `~` and `${workspaceFolder}`; a relative path is taken from the
 * workspace folder: `folder`, else that of the file in front, else the first.
 */
function expand(value: string, folder?: vscode.WorkspaceFolder): string {
  const active = vscode.window.activeTextEditor?.document.uri;
  const dir = (
    folder ??
    (active && vscode.workspace.getWorkspaceFolder(active)) ??
    vscode.workspace.workspaceFolders?.[0]
  )?.uri.fsPath;
  let result = value.replace(/\$\{workspaceFolder\}/g, dir ?? "");
  if (result === "~" || result.startsWith("~/") || result.startsWith("~\\")) {
    result = path.join(os.homedir(), result.slice(1));
  }
  if (!path.isAbsolute(result) && dir) {
    result = path.join(dir, result);
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

// Language status: the server and the config file of the file in front.

/** The config file nearest to `file`, as the server finds them. */
function nearestConfig(file: string): string | undefined {
  for (let dir = path.dirname(file); ; dir = path.dirname(dir)) {
    const found = CONFIG_FILES.map((name) => path.join(dir, name)).find(isFile);
    if (found || path.dirname(dir) === dir) {
      return found;
    }
  }
}

function hasConfig(dir: string): boolean {
  return CONFIG_FILES.some((name) => isFile(path.join(dir, name)));
}

function updateStatus(): void {
  if (!serverStatus) {
    return;
  }
  const doc = vscode.window.activeTextEditor?.document;
  const key = doc?.uri.scheme === "file" ? folderKey(doc.uri) : undefined;
  const client = key !== undefined ? clients.get(key) : undefined;
  const restartCommand = { title: "Restart", command: "vfl.restartServer" };
  serverStatus.busy = false;
  serverStatus.severity = vscode.LanguageStatusSeverity.Information;
  if (key !== undefined && starting.has(key) && !client) {
    serverStatus.text = "vfl";
    serverStatus.detail = "finding the language server";
    serverStatus.busy = true;
  } else if (client?.state === State.Running) {
    const version = client.initializeResult?.serverInfo?.version;
    serverStatus.text = version ? `vfl ${version}` : "vfl";
    serverStatus.detail = "language server running";
  } else if (client?.state === State.Starting) {
    serverStatus.text = "vfl";
    serverStatus.detail = "language server starting";
    serverStatus.busy = true;
  } else {
    serverStatus.text = "vfl";
    serverStatus.detail = "language server not running";
    serverStatus.severity = vscode.LanguageStatusSeverity.Error;
  }
  serverStatus.command = restartCommand;

  const config = doc?.uri.scheme === "file" ? nearestConfig(doc.uri.fsPath) : undefined;
  if (config) {
    configStatus.text = vscode.workspace.asRelativePath(config);
    configStatus.detail = "the config file in effect";
    configStatus.command = {
      title: "Open",
      command: "vscode.open",
      arguments: [vscode.Uri.file(config)],
    };
  } else {
    configStatus.text = "no vfl.toml";
    const settings = vscode.workspace.getConfiguration("vfl");
    configStatus.detail = `preset ${settings.get("lint.preset")}, style ${settings.get("format.style")} from the settings`;
    configStatus.command = { title: "Generate", command: "vfl.generateConfig" };
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
  const client = runningClient();
  if (!client) {
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
  const client = runningClient();
  if (!client) {
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
/**
 * Flips `vfl.format.onType` where it is set: in the workspace folder or the
 * workspace when it is set there, else in the user settings. It is read at each keystroke, so the
 * change applies at once.
 */
async function toggleFormatOnType(): Promise<void> {
  // Scoped to the file in front, as the keystroke handler reads it.
  const config = vscode.workspace.getConfiguration(
    "vfl",
    vscode.window.activeTextEditor?.document,
  );
  const on = !config.get<boolean>("format.onType", false);
  const inspected = config.inspect<boolean>("format.onType");
  const target =
    inspected?.workspaceFolderValue !== undefined
      ? vscode.ConfigurationTarget.WorkspaceFolder
      : inspected?.workspaceValue !== undefined
        ? vscode.ConfigurationTarget.Workspace
        : vscode.ConfigurationTarget.Global;
  await config.update("format.onType", on, target);
  vscode.window.setStatusBarMessage(`vfl: format on type ${on ? "on" : "off"}`, 3000);
}

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

// Project config.

interface ProjectConfig {
  /** The config file at the project root: the one there, or `vfl.toml`. */
  path: string;
  /** The settings in effect, those at their default as comments. */
  text: string;
  exists: boolean;
}

/**
 * Writes the settings in effect (`vfl.lint.preset`, `vfl.format.style`, or
 * the `vfl.toml` there is) to the project's `vfl.toml` and opens it.
 * Returns the file written.
 */
async function generateConfig(): Promise<string | undefined> {
  const client = runningClient();
  if (!client) {
    return undefined;
  }
  let generated: ProjectConfig;
  try {
    generated = await client.sendRequest<ProjectConfig>("vfl/projectConfig", {});
  } catch (err) {
    void vscode.window.showErrorMessage(`vfl: ${err instanceof Error ? err.message : err}`);
    return undefined;
  }
  if (generated.exists) {
    const choice = await vscode.window.showWarningMessage(
      `${path.basename(generated.path)} exists. Replace it with the settings in effect?`,
      { modal: true },
      "Replace",
    );
    if (choice !== "Replace") {
      return undefined;
    }
  }
  const uri = vscode.Uri.file(generated.path);
  await vscode.workspace.fs.writeFile(uri, Buffer.from(generated.text, "utf8"));
  await vscode.window.showTextDocument(uri);
  return generated.path;
}

// Documentation and diagrams, by running `vfl doc`.

/** Runs `vfl` with `args` from `cwd`; resolves with its standard output. */
async function runVfl(args: string[], cwd: string): Promise<string | undefined> {
  const command = await serverCommand();
  if (!command) {
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
        // For the zoom of the diagrams; the page's CSP allows only it.
        enableScripts: true,
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
  const nonce = crypto.randomBytes(16).toString("base64");
  view.panel.webview.html = webviewHtml(html, view.page, dark, nonce);
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
