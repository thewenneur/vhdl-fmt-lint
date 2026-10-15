//! Zed extension for vfl: starts `vfl lsp` for VHDL, Verilog and
//! SystemVerilog files.
//!
//! The server binary is, in order: `lsp.vfl.binary.path` from the Zed
//! settings, `vfl` on the `PATH` of the worktree's shell, or the binary of
//! the latest GitHub release, downloaded into the extension's directory.
//!
//! `lsp.vfl.initialization_options` goes to the server as is: its `config`
//! object holds `vfl.toml` keys for projects without a `vfl.toml`, such as
//! the lint preset and format style. It is also the workspace configuration,
//! so that a change applies without restarting the server.

use std::fs;

use zed_extension_api::{
    self as zed, Architecture, DownloadedFileType, GithubReleaseOptions, LanguageServerId,
    LanguageServerInstallationStatus, Os, Result, Worktree, settings::LspSettings,
};

const REPO: &str = "thewenneur/vhdl-fmt-lint";
const SERVER: &str = "vfl";

struct Vfl {
    /// The downloaded binary, once found in this session.
    downloaded: Option<String>,
}

impl Vfl {
    fn downloaded_binary(&mut self, id: &LanguageServerId) -> Result<String> {
        if let Some(path) = &self.downloaded
            && fs::metadata(path).is_ok_and(|m| m.is_file())
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

        zed::set_language_server_installation_status(
            id,
            &LanguageServerInstallationStatus::CheckingForUpdate,
        );
        let release = match zed::latest_github_release(
            REPO,
            GithubReleaseOptions {
                require_assets: true,
                pre_release: false,
            },
        ) {
            Ok(release) => release,
            // Offline: keep using a binary downloaded earlier.
            Err(err) => return newest_download(os).ok_or(err),
        };

        let version = release.version.trim_start_matches('v');
        let name = format!("vfl-v{version}-{target}");
        let (archive, file_type) = if os == Os::Windows {
            (format!("{name}.zip"), DownloadedFileType::Zip)
        } else {
            (format!("{name}.tar.gz"), DownloadedFileType::GzipTar)
        };
        let binary = format!("{name}/{}", binary_name(os));

        if !fs::metadata(&binary).is_ok_and(|m| m.is_file()) {
            let asset = release
                .assets
                .iter()
                .find(|asset| asset.name == archive)
                .ok_or_else(|| format!("release {} has no asset {archive}", release.version))?;
            zed::set_language_server_installation_status(
                id,
                &LanguageServerInstallationStatus::Downloading,
            );
            // The archive holds a directory named like itself, with the binary in it.
            zed::download_file(&asset.download_url, ".", file_type)
                .map_err(|err| format!("downloading {archive}: {err}"))?;
            zed::make_file_executable(&binary)?;

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
        _ => return None,
    })
}

fn binary_name(os: Os) -> &'static str {
    if os == Os::Windows { "vfl.exe" } else { "vfl" }
}

/// A binary downloaded in an earlier session, if any.
fn newest_download(os: Os) -> Option<String> {
    fs::read_dir(".")
        .ok()?
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("vfl-v"))
        .map(|name| format!("{name}/{}", binary_name(os)))
        .find(|binary| fs::metadata(binary).is_ok_and(|m| m.is_file()))
}

zed::register_extension!(Vfl);
