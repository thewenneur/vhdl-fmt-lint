// Tokenizes VHDL with the grammar, as VS Code does, and checks the scopes.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const oniguruma = require("vscode-oniguruma");
const textmate = require("vscode-textmate");

let grammar;

test.before(async () => {
  const wasm = fs.readFileSync(require.resolve("vscode-oniguruma/release/onig.wasm"));
  await oniguruma.loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength));
  const registry = new textmate.Registry({
    onigLib: Promise.resolve({
      createOnigScanner: (patterns) => new oniguruma.OnigScanner(patterns),
      createOnigString: (s) => new oniguruma.OnigString(s),
    }),
    loadGrammar: async () => {
      const file = path.join(__dirname, "..", "syntaxes", "vhdl.tmLanguage.json");
      return textmate.parseRawGrammar(fs.readFileSync(file, "utf8"), file);
    },
  });
  grammar = await registry.loadGrammar("source.vhdl");
});

/** The `[text, innermost scope]` pairs of each line, without whitespace tokens. */
function tokenize(source) {
  let state = textmate.INITIAL;
  const tokens = [];
  for (const line of source.split("\n")) {
    const result = grammar.tokenizeLine(line, state);
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
function has(source, text, scope) {
  const tokens = tokenize(source);
  assert.ok(
    tokens.some(([t, s]) => t === text && s === scope),
    `expected ${JSON.stringify(text)} as ${scope}, got:\n${tokens.map((t) => t.join("  ")).join("\n")}`,
  );
}

test("comments", () => {
  has("a <= b; -- note", "-- note", "comment.line.double-dash.vhdl");
  has("/* one\ntwo */ x", "two ", "comment.block.vhdl");
});

test("strings and bit strings", () => {
  has('report "say ""hi""";', '""', "constant.character.escape.vhdl");
  has('x <= X"FF";', 'X"FF"', "constant.numeric.bit-string.vhdl");
  has('x <= 12UX"F-";', '12UX"F-"', "constant.numeric.bit-string.vhdl");
});

test("numbers", () => {
  has("c := 16#FF#;", "16#FF#", "constant.numeric.based.vhdl");
  has("c := 1_000.5e-3;", "1_000.5e-3", "constant.numeric.decimal.vhdl");
});

test("character literals and attributes", () => {
  has("q <= '1';", "'1'", "constant.character.vhdl");
  has("x <= (others => '0');", "'0'", "constant.character.vhdl");
  has("n := v'length;", "length", "entity.other.attribute-name.vhdl");
  has("if rising_edge(clk) and clk'event then", "event", "entity.other.attribute-name.vhdl");
  has("y := t'('1');", "'1'", "constant.character.vhdl");
});

test("keywords are case-insensitive", () => {
  has("ENTITY foo IS", "ENTITY", "storage.type.vhdl");
  has("if a then", "then", "keyword.control.vhdl");
  has("y <= a xor b;", "xor", "keyword.operator.word.vhdl");
  has("port (clk : in std_logic);", "in", "storage.modifier.vhdl");
});

test("declarations", () => {
  has("entity counter is", "counter", "entity.name.type.vhdl");
  has("architecture rtl of counter is", "rtl", "entity.name.type.architecture.vhdl");
  has("architecture rtl of counter is", "counter", "entity.name.type.vhdl");
  has("package body util is", "util", "entity.name.type.package.vhdl");
  has("pure function add(a, b : integer) return integer is", "add", "entity.name.function.vhdl");
  has("type state_t is (idle, run);", "state_t", "entity.name.type.vhdl");
  has("end architecture rtl;", "rtl", "entity.name.label.vhdl");
});

test("labels and instantiations", () => {
  has("  p_reg : process (clk)", "p_reg", "entity.name.label.vhdl");
  has("  u0 : entity work.counter(rtl)", "counter", "entity.name.type.vhdl");
  has("  u0 : entity work.counter(rtl)", "rtl", "entity.name.type.architecture.vhdl");
});

test("standard types, functions and constants", () => {
  has("signal s : std_logic_vector(7 downto 0);", "std_logic_vector", "support.type.vhdl");
  has("n := to_integer(unsigned(v));", "to_integer", "support.function.vhdl");
  has("severity failure;", "failure", "constant.language.vhdl");
});

test("operators", () => {
  has("a <= b;", "<=", "keyword.operator.assignment.vhdl");
  has("port map (a => b)", "=>", "keyword.operator.association.vhdl");
  has("if a ?= b then", "?=", "keyword.operator.vhdl");
});

test("extended identifiers and tool directives", () => {
  has("signal \\my sig\\ : bit;", "\\my sig\\", "variable.other.extended.vhdl");
  has("`if TOOL_NAME = \"x\" then", "if", "keyword.control.directive.vhdl");
});
