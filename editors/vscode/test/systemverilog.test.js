// Tokenizes Verilog and SystemVerilog with the grammar, as VS Code does,
// and checks the scopes.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const oniguruma = require("vscode-oniguruma");
const textmate = require("vscode-textmate");

const grammars = {};

test.before(async () => {
  const wasm = fs.readFileSync(require.resolve("vscode-oniguruma/release/onig.wasm"));
  await oniguruma.loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength));
  const files = {
    "source.systemverilog": "systemverilog.tmLanguage.json",
    "source.verilog": "verilog.tmLanguage.json",
  };
  const registry = new textmate.Registry({
    onigLib: Promise.resolve({
      createOnigScanner: (patterns) => new oniguruma.OnigScanner(patterns),
      createOnigString: (s) => new oniguruma.OnigString(s),
    }),
    loadGrammar: async (scope) => {
      const file = path.join(__dirname, "..", "syntaxes", files[scope]);
      return textmate.parseRawGrammar(fs.readFileSync(file, "utf8"), file);
    },
  });
  for (const scope of Object.keys(files)) {
    grammars[scope] = await registry.loadGrammar(scope);
  }
});

/** The `[text, innermost scope]` pairs of each line, without whitespace tokens. */
function tokenize(source, scope) {
  let state = textmate.INITIAL;
  const tokens = [];
  for (const line of source.split("\n")) {
    const result = grammars[scope].tokenizeLine(line, state);
    for (const t of result.tokens) {
      const text = line.slice(t.startIndex, t.endIndex);
      if (text.trim()) {
        tokens.push([text, t.scopes[t.scopes.length - 1]]);
      }
    }
    state = result.ruleStack;
  }
  return tokens;
}

/** Asserts that `text` is tokenized as `scope` somewhere in `source`. */
function has(source, text, scope, grammar = "source.systemverilog") {
  const tokens = tokenize(source, grammar);
  assert.ok(
    tokens.some(([t, s]) => t === text && s === scope),
    `expected ${JSON.stringify(text)} as ${scope}, got:\n${tokens.map((t) => t.join("  ")).join("\n")}`,
  );
}

test("comments and strings", () => {
  has("assign a = b; // note", " note", "comment.line.double-slash.systemverilog");
  has("/* one\ntwo */ wire x;", "two ", "comment.block.systemverilog");
  has('$display("n=%0d\\n", n);', "%0d", "constant.other.placeholder.systemverilog");
  has('$display("n=%0d\\n", n);', "\\n", "constant.character.escape.systemverilog");
});

test("numbers", () => {
  has("x = 8'hFF;", "8'hFF", "constant.numeric.systemverilog");
  has("x = 4'b10_xz;", "4'b10_xz", "constant.numeric.systemverilog");
  has("x = '0;", "'0", "constant.numeric.systemverilog");
  has("#10ns x = 1.5e3;", "1.5e3", "constant.numeric.systemverilog");
});

test("directives and system tasks", () => {
  has("`include \"defs.svh\"", '"defs.svh"', "string.quoted.other.systemverilog");
  has("`ifdef SIM", "SIM", "entity.name.function.preprocessor.systemverilog");
  has("`endif", "`endif", "keyword.control.directive.systemverilog");
  has("x = `WIDTH;", "`WIDTH", "entity.name.function.preprocessor.systemverilog");
  has("$display(x);", "$display", "support.function.systemverilog");
});

test("keywords and types", () => {
  has("always_ff @(posedge clk) begin", "always_ff", "keyword.other.systemverilog");
  has("always_ff @(posedge clk) begin", "begin", "keyword.control.systemverilog");
  has("input logic [7:0] a,", "logic", "storage.type.systemverilog");
  has("input logic [7:0] a,", "input", "keyword.other.systemverilog");
  has("unique case (s)", "case", "keyword.control.systemverilog");
});

test("declarations and instances", () => {
  has("module counter #(", "counter", "entity.name.type.systemverilog");
  has("package pkg;", "pkg", "entity.name.type.systemverilog");
  has("function automatic logic [3:0] f(input a);", "f", "entity.name.function.systemverilog");
  has("endmodule : counter", "counter", "entity.name.label.systemverilog");
  has("begin : g_loop", "g_loop", "entity.name.label.systemverilog");
  has("  fifo #(.W(8)) u_fifo (", "fifo", "entity.name.type.module.systemverilog");
  has("  fifo #(.W(8)) u_fifo (", "u_fifo", "entity.name.label.systemverilog");
  has("  fifo u_fifo (", "u_fifo", "entity.name.label.systemverilog");
  // Keywords followed by a name and `(` are not instances.
  has("  function automatic f(", "f", "entity.name.function.systemverilog");
});

test("the Verilog grammar is the SystemVerilog one", () => {
  has("module top;", "top", "entity.name.type.systemverilog", "source.verilog");
  has("wire a; // x", " x", "comment.line.double-slash.systemverilog", "source.verilog");
});
