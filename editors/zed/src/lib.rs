//! Zed extension for vfl: starts `vfl lsp` for VHDL, Verilog and
//! SystemVerilog files.
//!
//! The server binary is, in order: `lsp.vfl.binary.path` from the Zed
//! settings, `vfl` on the `PATH` of the worktree's shell, or the binary of
//! the GitHub release of this extension's version, downloaded into the
//! extension's directory once its archive matches the release's
//! `SHA256SUMS`.
//!
//! `lsp.vfl.initialization_options` goes to the server as is: its `config`
//! object holds `vfl.toml` keys for projects without a `vfl.toml`, such as
//! the lint preset and format style. It is also the workspace configuration,
//! so that a change applies without restarting the server.

use std::fs;
use std::io::Read;

use sha2::{Digest, Sha256};
use zed_extension_api::{
    self as zed, Architecture, DownloadedFileType, LanguageServerId,
    LanguageServerInstallationStatus, Os, Result, Worktree, settings::LspSettings,
};

const REPO: &str = "thewenneur/vhdl-fmt-lint";
const SERVER: &str = "vfl";
/// The release whose binary is downloaded: the one the extension comes from.
const VERSION: &str = env!("CARGO_PKG_VERSION");
const CHECKSUMS: &str = "SHA256SUMS";

struct Vfl {
    /// The downloaded binary, once found in this session.
    downloaded: Option<String>,
}

impl Vfl {
    fn downloaded_binary(&mut self, id: &LanguageServerId) -> Result<String> {
        if let Some(path) = &self.downloaded
            && is_file(path)
        {
            return Ok(path.clone());
        }

        let (os, arch) = zed::current_platform();
        let target = release_target(os, arch).ok_or_else(|| {
            "vfl has no prebuilt binary for this platform: install it \
             (cargo install --git https://github.com/thewenneur/vhdl-fmt-lint vfl-cli) \
             and put it on the PATH, or set lsp.vfl.binary.path"
                .to_string()
        })?;
        let name = format!("vfl-v{VERSION}-{target}");
        let binary = format!("{name}/{}", binary_name(os));

        if !is_file(&binary) {
            zed::set_language_server_installation_status(
                id,
                &LanguageServerInstallationStatus::Downloading,
            );
            if let Err(err) = download(&name, &binary, os) {
                // Offline, say: keep using a binary downloaded earlier.
                if let Some(earlier) = newest_download(os) {
                    zed::set_language_server_installation_status(
                        id,
                        &LanguageServerInstallationStatus::None,
                    );
                    return Ok(earlier);
                }
                zed::set_language_server_installation_status(
                    id,
                    &LanguageServerInstallationStatus::Failed(err.clone()),
                );
                return Err(err);
            }

            // Older downloads are no longer needed.
            for entry in fs::read_dir(".").into_iter().flatten().flatten() {
                let file_name = entry.file_name();
                let file_name = file_name.to_string_lossy();
                if file_name.starts_with("vfl-v") && file_name != name {
                    fs::remove_dir_all(entry.path()).ok();
                }
            }
        }

        zed::set_language_server_installation_status(id, &LanguageServerInstallationStatus::None);
        self.downloaded = Some(binary.clone());
        Ok(binary)
    }
}

/// Downloads the release archive `name` and the release's checksums, checks
/// the one against the other, and writes the binary in the archive to
/// `binary`. Only the binary stays on disk.
fn download(name: &str, binary: &str, os: Os) -> Result<()> {
    let tag = format!("v{VERSION}");
    let release = zed::github_release_by_tag_name(REPO, &tag)?;
    let url = |file: &str| {
        release
            .assets
            .iter()
            .find(|asset| asset.name == file)
            .map(|asset| asset.download_url.clone())
            .ok_or_else(|| format!("release {tag} has no asset {file}"))
    };
    let archive = if os == Os::Windows {
        format!("{name}.zip")
    } else {
        format!("{name}.tar.gz")
    };
    let (archive_url, checksums_url) = (url(&archive)?, url(CHECKSUMS)?);

    // Outside the directory of the binary, so an interrupted download leaves
    // no binary behind.
    let checksums_file = format!("{name}.{CHECKSUMS}");
    let result = (|| {
        zed::download_file(
            &checksums_url,
            &checksums_file,
            DownloadedFileType::Uncompressed,
        )
        .map_err(|err| format!("downloading {CHECKSUMS}: {err}"))?;
        zed::download_file(&archive_url, &archive, DownloadedFileType::Uncompressed)
            .map_err(|err| format!("downloading {archive}: {err}"))?;
        let checksums = fs::read_to_string(&checksums_file).map_err(|err| err.to_string())?;
        let bytes = fs::read(&archive).map_err(|err| err.to_string())?;
        verify(&checksums, &archive, &bytes)?;
        let content = if os == Os::Windows {
            unzip(&bytes, binary)?
        } else {
            untar(&bytes, binary)?
        };
        fs::create_dir_all(name).map_err(|err| err.to_string())?;
        fs::write(binary, content).map_err(|err| err.to_string())?;
        zed::make_file_executable(binary)
    })();
    fs::remove_file(&checksums_file).ok();
    fs::remove_file(&archive).ok();
    result
}

/// Checks `bytes`, the content of `file`, against `checksums`, a
/// `SHA256SUMS` file as written by `sha256sum`.
fn verify(checksums: &str, file: &str, bytes: &[u8]) -> Result<()> {
    let expected = checksums
        .lines()
        .find_map(|line| {
            let (sum, name) = line.trim().split_once(char::is_whitespace)?;
            // `sha256sum` marks files read in binary mode with `*`.
            (name.trim_start().trim_start_matches('*') == file).then_some(sum)
        })
        .ok_or_else(|| format!("{CHECKSUMS} has no line for {file}"))?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "{file} does not match {CHECKSUMS}: its SHA-256 is {actual}, not {expected}"
        ))
    }
}

/// The file at `path` in a `.tar.gz` archive.
fn untar(archive: &[u8], path: &str) -> Result<Vec<u8>> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries().map_err(|err| err.to_string())? {
        let mut entry = entry.map_err(|err| err.to_string())?;
        if entry.path().is_ok_and(|p| p.to_string_lossy() == path) {
            let mut content = Vec::new();
            entry
                .read_to_end(&mut content)
                .map_err(|err| err.to_string())?;
            return Ok(content);
        }
    }
    Err(format!("the archive has no {path}"))
}

/// The file at `path` in a `.zip` archive, stored or deflated as `7z`
/// writes it.
fn unzip(archive: &[u8], path: &str) -> Result<Vec<u8>> {
    let corrupt = || "the zip archive is corrupt".to_string();
    let u16_at = |at: usize| {
        archive
            .get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
            .ok_or_else(corrupt)
    };
    let u32_at = |at: usize| {
        archive
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
            .ok_or_else(corrupt)
    };
    // The end of central directory record; only a comment follows it.
    let end = (0..archive.len().saturating_sub(21))
        .rev()
        .find(|&at| archive[at..].starts_with(b"PK\x05\x06"))
        .ok_or_else(corrupt)?;
    let mut at = u32_at(end + 16)?;
    for _ in 0..u16_at(end + 10)? {
        if !archive[at.min(archive.len())..].starts_with(b"PK\x01\x02") {
            return Err(corrupt());
        }
        let method = u16_at(at + 10)?;
        let size = u32_at(at + 20)?;
        let name_len = u16_at(at + 28)?;
        let local = u32_at(at + 42)?;
        let name = archive
            .get(at + 46..at + 46 + name_len)
            .ok_or_else(corrupt)?;
        at += 46 + name_len + u16_at(at + 30)? + u16_at(at + 32)?;
        if String::from_utf8_lossy(name).replace('\\', "/") != path {
            continue;
        }
        let start = local + 30 + u16_at(local + 26)? + u16_at(local + 28)?;
        let data = archive.get(start..start + size).ok_or_else(corrupt)?;
        return match method {
            0 => Ok(data.to_vec()),
            8 => {
                let mut content = Vec::new();
                flate2::read::DeflateDecoder::new(data)
                    .read_to_end(&mut content)
                    .map_err(|err| err.to_string())?;
                Ok(content)
            }
            _ => Err(format!("{path} is compressed with method {method}")),
        };
    }
    Err(format!("the archive has no {path}"))
}

impl zed::Extension for Vfl {
    fn new() -> Self {
        Vfl { downloaded: None }
    }

    fn language_server_command(
        &mut self,
        id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<zed::Command> {
        let binary = LspSettings::for_worktree(SERVER, worktree)
            .ok()
            .and_then(|settings| settings.binary);
        let args = binary
            .as_ref()
            .and_then(|binary| binary.arguments.clone())
            .unwrap_or_else(|| vec!["lsp".into()]);
        let env = binary
            .as_ref()
            .and_then(|binary| binary.env.clone())
            .map(|env| env.into_iter().collect())
            .unwrap_or_default();

        let command = match binary.and_then(|binary| binary.path) {
            Some(path) => path,
            None => match worktree.which(SERVER) {
                Some(path) => path,
                None => self.downloaded_binary(id)?,
            },
        };
        Ok(zed::Command { command, args, env })
    }

    fn language_server_initialization_options(
        &mut self,
        _id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(editor_settings(worktree))
    }

    fn language_server_workspace_configuration(
        &mut self,
        _id: &LanguageServerId,
        worktree: &Worktree,
    ) -> Result<Option<zed::serde_json::Value>> {
        Ok(editor_settings(worktree))
    }
}

/// `lsp.vfl.initialization_options` from the Zed settings.
fn editor_settings(worktree: &Worktree) -> Option<zed::serde_json::Value> {
    LspSettings::for_worktree(SERVER, worktree)
        .ok()
        .and_then(|settings| settings.initialization_options)
}

/// The target triple of the release archive for this platform.
fn release_target(os: Os, arch: Architecture) -> Option<&'static str> {
    Some(match (os, arch) {
        (Os::Linux, Architecture::X8664) => "x86_64-unknown-linux-musl",
        (Os::Linux, Architecture::Aarch64) => "aarch64-unknown-linux-musl",
        (Os::Mac, Architecture::X8664) => "x86_64-apple-darwin",
        (Os::Mac, Architecture::Aarch64) => "aarch64-apple-darwin",
        (Os::Windows, Architecture::X8664) => "x86_64-pc-windows-msvc",
        (Os::Windows, Architecture::Aarch64) => "aarch64-pc-windows-msvc",
        _ => return None,
    })
}

fn binary_name(os: Os) -> &'static str {
    if os == Os::Windows { "vfl.exe" } else { "vfl" }
}

fn is_file(path: &str) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file())
}

/// A binary downloaded in an earlier session, if any.
fn newest_download(os: Os) -> Option<String> {
    fs::read_dir(".")
        .ok()?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("vfl-v"))
        .map(|name| format!("{name}/{}", binary_name(os)))
        .find(|binary| is_file(binary))
}

zed::register_extension!(Vfl);

#[cfg(test)]
mod tests {
    use super::*;

    const BINARY: &str = "vfl-v1.2.3-x/vfl";

    fn sums(bytes: &[u8], file: &str) -> String {
        format!("0000  other.zip\n{:x}  {file}\n", Sha256::digest(bytes))
    }

    #[test]
    fn verify_checks_the_line_of_the_file() {
        let bytes = b"archive";
        assert!(verify(&sums(bytes, "a.tar.gz"), "a.tar.gz", bytes).is_ok());
        assert!(verify(&sums(bytes, "a.tar.gz"), "a.tar.gz", b"tampered").is_err());
        assert!(verify(&sums(bytes, "a.tar.gz"), "b.tar.gz", bytes).is_err());
        let starred = sums(bytes, "*a.tar.gz").replace("  *", " *");
        assert!(verify(&starred, "a.tar.gz", bytes).is_ok());
    }

    #[test]
    fn untar_finds_the_binary() {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, content) in [("vfl-v1.2.3-x/README.md", &b"readme"[..]), (BINARY, b"elf")] {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_cksum();
            tar.append_data(&mut header, path, content).unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, &tar.into_inner().unwrap()).unwrap();
        let archive = gz.finish().unwrap();
        assert_eq!(untar(&archive, BINARY).unwrap(), b"elf");
        assert!(untar(&archive, "vfl-v1.2.3-x/nothing").is_err());
    }

    /// A zip archive of `files`, the first stored and the others deflated.
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (i, (name, content)) in files.iter().enumerate() {
            let (method, data) = if i == 0 {
                (0u16, content.to_vec())
            } else {
                let mut deflate =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                std::io::Write::write_all(&mut deflate, content).unwrap();
                (8, deflate.finish().unwrap())
            };
            let offset = out.len() as u32;
            out.extend(b"PK\x03\x04");
            out.extend([20, 0, 0, 0]);
            out.extend(method.to_le_bytes());
            out.extend([0; 8]);
            out.extend((data.len() as u32).to_le_bytes());
            out.extend((content.len() as u32).to_le_bytes());
            out.extend((name.len() as u16).to_le_bytes());
            out.extend([0, 0]);
            out.extend(name.as_bytes());
            out.extend(&data);

            central.extend(b"PK\x01\x02");
            central.extend([20, 0, 20, 0, 0, 0]);
            central.extend(method.to_le_bytes());
            central.extend([0; 8]);
            central.extend((data.len() as u32).to_le_bytes());
            central.extend((content.len() as u32).to_le_bytes());
            central.extend((name.len() as u16).to_le_bytes());
            central.extend([0; 12]);
            central.extend(offset.to_le_bytes());
            central.extend(name.as_bytes());
        }
        let start = out.len() as u32;
        out.extend(&central);
        out.extend(b"PK\x05\x06");
        out.extend([0; 4]);
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((files.len() as u16).to_le_bytes());
        out.extend((central.len() as u32).to_le_bytes());
        out.extend(start.to_le_bytes());
        out.extend([0, 0]);
        out
    }

    #[test]
    fn unzip_finds_the_binary() {
        let archive = zip(&[
            ("vfl-v1.2.3-x/README.md", b"readme"),
            ("vfl-v1.2.3-x\\vfl.exe", b"MZ binary MZ binary MZ binary"),
        ]);
        assert_eq!(
            unzip(&archive, "vfl-v1.2.3-x/vfl.exe").unwrap(),
            b"MZ binary MZ binary MZ binary"
        );
        assert_eq!(
            unzip(&archive, "vfl-v1.2.3-x/README.md").unwrap(),
            b"readme"
        );
        assert!(unzip(&archive, "vfl-v1.2.3-x/nothing").is_err());
        assert!(unzip(&archive[..archive.len() / 2], "vfl-v1.2.3-x/vfl.exe").is_err());
    }
}
