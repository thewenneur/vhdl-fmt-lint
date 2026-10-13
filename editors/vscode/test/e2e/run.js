// Downloads VS Code, opens a copy of the fixture project with the
// extension loaded and runs suite.js inside it, against a real `vhone`.
//
// Usage: node test/e2e/run.js [path/to/vhone]   (default: target/debug/vhone)

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { runTests } = require("@vscode/test-electron");

async function main() {
  const root = path.resolve(__dirname, "..", "..");
  const exe = process.platform === "win32" ? "vhone.exe" : "vhone";
  const vhone = path.resolve(process.argv[2] ?? path.join(root, "..", "..", "target", "debug", exe));
  if (!fs.existsSync(vhone)) {
    throw new Error(`${vhone} does not exist; build it with \`cargo build -p vhone-cli\``);
  }

  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), "vhone-e2e-"));
  fs.cpSync(path.join(root, "test", "fixture"), workspace, { recursive: true });
  fs.mkdirSync(path.join(workspace, ".vscode"));
  fs.writeFileSync(
    path.join(workspace, ".vscode", "settings.json"),
    JSON.stringify({ "vhone.server.path": vhone }),
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
