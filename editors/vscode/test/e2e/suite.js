// Runs inside VS Code (see run.js): checks that the extension starts the
// language server and that its features reach the editor.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vscode = require("vscode");

/** Polls `get` until it returns a truthy value. */
async function until(what, get, timeoutMs = 30000) {
  const end = Date.now() + timeoutMs;
  for (;;) {
    const value = await get();
    if (value) {
      return value;
    }
    if (Date.now() > end) {
      throw new Error(`timed out waiting for ${what}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
}

async function run() {
  const folder = vscode.workspace.workspaceFolders[0].uri.fsPath;
  const uri = vscode.Uri.file(path.join(folder, "e2e_top.vhd"));
  const doc = await vscode.workspace.openTextDocument(uri);
  await vscode.window.showTextDocument(doc);
  assert.equal(doc.languageId, "vhdl");

  const extension = vscode.extensions.getExtension("thewenneur.vfl");
  await until("the extension to activate", () => extension.isActive);

  const diagnostics = await until("diagnostics", () => {
    const found = vscode.languages.getDiagnostics(uri);
    return found.length > 0 && found;
  });
  const unused = diagnostics.find((d) => d.code === "sema/unused-signal");
  assert.ok(unused, `no sema/unused-signal in ${JSON.stringify(diagnostics)}`);
  assert.equal(unused.source, "vfl");
  assert.equal(unused.range.start.line, 8);

  const edits = await vscode.commands.executeCommand(
    "vscode.executeFormatDocumentProvider",
    uri,
    { tabSize: 2, insertSpaces: true },
  );
  assert.ok(edits && edits.length > 0, "formatting returned no edits");

  // `r` in `q <= r;` goes to `signal r`.
  const use = doc.lineAt(16).text.lastIndexOf("r");
  const locations = await vscode.commands.executeCommand(
    "vscode.executeDefinitionProvider",
    uri,
    new vscode.Position(16, use),
  );
  assert.ok(locations && locations.length > 0, "no definition found");
  const target = locations[0].targetRange ?? locations[0].range;
  assert.equal(target.start.line, 9);

  const fixes = await vscode.commands.executeCommand(
    "vscode.executeCodeActionProvider",
    uri,
    unused.range,
  );
  assert.ok(fixes && fixes.length > 0, "no quick fix for the unused signal");

  // Code generation: an instantiation at the cursor, and a testbench file.
  const scratch = await vscode.workspace.openTextDocument({ language: "vhdl", content: "" });
  await vscode.window.showTextDocument(scratch);
  await vscode.commands.executeCommand("vfl.insertInstance", "work.e2e_top");
  assert.match(scratch.getText(), /^u_e2e_top : entity work\.e2e_top\n  port map \(/);
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor");

  await vscode.commands.executeCommand("vfl.generateTestbench", "e2e_top");
  const tb = vscode.window.activeTextEditor?.document;
  assert.equal(tb && path.basename(tb.uri.fsPath), "e2e_top_tb.vhd");
  assert.match(tb.getText(), /dut : entity work\.e2e_top/);

  // Documentation: written where `[doc]` says, the index returned.
  const index = await vscode.commands.executeCommand("vfl.generateDocs");
  assert.equal(index, path.join(folder, "doc", "index.html"));
  assert.ok(fs.existsSync(path.join(folder, "doc", "work.e2e_top.html")));

  // Diagrams: the page of the file's entity, in a webview beside the editor.
  const page = await vscode.commands.executeCommand("vfl.showDiagrams", uri);
  assert.equal(path.basename(page), "work.e2e_top.html");
  assert.ok(!page.startsWith(folder), `${page} is not in the extension's storage`);
  const webviewTab = (label) =>
    vscode.window.tabGroups.all
      .flatMap((g) => g.tabs)
      .find((t) => t.input instanceof vscode.TabInputWebview && t.label === label);
  await until("the diagrams view", () => webviewTab("e2e_top"));
  await vscode.commands.executeCommand("vfl.openDocPage", "index.html");
  await until("the index in the diagrams view", () => webviewTab("Documentation"));
}

module.exports = { run };
