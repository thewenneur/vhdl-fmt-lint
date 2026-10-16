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
  assert.match(scratch.getText(), /^u_e2e_top : entity work\.e2e_top\n    port map \(/);
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor");

  // The snippets are there for VHDL.
  const snippet = await vscode.workspace.openTextDocument({ language: "vhdl", content: "" });
  await vscode.window.showTextDocument(snippet);
  await vscode.commands.executeCommand("editor.action.insertSnippet", {
    langId: "vhdl",
    name: "Record type",
  });
  assert.match(snippet.getText(), /^type name_t is record\n\s+field : std_logic;\nend record name_t;$/);
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor");
  for (const language of ["vhdl", "verilog", "systemverilog"]) {
    const formatter = vscode.workspace
      .getConfiguration("editor", { languageId: language })
      .get("defaultFormatter");
    assert.equal(formatter, "thewenneur.vfl", language);
  }

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

  // SystemVerilog: formatting, the outline and folding.
  const svUri = vscode.Uri.file(path.join(folder, "e2e_counter.sv"));
  const sv = await vscode.workspace.openTextDocument(svUri);
  await vscode.window.showTextDocument(sv);
  assert.equal(sv.languageId, "systemverilog");
  const svEdits = await until("SystemVerilog formatting", () =>
    vscode.commands.executeCommand("vscode.executeFormatDocumentProvider", svUri, {
      tabSize: 2,
      insertSpaces: true,
    }),
  );
  assert.ok(svEdits.length > 0, "SystemVerilog formatting returned no edits");
  const symbols = await vscode.commands.executeCommand(
    "vscode.executeDocumentSymbolProvider",
    svUri,
  );
  assert.equal(symbols?.[0]?.name, "e2e_counter", JSON.stringify(symbols));
  const folds = await vscode.commands.executeCommand("vscode.executeFoldingRangeProvider", svUri);
  assert.ok(
    folds?.some((f) => f.start === 0),
    `no fold for the module in ${JSON.stringify(folds)}`,
  );

  // Without a vfl.toml, the preset and style settings apply, and become one
  // on request.
  // `sema/unused-signal` is reported until the server has the new preset;
  // without a `vfl.toml` the default preset would still report it.
  assert.ok(vscode.languages.getDiagnostics(uri).some((d) => d.code === "sema/unused-signal"));
  fs.rmSync(path.join(folder, "vfl.toml"));
  await vscode.workspace
    .getConfiguration("vfl")
    .update("lint.preset", "none", vscode.ConfigurationTarget.Workspace);
  await until("no findings with the `none` preset", () =>
    vscode.languages.getDiagnostics(uri).every((d) => d.code !== "sema/unused-signal"),
  );
  const written = await vscode.commands.executeCommand("vfl.generateConfig");
  assert.equal(written, path.join(folder, "vfl.toml"));
  assert.match(fs.readFileSync(written, "utf8"), /^preset = "none"$/m);
}

module.exports = { run };
