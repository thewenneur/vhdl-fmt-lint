// Checks how a page of `vfl doc` is prepared for the diagrams webview.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const Module = require("node:module");
const path = require("node:path");
const test = require("node:test");
const esbuild = require("esbuild");

/** src/docview.ts, compiled on the fly. */
function load() {
  const file = path.join(__dirname, "..", "src", "docview.ts");
  const { code } = esbuild.transformSync(fs.readFileSync(file, "utf8"), {
    loader: "ts",
    format: "cjs",
  });
  const m = new Module(file);
  m._compile(code, file);
  return m.exports;
}

const { webviewHtml, pageTitle } = load();

const PAGE = `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<style>:root{color-scheme:light dark;--bg:#fff}@media (prefers-color-scheme:dark){:root{--bg:#000}}</style>
</head>
<body>
<a href="index.html">Home</a>
<a href="work.uart.html#arch-rtl">uart</a>
<a href="work.fifo.html#ports">self</a>
<a href="https://example.com/x.html">out</a>
<svg><style>@media (prefers-color-scheme:dark){text{fill:#fff}}</style><a href="work.uart_rx.html"><text>rx</text></a></svg>
</body>
</html>`;

test("adds a content security policy without scripts", () => {
  const html = webviewHtml(PAGE, "work.fifo.html", false);
  assert.match(
    html,
    /<head>\n<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data:;">/,
  );
});

test("follows the VS Code theme instead of the OS one", () => {
  const dark = webviewHtml(PAGE, "work.fifo.html", true);
  assert.ok(!dark.includes("prefers-color-scheme"));
  assert.match(dark, /:root\{color-scheme:dark;--bg:#fff\}@media all\{:root\{--bg:#000\}\}/);
  assert.match(dark, /<style>@media all\{text/);
  const light = webviewHtml(PAGE, "work.fifo.html", false);
  assert.match(light, /:root\{color-scheme:light;--bg:#fff\}@media not all\{/);
});

test("turns links to other pages into command links", () => {
  const html = webviewHtml(PAGE, "work.fifo.html", false);
  const link = (page) =>
    `href="command:vfl.openDocPage?${encodeURIComponent(JSON.stringify([page]))}"`;
  assert.ok(html.includes(`<a ${link("index.html")}>Home</a>`), html);
  assert.ok(html.includes(`<a ${link("work.uart.html")}>uart</a>`), html);
  assert.ok(html.includes(`<a ${link("work.uart_rx.html")}><text>`), html);
  // Anchors of the page itself stay, external links are left alone.
  assert.ok(html.includes('<a href="#ports">self</a>'), html);
  assert.ok(html.includes('<a href="https://example.com/x.html">out</a>'), html);
});

test("titles a page with the name of its unit", () => {
  assert.equal(pageTitle("work.uart.html"), "uart");
  assert.equal(pageTitle("lib_a.my.pkg.html"), "my.pkg");
  assert.equal(pageTitle("index.html"), "Documentation");
});
