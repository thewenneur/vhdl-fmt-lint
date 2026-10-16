// Checks the VHDL snippets: their shape, that each one expands to VHDL that
// `vfl lint` finds no error in (when target/debug/vfl is built), and that
// the copy for Zed is up to date. UPDATE_ZED_SNIPPETS=1 rewrites that copy.

const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const root = path.join(__dirname, "..");
const pkg = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
const file = path.join(root, "snippets", "vhdl.json");
const snippets = JSON.parse(fs.readFileSync(file, "utf8"));
const zedFile = path.join(root, "..", "zed", "snippets", "vhdl.json");
const vfl = path.join(root, "..", "..", "target", "debug", process.platform === "win32" ? "vfl.exe" : "vfl");

const prefixes = (s) => (Array.isArray(s.prefix) ? s.prefix : [s.prefix]);

/** The text a snippet inserts when every placeholder keeps its default. */
function expand(body, fileBase) {
  let text = body.join("\n").replace(/\$\{?TM_FILENAME_BASE\}?/g, fileBase);
  const values = new Map();
  // Innermost placeholders first, so that nested ones are resolved.
  for (;;) {
    const next = text
      .replace(/\$\{(\d+)\|([^,|]*)[^|]*\|\}/g, (_, n, first) => {
        values.set(n, first);
        return first;
      })
      .replace(/\$\{(\d+):([^${}]*)\}/g, (_, n, value) => {
        values.set(n, value);
        return value;
      });
    if (next === text) {
      break;
    }
    text = next;
  }
  return text.replace(/\$(\d+)/g, (_, n) => values.get(n) ?? "").replace(/\t/g, "    ");
}

/** Snippets that are design units of a file of their own. */
const UNITS = new Set(["Entity and architecture", "Testbench", "Package and body"]);
/** Snippets that are an architecture of entity `top`. */
const ARCHITECTURES = new Set(["Architecture", "State machine"]);
/** Snippets that are declarations. */
const DECLARATIONS = new Set(["Record type"]);

/** A VHDL file holding `name`'s expansion where it belongs. */
function vhdlFile(name, text) {
  const entity = [
    "library ieee;",
    "use ieee.std_logic_1164.all;",
    "",
    "entity top is",
    "    port (clk, rst : in std_logic);",
    "end entity top;",
    "",
  ].join("\n");
  if (UNITS.has(name)) {
    return text;
  }
  if (ARCHITECTURES.has(name)) {
    return `${entity}\n${text}\n`;
  }
  const declarations = DECLARATIONS.has(name) ? text : "";
  const statements = DECLARATIONS.has(name) ? "" : text;
  return [
    entity,
    "architecture rtl of top is",
    "    type state_t is (IDLE, RUN);",
    "    signal state : state_t;",
    declarations,
    "begin",
    statements,
    "end architecture rtl;",
    "",
  ].join("\n");
}

test("snippets are contributed for VHDL", () => {
  assert.deepEqual(pkg.contributes.snippets, [{ language: "vhdl", path: "./snippets/vhdl.json" }]);
});

test("every snippet has a unique prefix, a body and a description", () => {
  const seen = new Set();
  for (const [name, s] of Object.entries(snippets)) {
    assert.ok(Array.isArray(s.body) && s.body.length > 0, name);
    assert.ok(s.description, name);
    for (const prefix of prefixes(s)) {
      assert.ok(!seen.has(prefix), `${name}: prefix ${prefix} is taken`);
      seen.add(prefix);
    }
  }
  for (const name of [...UNITS, ...ARCHITECTURES, ...DECLARATIONS]) {
    assert.ok(snippets[name], `no snippet ${name}`);
  }
});

test("expanded snippets are valid VHDL", { skip: !fs.existsSync(vfl) && `${vfl} is not built` }, () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "vfl-snippets-"));
  try {
    for (const [name, s] of Object.entries(snippets)) {
      // One project per snippet, since several declare `top`.
      const project = path.join(dir, name.replace(/\W+/g, "_"));
      fs.mkdirSync(project);
      fs.writeFileSync(path.join(project, "vfl.toml"), "root = true\n");
      const text = vhdlFile(name, expand(s.body, "top"));
      fs.writeFileSync(path.join(project, "top.vhd"), text);
      try {
        execFileSync(vfl, ["lint", "--output-format", "json", "."], { cwd: project, encoding: "utf8" });
      } catch (err) {
        assert.fail(`${name}:\n${text}\n${err.stdout}${err.stderr}`);
      }
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

/**
 * The snippets for Zed: the same, without what Zed's snippets lack (choices
 * and variables), which fall back to their first choice or a placeholder.
 */
function zedSnippets() {
  const out = {};
  for (const [name, s] of Object.entries(snippets)) {
    const body = s.body.map((line) =>
      line
        .replace(/\$\{(\d+)\|([^,|]*)[^|]*\|\}/g, "${$1:$2}")
        .replace(/\$\{(\d+):\$\{TM_FILENAME_BASE\}\}/g, "${$1:name}"),
    );
    out[name] = { ...s, body };
  }
  return `${JSON.stringify(out, null, 2)}\n`;
}

test("the Zed snippets are a copy of these", () => {
  const expected = zedSnippets();
  if (process.env.UPDATE_ZED_SNIPPETS) {
    fs.mkdirSync(path.dirname(zedFile), { recursive: true });
    fs.writeFileSync(zedFile, expected);
  }
  assert.equal(
    fs.readFileSync(zedFile, "utf8"),
    expected,
    "editors/zed/snippets/vhdl.json is out of date: run `UPDATE_ZED_SNIPPETS=1 npm test`",
  );
  assert.ok(!expected.includes("TM_FILENAME_BASE") && !/\$\{\d+\|/.test(expected));
});
