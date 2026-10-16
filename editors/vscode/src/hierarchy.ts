// The "VHDL Hierarchy" view in the Explorer: the instantiation tree of the
// project, from the `vfl/hierarchy` request of the language server.
//
// The server builds the tree from its analysis only when asked: when the
// view is shown, after a save, and after the diagnostics change (that is,
// once the server has analyzed an edit), debounced and only while the view
// is visible. The top is `vfl.hierarchy.top`, chosen with a quick pick and
// remembered in the workspace settings; without one, the tree starts from
// every entity no other entity instantiates.
//
// The extension hands this file a function giving the language client to
// ask (see `registerHierarchy`), so that it does not depend on how clients
// are started.

import * as vscode from "vscode";

/** An LSP location. */
export interface Place {
  uri: string;
  range: {
    start: { line: number; character: number };
    end: { line: number; character: number };
  };
}

/** A node of the tree, as the server sends it. */
export interface HierarchyNode {
  kind: "entity" | "instance" | "generate" | "block";
  label: string;
  name?: string;
  unitKind?: "entity" | "component" | "configuration" | "module";
  library?: string;
  architecture?: string;
  /** The scheme of a generate statement: `for i in 0 to 3`. */
  detail?: string;
  location?: Place;
  entityLocation?: Place;
  architectureLocation?: Place;
  unresolved?: boolean;
  recursive?: boolean;
  truncated?: boolean;
  testbench?: boolean;
  children: HierarchyNode[];
  /** Set here: the path of labels from the root, for the view's ids. */
  id?: string;
}

/** An entity to choose a top from. */
export interface Candidate {
  library: string;
  name: string;
  path: string;
  /** No other entity instantiates it. */
  top: boolean;
  testbench: boolean;
}

export interface Hierarchy {
  roots: HierarchyNode[];
  entities: Candidate[];
  message?: string;
}

export interface HierarchyParams {
  top?: string;
  testbenches: boolean;
}

/** What the view needs of a language client. */
export interface HierarchyClient {
  sendRequest<R>(method: string, params: unknown): Promise<R>;
}

export const VIEW_ID = "vfl.hierarchy";

/** The settings of the view, for a workspace. */
function settings(): HierarchyParams {
  const config = vscode.workspace.getConfiguration("vfl.hierarchy");
  const top = config.get<string>("top", "").trim();
  return { top: top || undefined, testbenches: config.get<boolean>("testbenches", true) };
}

/** Gives each node an id from the labels above it, unique among its siblings. */
export function assignIds(nodes: HierarchyNode[], parent = ""): void {
  const seen = new Map<string, number>();
  for (const node of nodes) {
    const key = `${node.kind}:${node.label}`;
    const count = (seen.get(key) ?? 0) + 1;
    seen.set(key, count);
    node.id = `${parent}/${key}${count > 1 ? `#${count}` : ""}`;
    assignIds(node.children, node.id);
  }
}

/** `uart(rtl)`, `lib.fifo(rtl)` outside `work`. */
function unitText(node: HierarchyNode): string {
  let text = node.name ?? "";
  if (node.library && node.library !== "work" && node.kind === "instance") {
    text = `${node.library}.${text}`;
  }
  if (node.architecture) {
    text += `(${node.architecture})`;
  }
  return text;
}

/** What a node shows after its label. */
export function description(node: HierarchyNode): string {
  switch (node.kind) {
    case "generate":
      return node.detail ?? "generate";
    case "block":
      return "block";
    case "entity": {
      const parts = node.architecture ? [`(${node.architecture})`] : [];
      if (node.library && node.library !== "work") {
        parts.unshift(node.library);
      }
      if (node.testbench) {
        parts.push("testbench");
      }
      return parts.join(" ");
    }
    case "instance": {
      const parts = [unitText(node)];
      if (node.unitKind === "component") {
        parts.push("component");
      } else if (node.unitKind === "configuration") {
        parts.push("configuration");
      } else if (node.unitKind === "module") {
        parts.push("module");
      }
      if (node.unresolved) {
        parts.push("not found");
      }
      if (node.recursive) {
        parts.push("recursive");
      }
      if (node.truncated) {
        parts.push("not expanded");
      }
      return parts.join(" · ");
    }
  }
}

function icon(node: HierarchyNode): vscode.ThemeIcon {
  if (node.unresolved) {
    return new vscode.ThemeIcon("warning", new vscode.ThemeColor("list.warningForeground"));
  }
  switch (node.kind) {
    case "generate":
      return new vscode.ThemeIcon("symbol-array");
    case "block":
      return new vscode.ThemeIcon("symbol-namespace");
    case "entity":
      return new vscode.ThemeIcon(node.testbench ? "beaker" : "symbol-class");
    case "instance":
      if (node.recursive) {
        return new vscode.ThemeIcon("sync");
      }
      return new vscode.ThemeIcon(
        node.unitKind === "component" ? "symbol-interface" : "symbol-module",
      );
  }
}

/** The context value menus match: `entity` when the node has an entity to open. */
export function contextValue(node: HierarchyNode): string {
  if (node.kind === "generate" || node.kind === "block") {
    return node.kind;
  }
  return node.entityLocation && node.unitKind !== "module" ? "entity" : "unit";
}

export function toRange(place: Place): vscode.Range {
  const { start, end } = place.range;
  return new vscode.Range(start.line, start.character, end.line, end.character);
}

/** The tree of the view. */
export class HierarchyProvider implements vscode.TreeDataProvider<HierarchyNode> {
  private readonly changed = new vscode.EventEmitter<HierarchyNode | undefined>();
  readonly onDidChangeTreeData = this.changed.event;
  /** The last answer, for the top quick pick. */
  last: Hierarchy | undefined;
  /** Why the tree is empty, or what is shown instead of it. */
  message: string | undefined;
  /** Called with `message` after each fetch. */
  onMessage: ((message: string | undefined) => void) | undefined;

  constructor(
    private readonly client: () => HierarchyClient | undefined,
    private readonly params: () => HierarchyParams = settings,
  ) {}

  /** Asks the server again. */
  refresh(): void {
    this.changed.fire(undefined);
  }

  dispose(): void {
    this.changed.dispose();
  }

  async getChildren(node?: HierarchyNode): Promise<HierarchyNode[]> {
    if (node) {
      return node.children;
    }
    const tree = await this.fetch();
    return tree?.roots ?? [];
  }

  /** The hierarchy, or `undefined` with `message` saying why not. */
  async fetch(): Promise<Hierarchy | undefined> {
    const client = this.client();
    let tree: Hierarchy | undefined;
    let message: string | undefined;
    if (!client) {
      message = "The vfl language server is not running.";
    } else {
      try {
        tree = await client.sendRequest<Hierarchy>("vfl/hierarchy", this.params());
        assignIds(tree.roots);
        message = tree.message ?? (tree.roots.length === 0 ? "No entity in this project." : undefined);
        this.last = tree;
      } catch (err) {
        message = `vfl: ${err instanceof Error ? err.message : err}`;
      }
    }
    this.message = message;
    this.onMessage?.(message);
    return tree;
  }

  getTreeItem(node: HierarchyNode): vscode.TreeItem {
    const collapsible =
      node.children.length === 0
        ? vscode.TreeItemCollapsibleState.None
        : node.kind === "entity"
          ? vscode.TreeItemCollapsibleState.Expanded
          : vscode.TreeItemCollapsibleState.Collapsed;
    const item = new vscode.TreeItem(node.label, collapsible);
    item.id = node.id;
    item.description = description(node);
    item.iconPath = icon(node);
    item.contextValue = contextValue(node);
    const target = node.location ?? node.entityLocation;
    if (target) {
      item.command = {
        command: "vscode.open",
        title: "Go to Instance",
        arguments: [vscode.Uri.parse(target.uri), { selection: toRange(target) }],
      };
    }
    const lines = [node.kind === "instance" ? `${node.label}: ${unitText(node)}` : node.label];
    if (node.entityLocation) {
      lines.push(vscode.workspace.asRelativePath(vscode.Uri.parse(node.entityLocation.uri)));
    }
    if (node.unresolved) {
      lines.push(
        node.unitKind === "component"
          ? "No entity of this name in the project."
          : "Not found in the project.",
      );
    }
    item.tooltip = lines.join("\n");
    return item;
  }
}

/** `library.name` of a node's entity. */
function qualified(node: HierarchyNode): string | undefined {
  return node.name ? `${node.library ?? "work"}.${node.name}` : undefined;
}

async function openPlace(place: Place | undefined): Promise<void> {
  if (place) {
    await vscode.window.showTextDocument(vscode.Uri.parse(place.uri), {
      selection: toRange(place),
    });
  }
}

async function setTop(top: string | undefined): Promise<void> {
  await vscode.workspace
    .getConfiguration("vfl.hierarchy")
    .update("top", top ?? "", vscode.ConfigurationTarget.Workspace);
}

/** The top quick pick: automatic, then the tops, then every other entity. */
export function topItems(
  entities: Candidate[],
  current: string | undefined,
): (vscode.QuickPickItem & { top: string | undefined })[] {
  const items: (vscode.QuickPickItem & { top: string | undefined })[] = [
    {
      label: "Automatic",
      description: current ? undefined : "current",
      detail: "Every entity no other entity instantiates",
      top: undefined,
    },
  ];
  const sorted = [...entities].sort((a, b) => Number(b.top) - Number(a.top));
  for (const e of sorted) {
    const top = `${e.library}.${e.name}`;
    const tags = [e.library];
    if (e.top) {
      tags.push("top");
    }
    if (e.testbench) {
      tags.push("testbench");
    }
    if (current?.toLowerCase() === top.toLowerCase() || current?.toLowerCase() === e.name.toLowerCase()) {
      tags.push("current");
    }
    items.push({
      label: e.name,
      description: tags.join(" · "),
      detail: vscode.workspace.asRelativePath(e.path),
      top,
    });
  }
  return items;
}

/**
 * Registers the view and its commands. `client` gives the running language
 * client to ask, or `undefined`.
 */
export function registerHierarchy(client: () => HierarchyClient | undefined): vscode.Disposable {
  const provider = new HierarchyProvider(client);
  const view = vscode.window.createTreeView(VIEW_ID, {
    treeDataProvider: provider,
    showCollapseAll: true,
  });
  provider.onMessage = (message) => {
    view.message = message;
  };
  void vscode.commands.executeCommand("setContext", "vfl.active", true);

  // Asked again only while the view is visible; otherwise once it is shown.
  let stale = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const refreshSoon = (delay: number) => {
    if (!view.visible) {
      stale = true;
      return;
    }
    if (timer) {
      clearTimeout(timer);
    }
    timer = setTimeout(() => {
      timer = undefined;
      stale = false;
      provider.refresh();
    }, delay);
  };
  const hdl = (doc: vscode.TextDocument) =>
    ["vhdl", "verilog", "systemverilog"].includes(doc.languageId);

  return vscode.Disposable.from(
    view,
    provider,
    { dispose: () => timer && clearTimeout(timer) },
    view.onDidChangeVisibility((e) => {
      if (e.visible && stale) {
        refreshSoon(0);
      }
    }),
    vscode.workspace.onDidSaveTextDocument((doc) => {
      if (hdl(doc)) {
        refreshSoon(300);
      }
    }),
    // The server publishes diagnostics once it has analyzed an edit.
    vscode.languages.onDidChangeDiagnostics(() => refreshSoon(1500)),
    vscode.workspace.onDidChangeConfiguration((e) => {
      if (e.affectsConfiguration("vfl.hierarchy")) {
        refreshSoon(0);
      }
    }),
    vscode.commands.registerCommand("vfl.hierarchy.refresh", () => provider.refresh()),
    vscode.commands.registerCommand("vfl.hierarchy.selectTop", async () => {
      const tree = provider.last ?? (await provider.fetch());
      if (!tree) {
        void vscode.window.showErrorMessage(provider.message ?? "vfl: no hierarchy.");
        return;
      }
      const picked = await vscode.window.showQuickPick(topItems(tree.entities, settings().top), {
        placeHolder: "Top of the VHDL hierarchy",
        matchOnDetail: true,
      });
      if (picked) {
        await setTop(picked.top);
      }
    }),
    vscode.commands.registerCommand("vfl.hierarchy.setTop", (node?: HierarchyNode) =>
      node ? setTop(qualified(node)) : undefined,
    ),
    vscode.commands.registerCommand("vfl.hierarchy.openEntity", (node?: HierarchyNode) =>
      openPlace(node?.entityLocation),
    ),
    vscode.commands.registerCommand("vfl.hierarchy.openArchitecture", (node?: HierarchyNode) =>
      openPlace(node?.architectureLocation ?? node?.entityLocation),
    ),
    vscode.commands.registerCommand("vfl.hierarchy.generateTestbench", (node?: HierarchyNode) => {
      const entity = node && qualified(node);
      return entity ? vscode.commands.executeCommand("vfl.generateTestbench", entity) : undefined;
    }),
  );
}
