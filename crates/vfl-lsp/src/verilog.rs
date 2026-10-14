//! Editor features of Verilog and SystemVerilog files, read from their
//! syntax trees: syntax errors, the outline, folding and colored names.
//! The analysis knows these files only for the modules VHDL instantiates,
//! so none of this goes through it.

use std::collections::HashMap;
use std::path::Path;

use lsp_types::SymbolKind;
use vfl_project::VerilogFile;
use vfl_syntax::{TextRange, TextSize};
use vfl_verilog_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};

use crate::semantic::{Kind, Token, modifier};
use crate::world::World;

use SyntaxKind::*;

/// The Verilog file at `path`, when the project has one there.
pub(crate) fn file<'a>(world: &'a World, path: &Path) -> Option<&'a VerilogFile> {
    world
        .project
        .verilog
        .get(world.project.verilog_index(path)?)
}

/// The name token of a `NAME` node, or the first name token of `node`.
fn name_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| matches!(t.kind(), IDENT | ESC_IDENT))
}

/// The name of a node that declares one: its `NAME` child's token.
fn declared_name(node: &SyntaxNode) -> Option<SyntaxToken> {
    name_token(&node.children().find(|c| c.kind() == NAME)?)
}

/// The label of a block (`begin : name` or `name : begin`).
fn block_label(node: &SyntaxNode) -> Option<SyntaxToken> {
    let label = node
        .children()
        .find(|c| matches!(c.kind(), LABEL | END_LABEL))?;
    name_token(&label)
}

/// The text of a name, without the `\` and the white space that end an
/// escaped identifier.
fn name_text(token: &SyntaxToken) -> &str {
    token.text().trim_end()
}

/// The keyword starting a node, if it starts with one.
fn first_keyword(node: &SyntaxNode) -> Option<SyntaxKind> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
        .map(|t| t.kind())
        .filter(|k| k.is_keyword())
}

fn is_design_unit(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        MODULE_DECL
            | INTERFACE_DECL
            | PROGRAM_DECL
            | PACKAGE_DECL
            | PRIMITIVE_DECL
            | CLASS_DECL
            | CHECKER_DECL
    )
}

/// How the name of a design unit is colored.
fn unit_kind(kind: SyntaxKind) -> Kind {
    match kind {
        PACKAGE_DECL => Kind::Namespace,
        CLASS_DECL => Kind::Type,
        _ => Kind::Entity,
    }
}

/// An entry of the outline.
pub(crate) struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    /// What it is: `module`, `port`, `instance`.
    pub what: &'static str,
    /// What it is, in more words: `instance of fifo`.
    pub detail: String,
    pub range: TextRange,
    pub selection: TextRange,
    pub children: Vec<Symbol>,
}

impl Symbol {
    fn new(name: &SyntaxToken, kind: SymbolKind, what: &'static str, node: &SyntaxNode) -> Symbol {
        let selection = name.text_range();
        let mut range = node.text_range();
        if !range.contains_range(selection) {
            range = selection;
        }
        Symbol {
            name: name_text(name).to_owned(),
            kind,
            what,
            detail: what.to_owned(),
            range,
            selection,
            children: Vec::new(),
        }
    }
}

/// The outline of a file: its design units, and in them their parameters,
/// ports, declarations, subprograms, instances and named blocks.
pub(crate) fn symbols(root: &SyntaxNode) -> Vec<Symbol> {
    let mut out = Vec::new();
    outline(root, &mut out);
    out
}

fn outline(node: &SyntaxNode, out: &mut Vec<Symbol>) {
    for child in node.children() {
        let kind = child.kind();
        if is_design_unit(kind) {
            let Some(name) = declared_name(&child) else {
                continue;
            };
            let (symbol_kind, detail) = match kind {
                MODULE_DECL => (SymbolKind::MODULE, "module"),
                INTERFACE_DECL => (SymbolKind::INTERFACE, "interface"),
                PROGRAM_DECL => (SymbolKind::MODULE, "program"),
                PACKAGE_DECL => (SymbolKind::PACKAGE, "package"),
                PRIMITIVE_DECL => (SymbolKind::MODULE, "primitive"),
                CLASS_DECL => (SymbolKind::CLASS, "class"),
                _ => (SymbolKind::MODULE, "checker"),
            };
            let mut symbol = Symbol::new(&name, symbol_kind, detail, &child);
            outline(&child, &mut symbol.children);
            out.push(symbol);
            continue;
        }
        match kind {
            FUNCTION_DECL | TASK_DECL => {
                if let Some(name) = declared_name(&child) {
                    let (symbol_kind, detail) = if kind == FUNCTION_DECL {
                        (SymbolKind::FUNCTION, "function")
                    } else {
                        (SymbolKind::METHOD, "task")
                    };
                    out.push(Symbol::new(&name, symbol_kind, detail, &child));
                }
            }
            TYPEDEF_DECL => {
                if let Some(name) = declared_name(&child) {
                    let ty = child
                        .children()
                        .find(|c| c.kind() == DATA_TYPE)
                        .and_then(|t| t.first_child())
                        .map(|t| t.kind());
                    let symbol_kind = match ty {
                        Some(STRUCT_TYPE) => SymbolKind::STRUCT,
                        Some(ENUM_TYPE) => SymbolKind::ENUM,
                        _ => SymbolKind::TYPE_PARAMETER,
                    };
                    out.push(Symbol::new(&name, symbol_kind, "type", &child));
                }
            }
            PORT => {
                if let Some(name) = declared_name(&child) {
                    out.push(Symbol::new(&name, SymbolKind::FIELD, "port", &child));
                }
            }
            PARAM_DECL | PORT_DECL | NET_DECL | DATA_DECL => {
                let (symbol_kind, detail) = match kind {
                    PARAM_DECL if first_keyword(&child) == Some(LOCALPARAM_KW) => {
                        (SymbolKind::CONSTANT, "localparam")
                    }
                    PARAM_DECL => (SymbolKind::CONSTANT, "parameter"),
                    PORT_DECL => (SymbolKind::FIELD, "port"),
                    NET_DECL => (SymbolKind::VARIABLE, "net"),
                    _ => (SymbolKind::VARIABLE, "variable"),
                };
                for declarator in child.children().filter(|c| c.kind() == DECLARATOR) {
                    if let Some(name) = declared_name(&declarator) {
                        out.push(Symbol::new(&name, symbol_kind, detail, &child));
                    }
                }
            }
            INSTANTIATION => {
                let module = child
                    .children()
                    .find(|c| c.kind() == NAME_REF)
                    .map(|m| m.text().to_string())
                    .unwrap_or_default();
                for instance in child.children().filter(|c| c.kind() == INSTANCE) {
                    if let Some(name) = declared_name(&instance) {
                        let mut symbol =
                            Symbol::new(&name, SymbolKind::OBJECT, "instance", &instance);
                        symbol.detail = format!("instance of {module}");
                        out.push(symbol);
                    }
                }
            }
            GENERATE_BLOCK => match block_label(&child) {
                Some(name) => {
                    let mut symbol =
                        Symbol::new(&name, SymbolKind::NAMESPACE, "generate block", &child);
                    outline(&child, &mut symbol.children);
                    out.push(symbol);
                }
                None => outline(&child, out),
            },
            PORT_LIST | PARAM_PORT_LIST | GENERATE_REGION | GENERATE_FOR | GENERATE_IF
            | GENERATE_CASE | ELSE_CLAUSE | CASE_ITEM => outline(&child, out),
            _ => {}
        }
    }
}

/// What can be folded: blocks, lists that span lines, runs of line
/// comments, and code that is not compiled. Each range runs from the start
/// of the region to the start of its last token.
pub(crate) fn folds(root: &SyntaxNode) -> Vec<(TextRange, Fold)> {
    let mut out = Vec::new();
    let mut comments: Option<TextRange> = None;
    let end_comments = |comments: &mut Option<TextRange>, out: &mut Vec<_>| {
        if let Some(range) = comments.take() {
            out.push((range, Fold::Comment));
        }
    };
    let up_to_last = |range: TextRange, last: TextSize| TextRange::new(range.start(), last);
    for event in root.preorder_with_tokens() {
        let rowan::WalkEvent::Enter(element) = event else {
            continue;
        };
        match element {
            rowan::NodeOrToken::Node(node) => {
                if (is_design_unit(node.kind())
                    || matches!(
                        node.kind(),
                        FUNCTION_DECL
                            | TASK_DECL
                            | BLOCK_STMT
                            | FORK_STMT
                            | CASE_STMT
                            | GENERATE_REGION
                            | GENERATE_BLOCK
                            | GENERATE_CASE
                            | PORT_LIST
                            | PARAM_PORT_LIST
                            | INSTANCE
                            | STRUCT_TYPE
                            | ENUM_TYPE
                            | SPECIFY_BLOCK
                            | MODPORT_DECL
                            | CASE_ITEM
                    ))
                    && let Some(last) = node.last_token()
                {
                    out.push((
                        up_to_last(node.text_range(), last.text_range().start()),
                        Fold::Code,
                    ));
                }
            }
            rowan::NodeOrToken::Token(token) => match token.kind() {
                LINE_COMMENT => {
                    let start = token.text_range().start();
                    comments = Some(match comments {
                        Some(run) => TextRange::new(run.start(), start),
                        None => TextRange::empty(start),
                    });
                }
                WHITESPACE if token.text().matches('\n').count() < 2 => {}
                WHITESPACE => end_comments(&mut comments, &mut out),
                BLOCK_COMMENT => {
                    end_comments(&mut comments, &mut out);
                    out.push((
                        up_to_last(token.text_range(), token.text_range().end()),
                        Fold::Comment,
                    ));
                }
                INACTIVE => {
                    end_comments(&mut comments, &mut out);
                    out.push((
                        up_to_last(token.text_range(), token.text_range().end()),
                        Fold::Region,
                    ));
                }
                _ => end_comments(&mut comments, &mut out),
            },
        }
    }
    end_comments(&mut comments, &mut out);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fold {
    Code,
    Comment,
    /// Code that is not compiled.
    Region,
}

/// How a name is colored, by name, in a design unit or in the file.
type Scope = HashMap<String, (Kind, u32)>;

/// The names of a file colored by what they are declared as, in order,
/// within `within` when given. A name used in a design unit is colored as
/// the declaration of that name in the unit, or else in the file; names
/// are matched by spelling, without scopes inside a unit.
pub(crate) fn tokens(root: &SyntaxNode, within: Option<TextRange>) -> Vec<Token> {
    let mut out = Vec::new();
    let mut file_scope: Scope = HashMap::new();
    // Declarations outside design units, and the units' own names.
    let mut units = Vec::new();
    for child in root.children() {
        if is_design_unit(child.kind()) {
            if let Some(name) = declared_name(&child) {
                file_scope.insert(name_text(&name).to_owned(), (unit_kind(child.kind()), 0));
            }
            units.push(child);
        } else {
            declarations(&child, &mut file_scope, &mut out);
        }
    }
    for unit in &units {
        let mut scope = file_scope.clone();
        declarations(unit, &mut scope, &mut out);
        references(unit, &scope, &mut out);
    }
    for child in root.children().filter(|c| !is_design_unit(c.kind())) {
        references(&child, &file_scope, &mut out);
    }
    let text_len = root.text_range().end();
    let within = within.unwrap_or(TextRange::up_to(text_len));
    out.retain(|t| t.range.start() < within.end() && within.start() < t.range.end());
    out.sort_unstable_by_key(|t| t.range.start());
    let mut end = TextSize::from(0);
    out.retain(|t| {
        let keep = t.range.start() >= end;
        if keep {
            end = t.range.end();
        }
        keep
    });
    out
}

/// Whether `node` is inside a subprogram or a procedural block, where
/// declarations are local variables.
fn is_local(node: &SyntaxNode) -> bool {
    node.ancestors()
        .take_while(|a| !is_design_unit(a.kind()))
        .any(|a| matches!(a.kind(), FUNCTION_DECL | TASK_DECL | PROCEDURE | LOCAL_DECL))
}

/// The direction modifiers of a port declared with keyword `kind`.
fn direction(kind: Option<SyntaxKind>) -> Option<u32> {
    use modifier::*;
    Some(match kind? {
        INPUT_KW => IN | READONLY,
        OUTPUT_KW => OUT,
        INOUT_KW => INOUT,
        REF_KW => INOUT,
        _ => return None,
    })
}

/// Adds the declarations in `node` to `scope` and their names to `out`.
fn declarations(node: &SyntaxNode, scope: &mut Scope, out: &mut Vec<Token>) {
    // The direction of the ports of a list so far: `input a, b` declares
    // two inputs.
    let mut port_direction = None;
    for descendant in node.descendants() {
        if descendant.kind() == PORT_LIST {
            port_direction = None;
        }
        if descendant.kind() != NAME {
            continue;
        }
        let Some(name) = name_token(&descendant) else {
            continue;
        };
        let Some(parent) = descendant.parent() else {
            continue;
        };
        let (kind, modifiers) = match parent.kind() {
            PORT => {
                let in_subprogram = parent
                    .parent()
                    .and_then(|list| list.parent())
                    .is_some_and(|p| matches!(p.kind(), FUNCTION_DECL | TASK_DECL));
                let dir = direction(first_keyword(&parent));
                if dir.is_some() {
                    port_direction = dir;
                }
                let modifiers = port_direction.unwrap_or(0);
                if in_subprogram {
                    (Kind::Parameter, modifiers)
                } else {
                    (Kind::Port, modifiers)
                }
            }
            DECLARATOR => {
                let Some(declaration) = parent.parent() else {
                    continue;
                };
                match declaration.kind() {
                    PARAM_DECL if first_keyword(&declaration) == Some(LOCALPARAM_KW) => {
                        (Kind::Constant, modifier::READONLY)
                    }
                    PARAM_DECL => (Kind::Generic, modifier::READONLY),
                    PORT_DECL => {
                        let mut tokens = declaration
                            .children_with_tokens()
                            .filter_map(|e| e.into_token())
                            .filter(|t| !t.kind().is_trivia());
                        let keyword = tokens.next().map(|t| t.kind());
                        let keyword = match keyword {
                            Some(CONST_KW) => tokens.next().map(|t| t.kind()),
                            k => k,
                        };
                        let kind = if is_local(&declaration) {
                            Kind::Parameter
                        } else {
                            Kind::Port
                        };
                        (kind, direction(keyword).unwrap_or(0))
                    }
                    NET_DECL => (Kind::Signal, 0),
                    DATA_DECL if !is_local(&declaration) => (Kind::Signal, 0),
                    DATA_DECL | LOCAL_DECL => (Kind::Variable, 0),
                    GENVAR_DECL => (Kind::Variable, modifier::READONLY),
                    _ => continue,
                }
            }
            GENVAR_DECL => (Kind::Variable, modifier::READONLY),
            TYPEDEF_DECL => (Kind::Type, 0),
            ENUM_MEMBER => (Kind::EnumMember, modifier::READONLY),
            FUNCTION_DECL => (Kind::Function, 0),
            TASK_DECL => (Kind::Procedure, 0),
            INSTANCE => (Kind::Label, 0),
            kind if is_design_unit(kind) => (unit_kind(kind), 0),
            _ => continue,
        };
        declare(scope, name_text(&name), kind, modifiers, is_local(&parent));
        out.push(Token {
            range: name.text_range(),
            kind,
            modifiers: modifiers | modifier::DECLARATION,
        });
    }
}

/// Records that `name` is declared as `kind`. A unit-level declaration
/// wins over a local one, and a port over the net or variable that gives
/// it a type (`output b; reg b;`).
fn declare(scope: &mut Scope, name: &str, kind: Kind, modifiers: u32, local: bool) {
    match scope.get(name) {
        Some(_) if local => {}
        Some((Kind::Port, _)) if kind != Kind::Port => {}
        _ => {
            scope.insert(name.to_owned(), (kind, modifiers));
        }
    }
}

/// Adds the names `node` refers to, colored as declared in `scope`.
fn references(node: &SyntaxNode, scope: &Scope, out: &mut Vec<Token>) {
    for descendant in node.descendants() {
        match descendant.kind() {
            NAME_REF => {
                let names: Vec<SyntaxToken> = descendant
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .filter(|t| matches!(t.kind(), IDENT | ESC_IDENT))
                    .collect();
                let Some((last, packages)) = names.split_last() else {
                    continue;
                };
                for package in packages {
                    out.push(Token {
                        range: package.text_range(),
                        kind: Kind::Namespace,
                        modifiers: 0,
                    });
                }
                let instantiated = descendant
                    .parent()
                    .is_some_and(|p| p.kind() == INSTANTIATION)
                    && descendant.prev_sibling().is_none();
                let class = if instantiated {
                    Some((Kind::Entity, 0))
                } else if packages.is_empty() {
                    scope.get(name_text(last)).copied()
                } else {
                    None
                };
                let Some((kind, mut modifiers)) = class else {
                    continue;
                };
                if is_written(&descendant) {
                    modifiers |= modifier::MODIFICATION;
                }
                out.push(Token {
                    range: last.text_range(),
                    kind,
                    modifiers: modifiers & !modifier::DECLARATION,
                });
            }
            // `.port(...)` and `.parameter(...)` in an instance.
            CONNECTION => {
                let kind = match descendant.parent().map(|p| p.kind()) {
                    Some(INSTANCE) => Kind::Port,
                    Some(PARAM_VALUES) => Kind::Generic,
                    _ => continue,
                };
                if let Some(name) = declared_name(&descendant) {
                    out.push(Token {
                        range: name.text_range(),
                        kind,
                        modifiers: 0,
                    });
                }
            }
            LABEL | END_LABEL => {
                let Some(name) = name_token(&descendant) else {
                    continue;
                };
                let (kind, modifiers) = match scope.get(name_text(&name)) {
                    Some(&(kind, _)) if kind != Kind::Label => (kind, 0),
                    _ => (Kind::Label, 0),
                };
                // The label of a block declares it; one after `end` names it.
                let previous = std::iter::successors(descendant.prev_sibling_or_token(), |e| {
                    e.prev_sibling_or_token()
                })
                .find(|e| !e.kind().is_trivia());
                let declares = descendant.kind() == LABEL
                    || previous.is_some_and(|e| matches!(e.kind(), BEGIN_KW | FORK_KW));
                out.push(Token {
                    range: name.text_range(),
                    kind,
                    modifiers: if declares && kind == Kind::Label {
                        modifiers | modifier::DECLARATION
                    } else {
                        modifiers
                    },
                });
            }
            _ => {}
        }
    }
}

/// Whether the name `node` is the target of an assignment, alone or
/// indexed (`q[0] <= d`).
fn is_written(node: &SyntaxNode) -> bool {
    let mut node = node.clone();
    loop {
        let Some(parent) = node.parent() else {
            return false;
        };
        let first = parent.first_child().is_some_and(|c| c == node);
        if !first {
            return false;
        }
        match parent.kind() {
            INDEX_EXPR | FIELD_EXPR => node = parent,
            BLOCKING_ASSIGN | NONBLOCKING_ASSIGN | COMPOUND_ASSIGN | ASSIGN_EXPR => return true,
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    const TEXT: &str = "\
// A counter.
// Counts.
module counter #(parameter int W = 8) (
  input  logic         clk,
  output logic [W-1:0] q
);
  localparam int ONE = 1;
  logic [W-1:0] n;
  typedef enum {IDLE, RUN} state_t;
  always_ff @(posedge clk) begin : count
    n <= n + ONE;
  end
  assign q = n;
  sub #(.W(W)) u_sub (.clk(clk), .q());
endmodule
";

    fn world(text: &str) -> (tempfile::TempDir, std::path::PathBuf, World) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("counter.sv");
        std::fs::write(&path, text).unwrap();
        let mut world = World::new(dir.path());
        world.refresh();
        (dir, path, world)
    }

    fn request(world: &mut World, method: &str, params: Value) -> Value {
        match crate::handlers::request(world, method, params) {
            Ok(value) => value,
            Err(_) => panic!("{method} failed"),
        }
    }

    /// `text` with each colored name's kind and modifiers after it.
    fn annotated(text: &str) -> String {
        let root = vfl_verilog_syntax::parse(text, Default::default()).syntax();
        let mut out = String::new();
        for token in tokens(&root, None) {
            let modifiers: String = modifier::NAMES
                .iter()
                .enumerate()
                .filter(|(i, _)| token.modifiers & (1 << i) != 0)
                .map(|(_, m)| format!(" {m}"))
                .collect();
            out.push_str(&format!(
                "{} {}{modifiers}\n",
                &text[token.range],
                token.kind.name()
            ));
        }
        out
    }

    #[test]
    fn names_are_colored_by_declaration() {
        assert_eq!(
            annotated(TEXT),
            "\
counter class declaration
W generic declaration readonly
clk port declaration readonly in
W generic readonly
q port declaration out
ONE constant declaration readonly
W generic readonly
n signal declaration
IDLE enumMember declaration readonly
RUN enumMember declaration readonly
state_t type declaration
clk port readonly in
count label declaration
n signal modification
n signal
ONE constant readonly
q port modification out
n signal
sub class
W generic
W generic readonly
u_sub label declaration
clk port
clk port readonly in
q port
"
        );
    }

    #[test]
    fn requests_on_verilog_documents() {
        let (_dir, path, mut world) = world(TEXT);
        let uri = lsp_types::Url::from_file_path(&path).unwrap();
        let document = json!({ "textDocument": { "uri": uri } });

        let symbols = request(&mut world, "textDocument/documentSymbol", document.clone());
        let module = &symbols[0];
        assert_eq!(module["name"], "counter");
        let children: Vec<&str> = module["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(children, ["W", "clk", "q", "ONE", "n", "state_t", "u_sub"]);

        let folds = request(&mut world, "textDocument/foldingRange", document.clone());
        let lines: Vec<(u64, u64)> = folds
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["startLine"].as_u64().unwrap(),
                    f["endLine"].as_u64().unwrap(),
                )
            })
            .collect();
        // The comments, the module, its ports and the `always` block.
        assert_eq!(lines, [(2, 13), (0, 1), (2, 4), (9, 10)]);

        let tokens = request(&mut world, "textDocument/semanticTokens/full", document);
        assert!(!tokens["data"].as_array().unwrap().is_empty());

        // Formatting reads the open document.
        let messy = TEXT.replace("assign q = n;", "assign   q=n;");
        world.set_text(path.clone(), messy);
        let edits = request(
            &mut world,
            "textDocument/formatting",
            json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 2, "insertSpaces": true } }),
        );
        assert_eq!(edits.as_array().unwrap().len(), 1, "{edits}");

        // Requests answered from the analysis of VHDL have no answer here.
        let at =
            json!({ "textDocument": { "uri": uri }, "position": { "line": 10, "character": 4 } });
        world.refresh();
        assert_eq!(
            request(&mut world, "textDocument/hover", at.clone()),
            Value::Null
        );
        assert_eq!(
            request(&mut world, "textDocument/definition", at),
            Value::Null
        );
    }

    #[test]
    fn syntax_errors_are_published() {
        let (_dir, path, world) = world("module m;\n  assign a = ;\nendmodule\n");
        let diagnostics = crate::handlers::diagnostics(&world);
        let uri = lsp_types::Url::from_file_path(&path).unwrap();
        let (_, found) = diagnostics.iter().find(|(u, _)| *u == uri).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].range.start.line, 1);
    }
}
