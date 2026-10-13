//! Pages as a small block structure, written out as Markdown or HTML.

use std::fmt::Write;

use crate::svg::escape;

/// A run of inline content.
#[derive(Debug, Clone)]
pub enum Inline {
    Text(String),
    Code(String),
    /// A link to another page (by id), optionally to an anchor in it.
    Link {
        text: String,
        code: bool,
        page: String,
        anchor: Option<String>,
    },
}

pub type Cell = Vec<Inline>;

pub fn text(text: impl Into<String>) -> Inline {
    Inline::Text(text.into())
}

pub fn code(text: impl Into<String>) -> Inline {
    Inline::Code(text.into())
}

#[derive(Debug, Clone)]
pub enum Block {
    Heading {
        level: u8,
        content: Vec<Inline>,
        id: Option<String>,
    },
    Para(Vec<Inline>),
    /// Text from doc comments: paragraphs separated by blank lines, and
    /// `-` or `*` list items.
    Doc(String),
    Table {
        head: Vec<&'static str>,
        rows: Vec<Vec<Cell>>,
    },
    List(Vec<Vec<Inline>>),
    Diagram {
        /// The file stem of the diagram when written to its own file.
        file: String,
        alt: String,
        svg: String,
        /// The Mermaid version, used in Markdown when configured.
        mermaid: Option<String>,
    },
}

/// One output page.
#[derive(Debug, Clone)]
pub struct Page {
    /// The file stem, also used in links: `work.counter`.
    pub id: String,
    pub title: String,
    pub blocks: Vec<Block>,
}

/// A generated file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFile {
    /// Relative to the output directory.
    pub path: String,
    pub contents: String,
}

/// Leaves out the columns (but the first) that are empty in every row,
/// such as a description column when nothing is documented.
fn prune(head: &[&'static str], rows: &[Vec<Cell>]) -> (Vec<&'static str>, Vec<Vec<Cell>>) {
    let empty = |cell: &Cell| {
        cell.iter().all(|i| match i {
            Inline::Text(t) | Inline::Code(t) => t.trim().is_empty(),
            Inline::Link { .. } => false,
        })
    };
    let keep: Vec<bool> = (0..head.len())
        .map(|c| {
            c == 0
                || rows
                    .iter()
                    .any(|r| r.get(c).is_some_and(|cell| !empty(cell)))
        })
        .collect();
    let head = head
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(h, _)| *h)
        .collect();
    let rows = rows
        .iter()
        .map(|r| {
            r.iter()
                .zip(&keep)
                .filter(|(_, k)| **k)
                .map(|(c, _)| c.clone())
                .collect()
        })
        .collect();
    (head, rows)
}

// ---------------------------------------------------------------- Markdown

fn md_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '|' | '#'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn md_code(text: &str) -> String {
    // A code span cannot hold a `|` inside a table, even escaped in some
    // renderers; GitHub accepts `\|`.
    let text = text.replace('|', "\\|");
    if text.contains('`') {
        format!("`` {text} ``")
    } else {
        format!("`{text}`")
    }
}

fn md_inline(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(t) => out.push_str(&md_escape(t)),
            Inline::Code(t) => out.push_str(&md_code(t)),
            Inline::Link {
                text,
                code,
                page,
                anchor,
            } => {
                let label = if *code {
                    md_code(text)
                } else {
                    md_escape(text)
                };
                let anchor = anchor.as_ref().map(|a| format!("#{a}")).unwrap_or_default();
                let _ = write!(out, "[{label}]({page}.md{anchor})");
            }
        }
    }
    out
}

/// Doc comment text in a table cell: one line.
fn md_cell_doc(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

pub fn markdown(page: &Page, mermaid: bool, files: &mut Vec<OutputFile>) -> String {
    let mut out = String::new();
    for block in &page.blocks {
        match block {
            Block::Heading { level, content, id } => {
                if let Some(id) = id {
                    let _ = writeln!(out, "<a id=\"{id}\"></a>\n");
                }
                let _ = writeln!(
                    out,
                    "{} {}\n",
                    "#".repeat(*level as usize),
                    md_inline(content)
                );
            }
            Block::Para(content) => {
                let _ = writeln!(out, "{}\n", md_inline(content));
            }
            Block::Doc(text) => {
                // Comments are often written in Markdown already: keep them.
                let _ = writeln!(out, "{}\n", text.trim_end());
            }
            Block::Table { head, rows } => {
                let (head, rows) = prune(head, rows);
                let (head, rows) = (&head, &rows);
                let _ = writeln!(out, "| {} |", head.join(" | "));
                let _ = writeln!(out, "|{}", "---|".repeat(head.len()));
                for row in rows {
                    let cells: Vec<String> = row
                        .iter()
                        .map(|cell| {
                            // The last column of every table is a description.
                            match cell.as_slice() {
                                [Inline::Text(t)] => md_cell_doc(t),
                                _ => md_inline(cell),
                            }
                        })
                        .collect();
                    let _ = writeln!(out, "| {} |", cells.join(" | "));
                }
                out.push('\n');
            }
            Block::List(items) => {
                for item in items {
                    let _ = writeln!(out, "- {}", md_inline(item));
                }
                out.push('\n');
            }
            Block::Diagram {
                file,
                alt,
                svg,
                mermaid: text,
            } => match text {
                Some(text) if mermaid => {
                    let _ = writeln!(out, "```mermaid\n{}```\n", text);
                }
                _ => {
                    let path = format!("{file}.svg");
                    let _ = writeln!(out, "![{}]({path})\n", md_escape(alt));
                    files.push(OutputFile {
                        path,
                        contents: svg.clone(),
                    });
                }
            },
        }
    }
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out.push('\n');
    out
}

// ---------------------------------------------------------------- AsciiDoc

/// Text shown as is: wrapped in a passthrough, which only escapes `<`,
/// `>` and `&`, when it holds a character AsciiDoc could read as markup.
fn adoc_text(text: &str) -> String {
    if !text.contains(|c: char| "*_`#+^~{}[]<>&|\\:'\"".contains(c)) {
        text.to_owned()
    } else {
        adoc_pass(text)
    }
}

fn adoc_pass(text: &str) -> String {
    if text.contains('+') {
        format!("pass:c[{}]", text.replace(']', "\\]"))
    } else {
        format!("++{text}++")
    }
}

fn adoc_code(text: &str) -> String {
    format!("``{}``", adoc_pass(text))
}

fn adoc_inline(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(t) => out.push_str(&adoc_text(t)),
            Inline::Code(t) => out.push_str(&adoc_code(t)),
            Inline::Link {
                text,
                code,
                page,
                anchor,
            } => {
                let label = if *code {
                    adoc_code(text)
                } else {
                    adoc_text(text)
                };
                let anchor = anchor.as_ref().map(|a| format!("#{a}")).unwrap_or_default();
                let _ = write!(out, "xref:{page}.adoc{anchor}[{label}]");
            }
        }
    }
    out
}

/// A table cell: one line, with the cell separator escaped.
fn adoc_cell(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

pub fn asciidoc(page: &Page, files: &mut Vec<OutputFile>) -> String {
    let mut out = String::new();
    for block in &page.blocks {
        match block {
            Block::Heading { level, content, id } => {
                if let Some(id) = id {
                    let _ = writeln!(out, "[[{id}]]");
                }
                let _ = writeln!(
                    out,
                    "{} {}\n",
                    "=".repeat(*level as usize),
                    adoc_inline(content)
                );
                if *level == 1 {
                    // Diagrams sit next to the pages.
                    out.truncate(out.len() - 1);
                    out.push_str(":imagesdir: .\n\n");
                }
            }
            Block::Para(content) => {
                let _ = writeln!(out, "{}\n", adoc_inline(content));
            }
            Block::Doc(text) => {
                // Comments are often written in a lightweight markup already;
                // their paragraphs and `-`/`*` lists read the same in AsciiDoc.
                let _ = writeln!(out, "{}\n", text.trim_end());
            }
            Block::Table { head, rows } => {
                let (head, rows) = prune(head, rows);
                let _ = writeln!(out, "[options=\"header\"]\n|===");
                let _ = writeln!(out, "|{}", head.join(" |"));
                for row in &rows {
                    let cells: Vec<String> = row
                        .iter()
                        .map(|cell| match cell.as_slice() {
                            // The last column of every table is a description,
                            // kept as written like other doc comments.
                            [Inline::Text(t)] => adoc_cell(t),
                            _ => adoc_cell(&adoc_inline(cell)),
                        })
                        .collect();
                    let _ = writeln!(out, "|{}", cells.join(" |"));
                }
                out.push_str("|===\n\n");
            }
            Block::List(items) => {
                for item in items {
                    let _ = writeln!(out, "* {}", adoc_inline(item));
                }
                out.push('\n');
            }
            Block::Diagram { file, alt, svg, .. } => {
                let path = format!("{file}.svg");
                let _ = writeln!(
                    out,
                    "image::{path}[\"{}\"]\n",
                    alt.replace('"', "'").replace(']', ")")
                );
                files.push(OutputFile {
                    path,
                    contents: svg.clone(),
                });
            }
        }
    }
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out.push('\n');
    out
}

// -------------------------------------------------------------------- HTML

const CSS: &str = "\
:root{color-scheme:light dark;--bg:#ffffff;--fg:#1f2328;--muted:#59636e;--line:#d1d9e0;--side:#f6f8fa;--code:#eff2f5;--link:#0969da}\
@media (prefers-color-scheme:dark){:root{--bg:#0d1117;--fg:#f0f6fc;--muted:#9198a1;--line:#3d444d;--side:#151b23;--code:#262c36;--link:#4493f8}}\
*{box-sizing:border-box}\
body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 -apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif}\
a{color:var(--link);text-decoration:none}a:hover{text-decoration:underline}\
.layout{display:flex;min-height:100vh}\
nav{width:260px;flex:none;background:var(--side);border-right:1px solid var(--line);padding:16px;position:sticky;top:0;height:100vh;overflow:auto}\
nav h2{font-size:12px;text-transform:uppercase;letter-spacing:.05em;color:var(--muted);margin:16px 0 4px}\
nav ul{list-style:none;margin:0;padding:0}nav li{padding:2px 0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}\
nav .home{font-weight:600;font-size:16px}\
nav .current a{font-weight:600}\
main{flex:1;min-width:0;max-width:1100px;padding:24px 32px}\
h1{font-size:28px;margin:0 0 8px}h2{font-size:21px;margin:32px 0 8px;padding-bottom:4px;border-bottom:1px solid var(--line)}\
h3{font-size:17px;margin:24px 0 8px}h4{font-size:15px;margin:16px 0 8px}\
code{font:13px ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;background:var(--code);padding:1px 5px;border-radius:4px}\
.meta{color:var(--muted)}\
table{border-collapse:collapse;margin:8px 0 16px;display:block;overflow-x:auto;max-width:100%}\
th,td{border:1px solid var(--line);padding:5px 10px;text-align:left;vertical-align:top}\
th{background:var(--side)}\
td code{overflow-wrap:break-word}\
.diagram{overflow-x:auto;margin:8px 0 16px}\
.diagram svg{max-width:100%;height:auto}\
footer{color:var(--muted);font-size:13px;margin-top:48px}\
@media (max-width:760px){.layout{display:block}nav{width:auto;height:auto;position:static}main{padding:16px}}";

/// Lets a diagram shrink to the width of the page, down to 60% of its size;
/// a larger one scrolls.
fn fit(svg: &str) -> std::borrow::Cow<'_, str> {
    let width = svg
        .split_once(" width=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .and_then(|(w, _)| w.parse::<f64>().ok());
    match width {
        Some(w) => svg
            .replacen(
                "<svg ",
                &format!("<svg style=\"min-width:{:.0}px\" ", w * 0.6),
                1,
            )
            .into(),
        None => svg.into(),
    }
}

fn html_inline(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            Inline::Text(t) => out.push_str(&escape(t)),
            Inline::Code(t) => {
                let _ = write!(out, "<code>{}</code>", escape(t));
            }
            Inline::Link {
                text,
                code,
                page,
                anchor,
            } => {
                let anchor = anchor.as_ref().map(|a| format!("#{a}")).unwrap_or_default();
                let label = if *code {
                    format!("<code>{}</code>", escape(text))
                } else {
                    escape(text)
                };
                let _ = write!(
                    out,
                    "<a href=\"{}.html{}\">{label}</a>",
                    escape(page),
                    escape(&anchor)
                );
            }
        }
    }
    out
}

fn html_doc(text: &str) -> String {
    let mut out = String::new();
    for paragraph in text.split("\n\n") {
        let lines: Vec<&str> = paragraph
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            continue;
        }
        let is_item = |l: &str| l.starts_with("- ") || l.starts_with("* ");
        if lines.iter().all(|l| is_item(l)) {
            out.push_str("<ul>");
            for line in lines {
                let _ = write!(out, "<li>{}</li>", escape(&line[2..]));
            }
            out.push_str("</ul>");
        } else {
            let _ = write!(out, "<p>{}</p>", escape(&lines.join(" ")));
        }
    }
    out
}

/// A navigation entry: section, page id and label.
pub struct NavItem {
    pub section: &'static str,
    pub page: String,
    pub label: String,
}

pub fn html(page: &Page, site_title: &str, nav: &[NavItem]) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{} - {}</title>\n<style>{CSS}</style>\n</head>\n<body>\n<div class=\"layout\">\n<nav>\n<div class=\"home\"><a href=\"index.html\">{}</a></div>\n",
        escape(&page.title),
        escape(site_title),
        escape(site_title)
    );
    let mut section = "";
    for item in nav {
        if item.section != section {
            if !section.is_empty() {
                out.push_str("</ul>\n");
            }
            section = item.section;
            let _ = writeln!(out, "<h2>{}</h2>\n<ul>", escape(section));
        }
        let current = if item.page == page.id {
            " class=\"current\""
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "<li{current}><a href=\"{}.html\" title=\"{}\">{}</a></li>",
            escape(&item.page),
            escape(&item.label),
            escape(&item.label)
        );
    }
    if !section.is_empty() {
        out.push_str("</ul>\n");
    }
    out.push_str("</nav>\n<main>\n");
    for block in &page.blocks {
        match block {
            Block::Heading { level, content, id } => {
                let id = id
                    .as_ref()
                    .map(|i| format!(" id=\"{}\"", escape(i)))
                    .unwrap_or_default();
                let _ = writeln!(out, "<h{level}{id}>{}</h{level}>", html_inline(content));
            }
            Block::Para(content) => {
                let _ = writeln!(out, "<p class=\"meta\">{}</p>", html_inline(content));
            }
            Block::Doc(text) => {
                let _ = writeln!(out, "{}", html_doc(text));
            }
            Block::Table { head, rows } => {
                let (head, rows) = prune(head, rows);
                let (head, rows) = (&head, &rows);
                out.push_str("<table>\n<thead><tr>");
                for h in head {
                    let _ = write!(out, "<th>{}</th>", escape(h));
                }
                out.push_str("</tr></thead>\n<tbody>\n");
                for row in rows {
                    out.push_str("<tr>");
                    for cell in row {
                        let _ = write!(out, "<td>{}</td>", html_inline(cell));
                    }
                    out.push_str("</tr>\n");
                }
                out.push_str("</tbody>\n</table>\n");
            }
            Block::List(items) => {
                out.push_str("<ul>\n");
                for item in items {
                    let _ = writeln!(out, "<li>{}</li>", html_inline(item));
                }
                out.push_str("</ul>\n");
            }
            Block::Diagram { svg, .. } => {
                let _ = writeln!(out, "<div class=\"diagram\">{}</div>", fit(svg));
            }
        }
    }
    out.push_str("<footer>Generated by <a href=\"https://github.com/thewenneur/vhone\">vhone</a>.</footer>\n</main>\n</div>\n</body>\n</html>\n");
    out
}
