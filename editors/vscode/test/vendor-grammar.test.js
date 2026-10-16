// Tokenizes Quartus settings and SDC/XDC constraints with their grammars,
// as VS Code does, and checks the scopes.

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
  const registry = new textmate.Registry({
    onigLib: Promise.resolve({
      createOnigScanner: (patterns) => new oniguruma.OnigScanner(patterns),
      createOnigString: (s) => new oniguruma.OnigString(s),
    }),
    loadGrammar: async (scope) => {
      const file = path.join(__dirname, "..", "syntaxes", `${scope.slice("source.".length)}.tmLanguage.json`);
      return textmate.parseRawGrammar(fs.readFileSync(file, "utf8"), file);
    },
  });
  grammars.quartus = await registry.loadGrammar("source.quartus");
  grammars.sdc = await registry.loadGrammar("source.sdc");
  grammars.lpf = await registry.loadGrammar("source.lpf");
  grammars.cst = await registry.loadGrammar("source.cst");
});

function tokenize(grammar, source) {
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

function has(language, source, text, scope) {
  const tokens = tokenize(grammars[language], source);
  assert.ok(
    tokens.some(([t, s]) => t === text && s === scope),
    `expected ${JSON.stringify(text)} as ${scope}, got:\n${tokens.map((t) => t.join("  ")).join("\n")}`,
  );
}

test("Quartus assignments", () => {
  const line = 'set_global_assignment -name VHDL_FILE "rtl/top.vhd" -library work';
  has("quartus", line, "set_global_assignment", "support.function.vendor.quartus");
  has("quartus", line, "-name", "variable.parameter.option.quartus");
  has("quartus", line, "VHDL_FILE", "support.constant.assignment.quartus");
  has("quartus", line, "rtl/top.vhd", "string.quoted.double.quartus");
  has("quartus", line, "-library", "variable.parameter.option.quartus");
  has("quartus", "set_location_assignment PIN_AB12 -to clk", "PIN_AB12", "constant.other.pin.quartus");
  has("quartus", "# Pins\nset a 1 ;# note", "# Pins", "comment.line.number-sign.quartus");
  has("quartus", "set a 1 ;# note", "# note", "comment.line.number-sign.quartus");
  has("quartus", "[file join $::quartus(qip_path) a.vhd]", "$::quartus(qip_path)", "variable.other.quartus");
  has("quartus", "if {$x} { set y 1 }", "if", "keyword.control.quartus");
});

test("SDC and XDC constraints", () => {
  const clock = "create_clock -name clk -period 10.0 [get_ports {clk}]";
  has("sdc", clock, "create_clock", "support.function.vendor.sdc");
  has("sdc", clock, "-period", "variable.parameter.option.sdc");
  has("sdc", clock, "10.0", "constant.numeric.sdc");
  has("sdc", clock, "get_ports", "support.function.collection.sdc");
  const pin = "set_property -dict {PACKAGE_PIN W5 IOSTANDARD LVCMOS33} [get_ports clk]";
  has("sdc", "set_property PACKAGE_PIN W5 [get_ports clk]", "PACKAGE_PIN", "support.constant.property.sdc");
  has("sdc", pin, "-dict", "variable.parameter.option.sdc");
  has("sdc", "derive_pll_clocks", "derive_pll_clocks", "support.function.vendor.sdc");
});

test("Libero and Radiant physical constraints", () => {
  has("sdc", "set_io clk -pinname A1 -fixed yes", "set_io", "support.function.vendor.sdc");
  has("sdc", "ldc_set_location -site {A1} [get_ports clk]", "ldc_set_location", "support.function.vendor.sdc");
});

test("Lattice preferences", () => {
  const locate = 'LOCATE COMP "led[0]" SITE "A1" ; # first led';
  has("lpf", locate, "LOCATE", "keyword.other.statement.lpf");
  has("lpf", locate, "COMP", "storage.type.object.lpf");
  has("lpf", locate, "led[0]", "string.quoted.double.lpf");
  has("lpf", locate, "# first led", "comment.line.number-sign.lpf");
  const iobuf = 'IOBUF PORT "clk" IO_TYPE=LVCMOS33 PULLMODE=UP;';
  has("lpf", iobuf, "IO_TYPE", "support.constant.attribute.lpf");
  has("lpf", 'FREQUENCY PORT "clk" 12.0 MHZ;', "12.0", "constant.numeric.lpf");
});

test("Gowin constraints", () => {
  has("cst", 'IO_LOC "clk" 35; // clock', "IO_LOC", "keyword.other.statement.cst");
  has("cst", 'IO_LOC "clk" 35; // clock', "// clock", "comment.line.double-slash.cst");
  has("cst", 'IO_PORT "clk" IO_TYPE=LVCMOS33 PULL_MODE=UP;', "PULL_MODE", "support.constant.attribute.cst");
});
