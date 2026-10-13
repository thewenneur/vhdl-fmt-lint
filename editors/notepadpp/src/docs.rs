//! Documentation and diagrams of the project around a file, written as
//! `vhone doc` writes them. Only run by the menu commands, never as you type.

use std::path::{Path, PathBuf};

use vhone_config::DocFormat;
use vhone_project::{Project, find_root};

/// Where the documentation went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The index page.
    pub index: PathBuf,
    /// The page of the first entity (else package) of the file, else the index.
    pub page: PathBuf,
    /// Whether the pages are HTML.
    pub html: bool,
}

/// Writes the documentation of the project around `file`, which is read
/// from disk with its `vhone.toml` like `vhone doc` does. With `out`, as HTML
/// into `out`; else as `[doc]` says.
pub fn write(file: &Path, out: Option<&Path>) -> Result<Written, String> {
    let file = std::path::absolute(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let project = Project::load(find_root(&file)).map_err(|e| e.to_string())?;
    let mut config = project.config.doc.clone();
    if out.is_some() {
        config.format = DocFormat::Html;
    }
    let design = vhone_doc::load(&project, &config)
        .map_err(|e| format!("invalid pattern in doc.exclude: {e}"))?;
    let dir = match out {
        Some(out) => out.to_owned(),
        None => project.root.dir.join(&config.out_dir),
    };
    let files = vhone_doc::write(&design, &config, &dir)
        .map_err(|e| format!("could not write {}: {e}", dir.display()))?;
    let extension = match config.format {
        DocFormat::Html => "html",
        DocFormat::Markdown => "md",
        DocFormat::Asciidoc => "adoc",
    };
    let index = dir.join(format!("index.{extension}"));
    let page = vhone_doc::page_for(&design, &project.root.dir, &file)
        .map(|id| format!("{id}.{extension}"))
        .filter(|name| files.iter().any(|f| f.path == *name))
        .map_or_else(|| index.clone(), |name| dir.join(name));
    Ok(Written {
        index,
        page,
        html: config.format == DocFormat::Html,
    })
}

/// Where Show diagrams writes its pages.
pub fn diagrams_dir() -> PathBuf {
    std::env::temp_dir().join("vhone_npp").join("diagrams")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    const FILES: &[(&str, &str)] = &[
        (
            "vhone.toml",
            "[doc]\nformat = \"markdown\"\nout_dir = \"site\"\n",
        ),
        (
            "src/top.vhd",
            "entity top is\n  port (a : in bit);\nend;\narchitecture rtl of top is\nbegin\nend;\n\
             entity other is\nend;\n",
        ),
        ("src/pkg.vhd", "package util is\nend package;\n"),
        ("src/empty.vhd", "-- nothing\n"),
    ];

    #[test]
    fn writes_the_configured_documentation() {
        let dir = project(FILES);
        let written = write(&dir.path().join("src/top.vhd"), None).unwrap();
        let site = dir.path().join("site");
        assert_eq!(written.index, site.join("index.md"));
        assert_eq!(written.page, site.join("work.top.md"));
        assert!(!written.html);
        assert!(written.index.exists() && written.page.exists());
    }

    #[test]
    fn writes_html_pages_elsewhere_for_the_diagrams() {
        let dir = project(FILES);
        let out = tempfile::tempdir().unwrap();
        let written = write(&dir.path().join("src/pkg.vhd"), Some(out.path())).unwrap();
        assert!(written.html);
        assert_eq!(written.page, out.path().join("work.util.html"));
        // The entity pages carry their diagrams inline.
        let top = std::fs::read_to_string(out.path().join("work.top.html")).unwrap();
        assert!(top.contains("<svg"), "{top}");
        assert!(!dir.path().join("site").exists());

        // A file that declares nothing gives the index.
        let written = write(&dir.path().join("src/empty.vhd"), Some(out.path())).unwrap();
        assert_eq!(written.page, out.path().join("index.html"));
    }
}
