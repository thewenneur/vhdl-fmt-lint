//! Text diagram formats: Mermaid (rendered by GitHub, GitLab and many
//! Markdown viewers) and Graphviz DOT.

use std::fmt::Write;

use crate::fsm::Fsm;
use crate::hierarchy::Node;

/// Mermaid labels cannot hold quotes, and `:`, `;` and `#` have meanings.
fn mermaid_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '#' => out.push_str("#35;"),
            '"' => out.push_str("#quot;"),
            ':' => out.push_str("#58;"),
            ';' => out.push_str("#59;"),
            '<' => out.push_str("#lt;"),
            '>' => out.push_str("#gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// The longest transition label of a text diagram.
const MAX_LABEL: usize = 60;

fn cut(text: &str) -> String {
    if text.chars().count() <= MAX_LABEL {
        text.to_owned()
    } else {
        let mut out: String = text.chars().take(MAX_LABEL - 1).collect();
        out.push('…');
        out
    }
}

fn dot_text(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn node_text(node: &Node) -> String {
    let mut text = node.unit.clone();
    if let Some(arch) = &node.architecture {
        let _ = write!(text, " ({arch})");
    }
    if node.recursive {
        text.push_str(" (recursive)");
    } else if node.repeated {
        text.push_str(" (see above)");
    } else if node.entity.is_none() {
        text.push_str(" (external)");
    }
    text
}

fn walk(
    node: &Node,
    parent: Option<usize>,
    next: &mut usize,
    f: &mut dyn FnMut(&Node, usize, Option<usize>),
) {
    let id = *next;
    *next += 1;
    f(node, id, parent);
    for child in &node.children {
        walk(child, Some(id), next, f);
    }
}

/// The instantiation tree as a Mermaid flowchart.
pub fn hierarchy_mermaid(roots: &[Node]) -> String {
    let mut out = String::from("flowchart TD\n");
    let mut next = 0;
    for root in roots {
        walk(root, None, &mut next, &mut |node, id, parent| {
            let _ = writeln!(out, "  n{id}[\"{}\"]", mermaid_text(&node_text(node)));
            if node.entity.is_none() {
                let _ = writeln!(out, "  style n{id} stroke-dasharray: 4 3");
            }
            if let Some(parent) = parent {
                let label = node.label.as_deref().unwrap_or("");
                let _ = writeln!(out, "  n{parent} -->|\"{}\"| n{id}", mermaid_text(label));
            }
        });
    }
    out
}

/// The instantiation tree as a DOT graph.
pub fn hierarchy_dot(roots: &[Node]) -> String {
    let mut out = String::from(
        "digraph hierarchy {\n  rankdir=TB;\n  node [shape=box, style=rounded, fontname=\"monospace\"];\n  edge [fontname=\"monospace\", fontsize=10];\n",
    );
    let mut next = 0;
    for root in roots {
        walk(root, None, &mut next, &mut |node, id, parent| {
            let style = if node.entity.is_none() {
                ", style=\"rounded,dashed\""
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "  n{id} [label=\"{}\"{style}];",
                dot_text(&node_text(node))
            );
            if let Some(parent) = parent {
                let label = node.label.as_deref().unwrap_or("");
                let _ = writeln!(out, "  n{parent} -> n{id} [label=\"{}\"];", dot_text(label));
            }
        });
    }
    out.push_str("}\n");
    out
}

/// A state machine as a Mermaid state diagram.
pub fn fsm_mermaid(machine: &Fsm) -> String {
    let mut out = String::from("stateDiagram-v2\n");
    // State ids must be plain words; extended identifiers get an alias.
    let id = |state: &str| -> String {
        let plain = state.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if plain {
            state.to_owned()
        } else {
            let index = machine.states.iter().position(|s| s == state).unwrap_or(0);
            format!("s{index}")
        }
    };
    for state in &machine.states {
        if id(state) != *state {
            let _ = writeln!(out, "  state \"{}\" as {}", mermaid_text(state), id(state));
        }
    }
    if let Some(initial) = &machine.initial {
        let _ = writeln!(out, "  [*] --> {}", id(initial));
    }
    for t in &machine.transitions {
        match &t.condition {
            Some(condition) => {
                let _ = writeln!(
                    out,
                    "  {} --> {} : {}",
                    id(&t.from),
                    id(&t.to),
                    mermaid_text(&cut(condition))
                );
            }
            None => {
                let _ = writeln!(out, "  {} --> {}", id(&t.from), id(&t.to));
            }
        }
    }
    out
}

/// A state machine as a DOT graph; the transitions into the same state
/// merge (`concentrate`).
pub fn fsm_dot(machine: &Fsm) -> String {
    let mut out = format!(
        "digraph \"{}\" {{\n  rankdir=LR;\n  concentrate=true;\n  node [shape=circle, fontname=\"monospace\"];\n  edge [fontname=\"monospace\", fontsize=10];\n",
        dot_text(&machine.signal)
    );
    for state in &machine.states {
        let shape = if machine.initial.as_deref() == Some(state.as_str()) {
            " [shape=doublecircle]"
        } else {
            ""
        };
        let _ = writeln!(out, "  \"{}\"{shape};", dot_text(state));
    }
    if let Some(initial) = &machine.initial {
        let _ = writeln!(
            out,
            "  __start [shape=point];\n  __start -> \"{}\";",
            dot_text(initial)
        );
    }
    for t in &machine.transitions {
        let label = t
            .condition
            .as_deref()
            .map(|c| format!(" [label=\"{}\"]", dot_text(&cut(c))))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "  \"{}\" -> \"{}\"{label};",
            dot_text(&t.from),
            dot_text(&t.to)
        );
    }
    out.push_str("}\n");
    out
}
