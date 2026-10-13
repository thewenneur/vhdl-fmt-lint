// Downloads VS Code, opens a copy of the fixture project with the
// extension loaded and runs suite.js inside it, against a real `vfl`.
//
// Usage: node test/e2e/run.js [path/to/vfl]   (default: target/debug/vfl)

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { runTests } = require("@vscode/test-electron");

async function main() {
  const root = path.resolve(__dirname, "..", "..");
  const exe = process.platform === "win32" ? "vfl.exe" : "vfl";
  const vfl = path.resolve(process.argv[2] ?? path.join(root, "..", "..", "target", "debug", exe));
  if (!fs.existsSync(vfl)) {
    throw new Error(`${vfl} does not exist; build it with \`cargo build -p vfl-cli\``);
  }

  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), "vfl-e2e-"));
  fs.cpSync(path.join(root, "test", "fixture"), workspace, { recursive: true });
  fs.mkdirSync(path.join(workspace, ".vscode"));
  fs.writeFileSync(
    path.join(workspace, ".vscode", "settings.json"),
    JSON.stringify({ "vfl.server.path": vfl }),
  );

  try {
    await runTests({
      extensionDevelopmentPath: root,
      extensionTestsPath: path.join(__dirname, "suite.js"),
      launchArgs: [workspace, "--disable-extensions", "--disable-workspace-trust"],
    });
  } finally {
    fs.rmSync(workspace, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
