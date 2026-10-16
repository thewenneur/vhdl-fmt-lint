//! Doc comments: the comments directly above a declaration, or at the end
//! of its line.

use vfl_config::{DocComments, DocConfig};
use vfl_syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};

use crate::spec::{Reader, Role, Tags, comment_body};

/// Which comments of one file document declarations.
#[derive(Debug, Clone)]
pub struct CommentStyle {
    /// Only comments that start with `--` and the marker.
    marked: bool,
    off: bool,
    marker: String,
    /// Spec comments are left to the specification.
    tags: Tags,
}

impl CommentStyle {
    /// The style for a file: `auto` means marked comments if the file has any.
    pub fn for_file(root: &SyntaxNode, config: &DocConfig) -> CommentStyle {
        let comments = config.comments;
        let marker = config.comment_marker.as_str();
        let prefix = format!("--{marker}");
        let marked = match comments {
            DocComments::Marked => true,
            DocComments::All | DocComments::None => false,
            DocComments::Auto => {
                !marker.is_empty()
                    && root
                        .descendants_with_tokens()
                        .filter_map(|e| e.into_token())
                        .any(|t| {
                            t.kind() == SyntaxKind::LINE_COMMENT && t.text().starts_with(&prefix)
                        })
            }
        };
        CommentStyle {
            marked,
            off: comments == DocComments::None,
            marker: marker.to_owned(),
            tags: Tags::new(&config.spec),
        }
    }

    /// The text of a comment if it is a doc comment, without its delimiters.
    /// `Some("")` for a comment that only separates (`-----`).
    fn text(&self, token: &SyntaxToken) -> Option<String> {
        let text = token.text();
        let body = match token.kind() {
            SyntaxKind::LINE_COMMENT => {
                let rest = text.strip_prefix("--")?;
                match rest.strip_prefix(self.marker.as_str()) {
                    Some(rest) if !self.marker.is_empty() => rest,
                    _ if self.marked => return None,
                    _ => rest,
                }
            }
            SyntaxKind::BLOCK_COMMENT if !self.marked => {
                let inner = text.strip_prefix("/*")?;
                inner.strip_suffix("*/").unwrap_or(inner)
            }
            _ => return None,
        };
        // `vfl-ignore` and `vfl: off` comments are directives, not docs.
        let trimmed = body.trim();
        if trimmed.starts_with("vfl-ignore") || trimmed.starts_with("vfl:") {
            return None;
        }
        let lines: Vec<&str> = body
            .lines()
            .map(|l| {
                let l = l.trim_end();
                let l = l.strip_prefix(' ').unwrap_or(l);
                // ` * ` continuation lines of block comments.
                match l.trim_start().strip_prefix('*') {
                    Some(rest) if token.kind() == SyntaxKind::BLOCK_COMMENT => {
                        rest.strip_prefix(' ').unwrap_or(rest)
                    }
                    _ => l,
                }
            })
            .collect();
        let joined = lines.join("\n");
        let decorative = joined
            .chars()
            .all(|c| c.is_whitespace() || "-=*#~_+/".contains(c));
        Some(if decorative {
            String::new()
        } else {
            joined.trim_end().to_owned()
        })
    }

    /// The documentation of the declaration or statement `node`: the
    /// comments directly above it (no blank line between), then the comment
    /// at the end of its first line or, for a list element, after its
    /// separator.
    pub fn doc(&self, node: &SyntaxNode) -> String {
        if self.off {
            return String::new();
        }
        let mut parts = self.leading(node);
        if let Some(trailing) = self.trailing(node) {
            parts.push(trailing);
        }
        normalize(&parts)
    }

    fn leading(&self, node: &SyntaxNode) -> Vec<String> {
        // The comments above, up to a blank line or code.
        let mut run = Vec::new();
        let mut at: Option<SyntaxElement> = previous(&SyntaxElement::Node(node.clone()));
        loop {
            let Some(element) = at else { break };
            match element.kind() {
                SyntaxKind::WHITESPACE => {
                    let text = element
                        .as_token()
                        .map(|t| t.text().to_owned())
                        .unwrap_or_default();
                    if text.matches('\n').count() > 1 {
                        break;
                    }
                }
                SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT => {
                    let token = element.as_token().cloned();
                    let Some(token) = token else { break };
                    // A comment after code on the same line belongs to that code.
                    if !starts_line(&token) {
                        break;
                    }
                    run.push(token);
                }
                _ => break,
            }
            at = previous(&element);
        }
        run.reverse();
        // Without the spec comments, the doc comments directly above.
        let mut reader = Reader::default();
        let mut found = Vec::new();
        for token in &run {
            let body = comment_body(token, &self.marker);
            let block = token.kind() == SyntaxKind::BLOCK_COMMENT;
            if reader.next(body, block, true, &self.tags) != Role::Other {
                continue;
            }
            match self.text(token) {
                Some(text) => found.push(text),
                None => found.clear(),
            }
        }
        found
    }

    fn trailing(&self, node: &SyntaxNode) -> Option<String> {
        // The end of the node's first line, for multi-line statements; the
        // element after the node and its separator otherwise.
        let first_line_end = node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == SyntaxKind::WHITESPACE && t.text().contains('\n'));
        if let Some(newline) = first_line_end {
            let comment = newline
                .prev_token()
                .filter(|t| matches!(t.kind(), SyntaxKind::LINE_COMMENT))?;
            if self.is_spec(&comment) {
                return None;
            }
            return self.text(&comment).filter(|t| !t.is_empty());
        }
        let mut at = node.next_sibling_or_token();
        while let Some(element) = at {
            match element.kind() {
                SyntaxKind::WHITESPACE => {
                    if element.as_token().is_some_and(|t| t.text().contains('\n')) {
                        return None;
                    }
                }
                SyntaxKind::SEMI | SyntaxKind::COMMA => {}
                SyntaxKind::LINE_COMMENT => {
                    let token = element.as_token()?;
                    if self.is_spec(token) {
                        return None;
                    }
                    return self.text(token).filter(|t| !t.is_empty());
                }
                _ => return None,
            }
            at = element.next_sibling_or_token();
        }
        None
    }
}

impl CommentStyle {
    fn is_spec(&self, token: &SyntaxToken) -> bool {
        self.tags
            .tagged(comment_body(token, &self.marker))
            .is_some()
    }
}

/// The element before `element`, climbing out of the design unit and
/// context clause wrappers that leading comments may sit outside of.
fn previous(element: &SyntaxElement) -> Option<SyntaxElement> {
    if let Some(prev) = element.prev_sibling_or_token() {
        return Some(prev);
    }
    let parent = element.parent()?;
    if parent.kind() == SyntaxKind::DESIGN_UNIT {
        parent.prev_sibling_or_token()
    } else {
        None
    }
}

/// Whether only whitespace precedes `token` on its line.
fn starts_line(token: &SyntaxToken) -> bool {
    match token.prev_token() {
        None => true,
        Some(prev) if prev.kind() == SyntaxKind::WHITESPACE => {
            prev.text().contains('\n') || prev.prev_token().is_none()
        }
        Some(_) => false,
    }
}

/// Joins comment lines into paragraphs: blank comment lines separate
/// paragraphs, and leading or trailing blank lines are dropped.
fn normalize(parts: &[String]) -> String {
    let text = parts.join("\n");
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| !l.trim().is_empty());
    let end = lines.iter().rposition(|l| !l.trim().is_empty());
    match (start, end) {
        (Some(start), Some(end)) => lines[start..=end].join("\n"),
        _ => String::new(),
    }
}

/// The first sentence (or line) of a description, for summaries.
pub fn brief(text: &str) -> String {
    let first = text.split("\n\n").next().unwrap_or("").replace('\n', " ");
    match first.find(". ") {
        Some(end) => first[..=end].trim().to_owned(),
        None => first.trim().to_owned(),
    }
}
