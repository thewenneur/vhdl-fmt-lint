// Checks the tree provider of the VHDL Hierarchy view against a fake
// `vscode` module and a fake language client.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const Module = require("node:module");
const path = require("node:path");
const test = require("node:test");
const esbuild = require("esbuild");

/** The parts of the `vscode` API the provider uses. */
const vscode = {
  TreeItemCollapsibleState: { None: 0, Collapsed: 1, Expanded: 2 },
  TreeItem: class {
    constructor(label, collapsibleState) {
      this.label = label;
      this.collapsibleState = collapsibleState;
    }
  },
  ThemeIcon: class {
    constructor(id, color) {
      this.id = id;
      this.color = color;
    }
  },
  ThemeColor: class {
    constructor(id) {
      this.id = id;
    }
  },
  EventEmitter: class {
    constructor() {
      this.listeners = [];
      this.event = (listener) => this.listeners.push(listener);
    }
    fire(value) {
      this.listeners.forEach((l) => l(value));
    }
    dispose() {}
  },
  Range: class {
    constructor(startLine, startCharacter, endLine, endCharacter) {
      Object.assign(this, { startLine, startCharacter, endLine, endCharacter });
    }
  },
  Uri: { parse: (uri) => ({ uri }) },
  workspace: {
    asRelativePath: (p) => (typeof p === "string" ? p : p.uri).replace(/^(file:\/\/)?\/project\//, ""),
    getConfiguration: () => ({ get: (_key, value) => value }),
  },
};

/** src/hierarchy.ts, compiled on the fly, with the fake `vscode`. */
function load() {
  const file = path.join(__dirname, "..", "src", "hierarchy.ts");
  const { code } = esbuild.transformSync(fs.readFileSync(file, "utf8"), {
    loader: "ts",
    format: "cjs",
  });
  const m = new Module(file);
  m.require = (id) => (id === "vscode" ? vscode : require(id));
  m._compile(code, file);
  return m.exports;
}

const { HierarchyProvider, assignIds, topItems } = load();

const place = (file, line, character) => ({
  uri: `file:///project/${file}`,
  range: { start: { line, character }, end: { line, character: character + 3 } },
});

/** What the server answers for the UART example, cut down. */
function answer() {
  return {
    roots: [
      {
        kind: "entity",
        label: "tb_uart",
        name: "tb_uart",
        unitKind: "entity",
        library: "work",
        architecture: "sim",
        testbench: true,
        location: place("tb/tb_uart.vhd", 4, 7),
        entityLocation: place("tb/tb_uart.vhd", 4, 7),
        children: [
          {
            kind: "instance",
            label: "dut",
            name: "uart",
            unitKind: "entity",
            library: "work",
            architecture: "rtl",
            location: place("tb/tb_uart.vhd", 21, 2),
            entityLocation: place("rtl/uart.vhd", 9, 7),
            children: [
              {
                kind: "generate",
                label: "gen",
                detail: "for i in 0 to 3",
                location: place("rtl/uart.vhd", 40, 2),
                children: [
                  {
                    kind: "instance",
                    label: "u_fifo",
                    name: "fifo",
                    unitKind: "component",
                    library: "lib",
                    architecture: "rtl",
                    location: place("rtl/uart.vhd", 41, 4),
                    entityLocation: place("rtl/fifo.vhd", 7, 7),
                    children: [],
                  },
                  {
                    kind: "instance",
                    label: "u_fifo",
                    name: "fifo",
                    unitKind: "entity",
                    library: "work",
                    location: place("rtl/uart.vhd", 42, 4),
                    entityLocation: place("rtl/fifo.vhd", 7, 7),
                    children: [],
                  },
                ],
              },
              {
                kind: "instance",
                label: "u_gone",
                name: "gone",
                unitKind: "entity",
                library: "work",
                unresolved: true,
                location: place("rtl/uart.vhd", 50, 2),
                children: [],
              },
            ],
          },
        ],
      },
    ],
    entities: [
      { library: "work", name: "fifo", path: "/project/rtl/fifo.vhd", top: false, testbench: false },
      { library: "work", name: "tb_uart", path: "/project/tb/tb_uart.vhd", top: true, testbench: true },
    ],
  };
}

function fakeClient(reply) {
  const calls = [];
  return {
    calls,
    sendRequest: async (method, params) => {
      calls.push({ method, params });
      if (reply instanceof Error) {
        throw reply;
      }
      return reply;
    },
  };
}

test("asks the server for the hierarchy with the settings", async () => {
  const client = fakeClient(answer());
  const provider = new HierarchyProvider(
    () => client,
    () => ({ top: "work.uart", testbenches: false }),
  );
  const roots = await provider.getChildren();
  assert.deepEqual(client.calls, [
    { method: "vfl/hierarchy", params: { top: "work.uart", testbenches: false } },
  ]);
  assert.equal(roots.length, 1);
  assert.equal(provider.message, undefined);
  // Children come from the tree already fetched.
  const [dut] = await provider.getChildren(roots[0]);
  assert.equal(dut.label, "dut");
  assert.equal(client.calls.length, 1);
});

test("shows instances with their entity, architecture and flags", async () => {
  const provider = new HierarchyProvider(() => fakeClient(answer()));
  const [root] = await provider.getChildren();
  const top = provider.getTreeItem(root);
  assert.equal(top.label, "tb_uart");
  assert.equal(top.description, "(sim) testbench");
  assert.equal(top.iconPath.id, "beaker");
  assert.equal(top.collapsibleState, vscode.TreeItemCollapsibleState.Expanded);
  assert.equal(top.contextValue, "entity");

  const dut = root.children[0];
  const item = provider.getTreeItem(dut);
  assert.equal(item.description, "uart(rtl)");
  assert.equal(item.collapsibleState, vscode.TreeItemCollapsibleState.Collapsed);
  assert.equal(item.tooltip, "dut: uart(rtl)\nrtl/uart.vhd");
  // Clicking goes to the instance's label.
  assert.equal(item.command.command, "vscode.open");
  assert.deepEqual(item.command.arguments[0], { uri: "file:///project/tb/tb_uart.vhd" });
  assert.deepEqual({ ...item.command.arguments[1].selection }, {
    startLine: 21,
    startCharacter: 2,
    endLine: 21,
    endCharacter: 5,
  });

  const [gen, gone] = dut.children;
  const group = provider.getTreeItem(gen);
  assert.equal(group.description, "for i in 0 to 3");
  assert.equal(group.contextValue, "generate");
  assert.equal(group.iconPath.id, "symbol-array");
  const component = provider.getTreeItem(gen.children[0]);
  assert.equal(component.description, "lib.fifo(rtl) · component");
  assert.equal(component.iconPath.id, "symbol-interface");

  const missing = provider.getTreeItem(gone);
  assert.equal(missing.description, "gone · not found");
  assert.equal(missing.iconPath.id, "warning");
  assert.equal(missing.contextValue, "unit");
  assert.equal(missing.collapsibleState, vscode.TreeItemCollapsibleState.None);
});

test("gives nodes ids that stay unique among siblings", () => {
  const tree = answer();
  assignIds(tree.roots);
  const gen = tree.roots[0].children[0].children[0];
  assert.equal(gen.id, "/entity:tb_uart/instance:dut/generate:gen");
  assert.deepEqual(
    gen.children.map((n) => n.id),
    [`${gen.id}/instance:u_fifo`, `${gen.id}/instance:u_fifo#2`],
  );
});

test("says why the tree is empty", async () => {
  const messages = [];
  let provider = new HierarchyProvider(() => undefined);
  provider.onMessage = (m) => messages.push(m);
  assert.deepEqual(await provider.getChildren(), []);
  assert.deepEqual(messages, ["The vfl language server is not running."]);

  provider = new HierarchyProvider(() => fakeClient({ roots: [], entities: [] }));
  await provider.getChildren();
  assert.equal(provider.message, "No entity in this project.");

  provider = new HierarchyProvider(() =>
    fakeClient({ roots: [], entities: [], message: "no entity `x` in the project" }),
  );
  await provider.getChildren();
  assert.equal(provider.message, "no entity `x` in the project");

  provider = new HierarchyProvider(() => fakeClient(new Error("internal error")));
  assert.deepEqual(await provider.getChildren(), []);
  assert.equal(provider.message, "vfl: internal error");
});

test("lists automatic, then the tops, then the other entities to pick a top from", () => {
  const items = topItems(answer().entities, "work.fifo");
  assert.deepEqual(
    items.map((i) => [i.label, i.description, i.top]),
    [
      ["Automatic", undefined, undefined],
      ["tb_uart", "work · top · testbench", "work.tb_uart"],
      ["fifo", "work · current", "work.fifo"],
    ],
  );
  assert.equal(items[2].detail, "rtl/fifo.vhd");
  assert.equal(topItems([], undefined)[0].description, "current");
});

test("refreshing asks the view for the whole tree again", () => {
  const provider = new HierarchyProvider(() => undefined);
  const fired = [];
  provider.onDidChangeTreeData((e) => fired.push(e));
  provider.refresh();
  assert.deepEqual(fired, [undefined]);
});
