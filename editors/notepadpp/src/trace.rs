//! How long the plugin keeps Notepad++ busy, for finding freezes.
//!
//! With `VFL_NPP_TRACE` set to a file path, each call from Notepad++ that
//! takes longer than [`THRESHOLD`] is appended to that file as
//! `label<TAB>microseconds`.

use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const THRESHOLD: Duration = Duration::from_micros(200);

fn file() -> Option<&'static Mutex<File>> {
    static FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();
    FILE.get_or_init(|| {
        let path = std::env::var_os("VFL_NPP_TRACE")?;
        let file = File::options().create(true).append(true).open(path).ok()?;
        Some(Mutex::new(file))
    })
    .as_ref()
}

/// Records how long the call started at `start` took.
pub fn record(label: impl std::fmt::Display, start: Instant) {
    let elapsed = start.elapsed();
    if elapsed < THRESHOLD {
        return;
    }
    if let Some(file) = file()
        && let Ok(mut file) = file.lock()
    {
        let _ = writeln!(file, "{label}\t{}", elapsed.as_micros());
    }
}
