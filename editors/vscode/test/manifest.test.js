// Checks the parts of package.json that point elsewhere: the vfl.toml schema,
// the settings a workspace you do not trust cannot set, the icon.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const root = path.join(__dirname, "..");
const pkg = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
const ignore = fs.readFileSync(path.join(root, ".vscodeignore"), "utf8").split(/\r?\n/);

test("the vfl.toml schema is the one of this version", () => {
  const [validation] = pkg.contributes.tomlValidation;
  assert.deepEqual(validation.fileMatch, ["vfl.toml", ".vfl.toml"]);
  // Even Better TOML fetches the URL as given, so it names the release tag,
  // and the schema there is the file in this directory.
  const prefix = `https://raw.githubusercontent.com/thewenneur/vhdl-fmt-lint/v${pkg.version}/editors/vscode/`;
  assert.ok(validation.url.startsWith(prefix), `${validation.url} is not under ${prefix}`);
  const schema = JSON.parse(fs.readFileSync(path.join(root, validation.url.slice(prefix.length)), "utf8"));
  assert.equal(schema.title, "vfl.toml");
  assert.ok(ignore.includes("!schemas/**"));
});

test("an untrusted workspace cannot choose the program the extension runs", () => {
  const trust = pkg.capabilities.untrustedWorkspaces;
  assert.equal(trust.supported, "limited");
  const settings = Object.keys(pkg.contributes.configuration.properties);
  for (const key of trust.restrictedConfigurations) {
    assert.ok(settings.includes(key), key);
  }
  assert.deepEqual(trust.restrictedConfigurations.sort(), settings.filter((k) => k.startsWith("vfl.server.") && k !== "vfl.server.trace").sort());
});

test("vfl formats VHDL, Verilog and SystemVerilog by default", () => {
  for (const language of ["vhdl", "verilog", "systemverilog"]) {
    const defaults = pkg.contributes.configurationDefaults[`[${language}]`];
    assert.equal(defaults["editor.defaultFormatter"], `${pkg.publisher}.${pkg.name}`, language);
  }
});

test("the icon and the changelog are packaged", () => {
  assert.ok(fs.statSync(path.join(root, pkg.icon)).isFile());
  assert.ok(ignore.includes(`!${pkg.icon}`));
  assert.ok(fs.statSync(path.join(root, "CHANGELOG.md")).isFile());
  assert.ok(ignore.includes("!snippets/**"));
});
