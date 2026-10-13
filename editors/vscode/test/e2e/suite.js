// Runs inside VS Code (see run.js): checks that the extension starts the
// language server and that its features reach the editor.

const assert = require("node:assert/strict");
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
  const titles = fixes.map((f) => f.title);
  assert.ok(
    titles.includes("Remove the unused signal `unused_s` (may change behavior)"),
    `no removal in ${JSON.stringify(titles)}`,
  );
  assert.ok(titles.includes("Ignore `sema/unused-signal` on this line"));

  // Renaming `r` changes its declaration and its three uses.
  const rename = await vscode.commands.executeCommand(
    "vscode.executeDocumentRenameProvider",
    uri,
    new vscode.Position(16, use),
    "state",
  );
  const renamed = rename.get(uri);
  assert.equal(renamed.length, 4, JSON.stringify(renamed));
  assert.ok(renamed.every((e) => e.newText === "state"));

  const references = await vscode.commands.executeCommand(
    "vscode.executeReferenceProvider",
    uri,
    new vscode.Position(16, use),
  );
  assert.equal(references.length, 4, JSON.stringify(references));

  // Names are colored by what they resolve to: `r` in `r <= not r;` is a
  // signal being assigned.
  const legend = await vscode.commands.executeCommand(
    "vscode.provideDocumentSemanticTokensLegend",
    uri,
  );
  assert.ok(legend && legend.tokenTypes.includes("signal"), "no semantic tokens legend");
  const tokens = await vscode.commands.executeCommand(
    "vscode.provideDocumentSemanticTokens",
    uri,
  );
  assert.ok(tokens && tokens.data.length > 0, "no semantic tokens");
  const decoded = [];
  let line = 0;
  let column = 0;
  for (let i = 0; i < tokens.data.length; i += 5) {
    line += tokens.data[i];
    column = tokens.data[i] === 0 ? column + tokens.data[i + 1] : tokens.data[i + 1];
    decoded.push({ line, column, type: legend.tokenTypes[tokens.data[i + 3]], modifiers: tokens.data[i + 4] });
  }
  const assigned = decoded.find((t) => t.line === 13 && t.column === 6);
  assert.equal(assigned?.type, "signal", JSON.stringify(decoded));
  assert.ok(assigned.modifiers & (1 << legend.tokenModifiers.indexOf("modification")));
  const clk = decoded.find((t) => t.line === 4 && t.column === 8);
  assert.equal(clk?.type, "port");
  assert.ok(clk.modifiers & (1 << legend.tokenModifiers.indexOf("in")));

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
}

module.exports = { run };
