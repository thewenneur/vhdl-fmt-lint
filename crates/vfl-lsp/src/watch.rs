//! The server's own file watcher, for editors that cannot watch files for
//! it (no dynamic registration of `workspace/didChangeWatchedFiles`).
//!
//! A thread lists the files that matter to the project now and then and
//! compares their modification times and sizes with the last listing. What
//! changed reaches the server as the editor's watcher would send it, as a
//! `workspace/didChangeWatchedFiles` notification. Polling needs no
//! platform watcher (inotify, FSEvents, ReadDirectoryChangesW) nor a
//! dependency for one, and sees every file system, network ones too. A
//! listing is one walk of the project, about what loading it costs
//! before parsing; the thread waits 20 times as long as the last walk
//! took between two, at least two seconds, so it takes 5% of one core at
//! most.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use lsp_server::{Message, Notification};
use lsp_types::notification::Notification as _;
use lsp_types::{FileChangeType, FileEvent, Url};
use rustc_hash::FxHashMap;
use vfl_config::Config;

/// The shortest wait between two walks.
const MIN_INTERVAL: Duration = Duration::from_secs(2);
/// How many times as long as a walk took the next one waits.
const IDLE_FACTOR: u32 = 20;

/// Stops the watcher when dropped.
pub struct Watcher {
    _stop: mpsc::Sender<()>,
}

/// The last-modified time and size of each file.
type Listing = FxHashMap<PathBuf, (Option<SystemTime>, u64)>;

/// Watches the project at `root`, sending what changed to `server`.
/// `defaults` is the config when no config file applies.
///
/// The first listing is taken before this returns: a file changed after
/// it, while the project loads or as soon as it is loaded, is reported.
/// Taken by the thread instead, it could come after such a change, which
/// would then never be seen.
pub fn spawn(root: PathBuf, defaults: Config, server: mpsc::Sender<Message>) -> Watcher {
    let (stop, stopped) = mpsc::channel::<()>();
    let mut listing = list(&root, &defaults);
    std::thread::spawn(move || {
        let mut interval = MIN_INTERVAL;
        // Waits, unless the server stopped.
        while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(interval) {
            let start = Instant::now();
            // A bug in the walk skips this round rather than the watcher.
            let walk = std::panic::AssertUnwindSafe(|| list(&root, &defaults));
            let Ok(now) = std::panic::catch_unwind(walk) else {
                continue;
            };
            interval = MIN_INTERVAL.max(start.elapsed() * IDLE_FACTOR);
            let changes = changes(&listing, &now);
            listing = now;
            if changes.is_empty() {
                continue;
            }
            let params = lsp_types::DidChangeWatchedFilesParams { changes };
            let notification = Notification::new(
                lsp_types::notification::DidChangeWatchedFiles::METHOD.into(),
                params,
            );
            if server.send(Message::Notification(notification)).is_err() {
                return;
            }
        }
    });
    Watcher { _stop: stop }
}

/// The files that matter to the project at `root`, with the config of
/// its root now: an invalid config file is left out, as the server does.
fn list(root: &Path, defaults: &Config) -> Listing {
    let (config, _) = vfl_config::load_for_dir_checked(root, defaults);
    vfl_project::collect_watched(root, &config)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|path| {
            let meta = std::fs::metadata(&path).ok()?;
            Some((path, (meta.modified().ok(), meta.len())))
        })
        .collect()
}

/// What changed from `before` to `now`, as an editor reports it.
fn changes(before: &Listing, now: &Listing) -> Vec<FileEvent> {
    let mut changes: Vec<(&PathBuf, FileChangeType)> = now
        .iter()
        .filter_map(|(path, stamp)| match before.get(path) {
            None => Some((path, FileChangeType::CREATED)),
            Some(old) if old != stamp => Some((path, FileChangeType::CHANGED)),
            Some(_) => None,
        })
        .chain(
            before
                .keys()
                .filter(|path| !now.contains_key(*path))
                .map(|path| (path, FileChangeType::DELETED)),
        )
        .collect();
    changes.sort_by(|a, b| a.0.cmp(b.0));
    changes
        .into_iter()
        .filter_map(|(path, typ)| {
            Some(FileEvent {
                uri: Url::from_file_path(path).ok()?,
                typ,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_are_listed_as_an_editor_sends_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("a.vhd"), "entity a is\nend entity;\n").unwrap();
        std::fs::write(root.join("b.vhd"), "").unwrap();
        let before = list(&root, &Config::default());
        assert_eq!(before.len(), 2);

        std::fs::write(root.join("a.vhd"), "entity a is\nend entity a;\n").unwrap();
        std::fs::remove_file(root.join("b.vhd")).unwrap();
        std::fs::write(root.join("c.vhd"), "").unwrap();
        std::fs::write(root.join("vfl.toml"), "").unwrap();
        std::fs::write(root.join("notes.txt"), "").unwrap();
        let now = list(&root, &Config::default());
        let found: Vec<(String, FileChangeType)> = changes(&before, &now)
            .into_iter()
            .map(|e| {
                let path = e.uri.to_file_path().unwrap();
                (path.file_name().unwrap().to_string_lossy().into(), e.typ)
            })
            .collect();
        assert_eq!(
            found,
            [
                ("a.vhd".into(), FileChangeType::CHANGED),
                ("b.vhd".into(), FileChangeType::DELETED),
                ("c.vhd".into(), FileChangeType::CREATED),
                ("vfl.toml".into(), FileChangeType::CREATED),
            ]
        );
        assert!(changes(&now, &now).is_empty());
    }

    /// A file made as soon as the watcher is started, before its thread
    /// ran, is reported all the same.
    #[test]
    fn a_file_made_right_after_the_start_is_seen() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let (sender, inbox) = mpsc::channel();
        let _watcher = spawn(root.clone(), Config::default(), sender);
        std::fs::write(root.join("a.vhd"), "").unwrap();
        let Ok(Message::Notification(n)) = inbox.recv_timeout(Duration::from_secs(20)) else {
            panic!("the change is reported");
        };
        let params: lsp_types::DidChangeWatchedFilesParams =
            serde_json::from_value(n.params).unwrap();
        assert_eq!(params.changes.len(), 1);
        assert_eq!(params.changes[0].typ, FileChangeType::CREATED);
    }

    #[test]
    fn the_watcher_stops_with_the_server() {
        let dir = tempfile::tempdir().unwrap();
        let (sender, _inbox) = mpsc::channel();
        let watcher = spawn(dir.path().to_owned(), Config::default(), sender);
        drop(watcher);
    }
}
