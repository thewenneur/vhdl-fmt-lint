// Checks that every command can be bound to a key: each command the
// extension registers is declared in package.json (so it is listed in
// Keyboard Shortcuts), and each declared one is registered.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const root = path.join(__dirname, "..");
const pkg = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
const declared = pkg.contributes.commands.map((c) => c.command);

/**
 * The ids passed as string literals to `registerCommand` in src/. The one
 * internal command, the target of the links of the diagrams view, is
 * registered through a constant and stays out of the list on purpose.
 */
function registered() {
  const ids = new Set();
  for (const file of fs.readdirSync(path.join(root, "src"))) {
    const text = fs.readFileSync(path.join(root, "src", file), "utf8");
    for (const m of text.matchAll(/registerCommand\(\s*"([^"]+)"/g)) {
      ids.add(m[1]);
    }
  }
  return ids;
}

test("every registered command is declared", () => {
  const ids = registered();
  assert.ok(ids.size >= declared.length);
  for (const id of ids) {
    assert.ok(declared.includes(id), `${id} is not in contributes.commands`);
  }
});

test("every declared command is registered", () => {
  const ids = registered();
  for (const id of declared) {
    assert.ok(ids.has(id), `${id} is declared but never registered`);
  }
});

test("every command is in the vfl category", () => {
  for (const c of pkg.contributes.commands) {
    assert.equal(c.category, "vfl", c.command);
    assert.ok(c.command.startsWith("vfl."), c.command);
  }
});

test("menu entries name declared commands", () => {
  for (const [menu, items] of Object.entries(pkg.contributes.menus)) {
    for (const item of items) {
      assert.ok(declared.includes(item.command), `${menu}: ${item.command}`);
    }
  }
});
