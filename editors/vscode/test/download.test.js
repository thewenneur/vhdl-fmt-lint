// Checks the download of the server binary: the release names, the checksum
// check and the extraction from the release archives, without the network.

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const Module = require("node:module");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const zlib = require("node:zlib");
const esbuild = require("esbuild");

/** src/download.ts, compiled on the fly. */
function load() {
  const file = path.join(__dirname, "..", "src", "download.ts");
  const { code } = esbuild.transformSync(fs.readFileSync(file, "utf8"), {
    loader: "ts",
    format: "cjs",
  });
  const m = new Module(file);
  m.paths = Module._nodeModulePaths(path.dirname(file));
  m._compile(code, file);
  return m.exports;
}

const dl = load();

/** A ustar archive of `files`, as `tar` writes it. */
function tar(files) {
  const blocks = [];
  for (const [name, content] of files) {
    const header = Buffer.alloc(512);
    header.write(name, 0);
    header.write("0000755\0", 100);
    header.write("0000000\0", 108);
    header.write("0000000\0", 116);
    header.write(`${content.length.toString(8).padStart(11, "0")}\0`, 124);
    header.write("00000000000\0", 136);
    header.write("        ", 148);
    header.write("0", 156);
    header.write("ustar\0", 257);
    header.write("00", 263);
    const sum = header.reduce((a, b) => a + b, 0);
    header.write(`${sum.toString(8).padStart(6, "0")}\0 `, 148);
    blocks.push(header, content, Buffer.alloc((512 - (content.length % 512)) % 512));
  }
  blocks.push(Buffer.alloc(1024));
  return zlib.gzipSync(Buffer.concat(blocks));
}

/** A zip archive of `files`, the first stored and the others deflated. */
function zip(files) {
  const parts = [];
  const central = [];
  let offset = 0;
  files.forEach(([name, content], i) => {
    const method = i === 0 ? 0 : 8;
    const data = method === 0 ? content : zlib.deflateRawSync(content);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(method, 8);
    local.writeUInt32LE(data.length, 18);
    local.writeUInt32LE(content.length, 22);
    local.writeUInt16LE(name.length, 26);
    const entry = Buffer.alloc(46);
    entry.writeUInt32LE(0x02014b50, 0);
    entry.writeUInt16LE(method, 10);
    entry.writeUInt32LE(data.length, 20);
    entry.writeUInt32LE(content.length, 24);
    entry.writeUInt16LE(name.length, 28);
    entry.writeUInt32LE(offset, 42);
    parts.push(local, Buffer.from(name), data);
    central.push(entry, Buffer.from(name));
    offset += 30 + name.length + data.length;
  });
  const dir = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(files.length, 8);
  end.writeUInt16LE(files.length, 10);
  end.writeUInt32LE(dir.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...parts, dir, end]);
}

const sha = (data) => crypto.createHash("sha256").update(data).digest("hex");

test("every release target of the CLI has a platform", () => {
  const workflow = fs.readFileSync(
    path.join(__dirname, "..", "..", "..", ".github", "workflows", "release.yml"),
    "utf8",
  );
  const cli = workflow.slice(workflow.indexOf("\n  cli:"), workflow.indexOf("\n  notepadpp:"));
  const released = [...cli.matchAll(/target: ([\w-]+)/g)].map((m) => m[1]).sort();
  const mapped = ["linux", "darwin", "win32"]
    .flatMap((p) => ["x64", "arm64"].map((a) => dl.releaseTarget(p, a)))
    .filter(Boolean)
    .sort();
  assert.deepEqual(mapped, released);
  assert.equal(dl.releaseTarget("win32", "ia32"), undefined);
});

test("release names follow the release workflow", () => {
  assert.deepEqual(dl.release("1.2.3", "x86_64-unknown-linux-musl"), {
    name: "vfl-v1.2.3-x86_64-unknown-linux-musl",
    archive: "vfl-v1.2.3-x86_64-unknown-linux-musl.tar.gz",
    binary: "vfl-v1.2.3-x86_64-unknown-linux-musl/vfl",
  });
  assert.equal(dl.release("1.2.3", "x86_64-pc-windows-msvc").binary, "vfl-v1.2.3-x86_64-pc-windows-msvc/vfl.exe");
});

test("checksums are read as sha256sum writes them", () => {
  const a = "a".repeat(64);
  const b = "B".repeat(64);
  const sums = `${a}  one.zip\n${b} *two.tar.gz\r\n`;
  assert.equal(dl.checksumOf(sums, "one.zip"), a);
  assert.equal(dl.checksumOf(sums, "two.tar.gz"), b.toLowerCase());
  assert.equal(dl.checksumOf(sums, "three.zip"), undefined);
});

test("the binary comes out of a tar.gz and a zip", () => {
  const binary = Buffer.from("\x7fELF binary ".repeat(100));
  const files = [
    ["vfl-v1-x/README.md", Buffer.from("readme")],
    ["vfl-v1-x/vfl", binary],
  ];
  assert.deepEqual(dl.untar(tar(files), "vfl-v1-x/vfl"), binary);
  assert.equal(dl.untar(tar(files), "vfl-v1-x/nothing"), undefined);
  assert.deepEqual(dl.unzip(zip(files), "vfl-v1-x/vfl"), binary);
  assert.deepEqual(dl.unzip(zip(files), "vfl-v1-x/README.md"), Buffer.from("readme"));
  assert.equal(dl.unzip(zip(files), "vfl-v1-x/nothing"), undefined);
});

test("downloadServer checks the archive and keeps only its binary", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "vfl-download-"));
  try {
    const target = "x86_64-unknown-linux-musl";
    const rel = dl.release("1.2.3", target);
    const binary = Buffer.from("binary");
    const archive = tar([[rel.binary, binary]]);
    fs.mkdirSync(path.join(dir, "vfl-v1.2.2-old"));
    const fetched = [];
    const serve = (sums) => async (url) => {
      fetched.push(url);
      return url.endsWith("/SHA256SUMS") ? Buffer.from(sums) : archive;
    };

    await assert.rejects(
      dl.downloadServer(dir, "1.2.3", target, serve(`${"0".repeat(64)}  ${rel.archive}\n`)),
      /does not match SHA256SUMS/,
    );
    await assert.rejects(dl.downloadServer(dir, "1.2.3", target, serve("")), /no line for/);
    assert.equal(dl.downloadedServer(dir, "1.2.3", target), undefined);

    const file = await dl.downloadServer(dir, "1.2.3", target, serve(`${sha(archive)}  ${rel.archive}\n`));
    assert.equal(file, path.join(dir, rel.binary));
    assert.deepEqual(fs.readFileSync(file), binary);
    assert.equal(dl.downloadedServer(dir, "1.2.3", target), file);
    assert.ok(fetched.includes(`https://github.com/thewenneur/vhdl-fmt-lint/releases/download/v1.2.3/${rel.archive}`));
    assert.deepEqual(fs.readdirSync(dir), [rel.name]);
    if (process.platform !== "win32") {
      assert.ok(fs.statSync(file).mode & 0o100, "the binary is executable");
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
