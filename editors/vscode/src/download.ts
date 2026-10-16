// Downloads the `vfl` binary of a GitHub release, for when none is
// installed: the archive for this platform, checked against the release's
// SHA256SUMS before the binary is taken out of it. Only Node's own modules,
// so that it can be tested without VS Code.

import * as crypto from "node:crypto";
import * as fs from "node:fs";
import * as https from "node:https";
import * as path from "node:path";
import * as zlib from "node:zlib";

const RELEASES = "https://github.com/thewenneur/vhdl-fmt-lint/releases/download";

/** The target triple of the release archive for a platform, if there is one. */
export function releaseTarget(platform: string, arch: string): string | undefined {
  const targets: Record<string, string> = {
    "linux-x64": "x86_64-unknown-linux-musl",
    "linux-arm64": "aarch64-unknown-linux-musl",
    "darwin-x64": "x86_64-apple-darwin",
    "darwin-arm64": "aarch64-apple-darwin",
    "win32-x64": "x86_64-pc-windows-msvc",
    "win32-arm64": "aarch64-pc-windows-msvc",
  };
  return targets[`${platform}-${arch}`];
}

export interface Release {
  /** The archive's name, which is also the name of the directory in it. */
  name: string;
  archive: string;
  /** The binary's path in the archive, and under the download directory. */
  binary: string;
}

export function release(version: string, target: string): Release {
  const name = `vfl-v${version}-${target}`;
  const windows = target.includes("windows");
  return {
    name,
    archive: `${name}${windows ? ".zip" : ".tar.gz"}`,
    binary: `${name}/${windows ? "vfl.exe" : "vfl"}`,
  };
}

/** The SHA-256 that `checksums`, as written by `sha256sum`, gives `file`. */
export function checksumOf(checksums: string, file: string): string | undefined {
  for (const line of checksums.split(/\r?\n/)) {
    const m = /^([0-9a-fA-F]{64})\s+\*?(.+?)\s*$/.exec(line);
    if (m && m[2] === file) {
      return m[1].toLowerCase();
    }
  }
  return undefined;
}

export function sha256(data: Buffer): string {
  return crypto.createHash("sha256").update(data).digest("hex");
}

/** The file at `entry` in a `.tar.gz` archive. */
export function untar(archive: Buffer, entry: string): Buffer | undefined {
  const tar = zlib.gunzipSync(archive);
  let longName: string | undefined;
  for (let at = 0; at + 512 <= tar.length; ) {
    const header = tar.subarray(at, at + 512);
    if (header.every((b) => b === 0)) {
      break;
    }
    const field = (start: number, len: number) =>
      header.toString("utf8", start, start + len).replace(/\0.*$/s, "");
    const size = parseInt(field(124, 12).trim() || "0", 8);
    const type = String.fromCharCode(header[156]);
    const prefix = field(345, 155);
    const name = longName ?? (prefix ? `${prefix}/${field(0, 100)}` : field(0, 100));
    const data = tar.subarray(at + 512, at + 512 + size);
    longName = undefined;
    if (type === "L") {
      // A GNU long name: the name of the next entry.
      longName = data.toString("utf8").replace(/\0.*$/s, "");
    } else if ((type === "0" || type === "\0") && name.replace(/^\.\//, "") === entry) {
      return Buffer.from(data);
    }
    at += 512 + Math.ceil(size / 512) * 512;
  }
  return undefined;
}

/** The file at `entry` in a `.zip` archive, stored or deflated. */
export function unzip(archive: Buffer, entry: string): Buffer | undefined {
  let end = -1;
  for (let at = archive.length - 22; at >= 0; at--) {
    if (archive.readUInt32LE(at) === 0x06054b50) {
      end = at;
      break;
    }
  }
  if (end < 0) {
    throw new Error("the zip archive is corrupt");
  }
  const count = archive.readUInt16LE(end + 10);
  let at = archive.readUInt32LE(end + 16);
  for (let i = 0; i < count; i++) {
    if (archive.readUInt32LE(at) !== 0x02014b50) {
      throw new Error("the zip archive is corrupt");
    }
    const method = archive.readUInt16LE(at + 10);
    const size = archive.readUInt32LE(at + 20);
    const nameLen = archive.readUInt16LE(at + 28);
    const local = archive.readUInt32LE(at + 42);
    const name = archive.toString("utf8", at + 46, at + 46 + nameLen).replace(/\\/g, "/");
    at += 46 + nameLen + archive.readUInt16LE(at + 30) + archive.readUInt16LE(at + 32);
    if (name !== entry) {
      continue;
    }
    const start = local + 30 + archive.readUInt16LE(local + 26) + archive.readUInt16LE(local + 28);
    const data = archive.subarray(start, start + size);
    if (method === 0) {
      return Buffer.from(data);
    }
    if (method === 8) {
      return zlib.inflateRawSync(data);
    }
    throw new Error(`${entry} is compressed with method ${method}`);
  }
  return undefined;
}

/** GETs `url`, following redirects (release assets are served from elsewhere). */
export function fetch(url: string, redirects = 5): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const request = https.get(url, { headers: { "User-Agent": "vfl-vscode" } }, (res) => {
      const status = res.statusCode ?? 0;
      if (status >= 300 && status < 400 && res.headers.location && redirects > 0) {
        res.resume();
        resolve(fetch(new URL(res.headers.location, url).toString(), redirects - 1));
        return;
      }
      if (status !== 200) {
        res.resume();
        reject(new Error(`${url}: HTTP ${status}`));
        return;
      }
      const chunks: Buffer[] = [];
      res.on("data", (chunk: Buffer) => chunks.push(chunk));
      res.on("end", () => resolve(Buffer.concat(chunks)));
      res.on("error", reject);
    });
    request.on("error", reject);
    request.setTimeout(60_000, () => request.destroy(new Error(`${url}: timed out`)));
  });
}

/**
 * Downloads the binary of release `version` for `target` into `dir`, unless
 * it is there already, and returns its path. Other versions there are
 * removed.
 */
export async function downloadServer(
  dir: string,
  version: string,
  target: string,
  get: (url: string) => Promise<Buffer> = fetch,
): Promise<string> {
  const rel = release(version, target);
  const binary = path.join(dir, rel.binary);
  if (isFile(binary)) {
    return binary;
  }
  const base = `${RELEASES}/v${version}`;
  const [checksums, archive] = await Promise.all([
    get(`${base}/SHA256SUMS`),
    get(`${base}/${rel.archive}`),
  ]);
  const expected = checksumOf(checksums.toString("utf8"), rel.archive);
  if (!expected) {
    throw new Error(`the SHA256SUMS of v${version} has no line for ${rel.archive}`);
  }
  const actual = sha256(archive);
  if (actual !== expected) {
    throw new Error(`${rel.archive} does not match SHA256SUMS: its SHA-256 is ${actual}, not ${expected}`);
  }
  const content = rel.archive.endsWith(".zip") ? unzip(archive, rel.binary) : untar(archive, rel.binary);
  if (!content) {
    throw new Error(`${rel.archive} has no ${rel.binary}`);
  }
  fs.mkdirSync(path.dirname(binary), { recursive: true });
  // Written under another name first, so that no half-written binary is ever run.
  const partial = `${binary}.partial`;
  fs.writeFileSync(partial, content, { mode: 0o755 });
  fs.renameSync(partial, binary);
  for (const entry of fs.readdirSync(dir)) {
    if (entry.startsWith("vfl-v") && entry !== rel.name) {
      fs.rmSync(path.join(dir, entry), { recursive: true, force: true });
    }
  }
  return binary;
}

/** The binary of release `version` in `dir`, if it was downloaded. */
export function downloadedServer(dir: string, version: string, target: string): string | undefined {
  const binary = path.join(dir, release(version, target).binary);
  return isFile(binary) ? binary : undefined;
}

function isFile(file: string): boolean {
  try {
    return fs.statSync(file).isFile();
  } catch {
    return false;
  }
}
