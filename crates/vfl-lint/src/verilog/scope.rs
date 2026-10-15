//! What a Verilog design unit declares, and how its names are used.
//!
//! Declarations are collected per unit (module, interface, program or
//! package) in one list, each with the region it is declared in, so that
//! names can be looked up without modeling every nested scope: a name is
//! declared if the unit, the compilation unit, an imported package or an
//! included file declares it anywhere. Uses are classified as reads and
//! writes the same way, by name; a local that shadows a signal makes the
//! signal look used, which only ever hides a finding.

use rowan::WalkEvent;
use rustc_hash::{FxHashMap, FxHashSet};
use vfl_syntax::green_walk::ChildTokens;
use vfl_verilog_syntax::summary::Direction;
use vfl_verilog_syntax::{SyntaxKind as K, SyntaxNode, SyntaxToken, TextRange};

/// What a declaration declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A port of an ANSI port list, or of a port declaration.
    Port(Option<Direction>),
    /// A name in a non-ANSI port list, declared again in the body.
    HeaderPort,
    Net,
    Var,
    Param,
    LocalParam,
    Genvar,
    Type,
    EnumMember,
    Function,
    Task,
    Instance,
    Label,
    Import,
    Modport,
    /// A nested unit, a forward type declaration, or a name in a construct
    /// kept as tokens (a class, a property).
    Other,
}

#[derive(Debug, Clone)]
pub struct Decl {
    pub name: String,
    /// The name's range.
    pub range: TextRange,
    pub kind: Kind,
    /// The scope it is declared in: the unit, a function, a block.
    pub region: SyntaxNode,
    /// Its `DATA_TYPE`, if one is written.
    pub data_type: Option<SyntaxNode>,
    /// The declarator, port or other node the name belongs to.
    pub node: SyntaxNode,
    /// Whether it has unpacked dimensions (a memory).
    pub unpacked: bool,
    /// Whether it has an initializer (`wire a = b;`).
    pub init: bool,
}

impl Decl {
    /// Whether it is declared at the top of its unit or in a generate
    /// block, rather than in a function, task or procedural block.
    pub fn is_static(&self) -> bool {
        matches!(
            self.region.kind(),
            K::MODULE_DECL
                | K::INTERFACE_DECL
                | K::PROGRAM_DECL
                | K::PACKAGE_DECL
                | K::GENERATE_BLOCK
                | K::GENERATE_FOR
                | K::SOURCE_FILE
        )
    }

    /// Whether it is a port, net or variable: something a design reads
    /// and drives.
    pub fn is_signal(&self) -> bool {
        matches!(self.kind, Kind::Port(_) | Kind::Net | Kind::Var)
    }

    /// The keyword of a net (`wire`, `tri`, `supply0`), if it is one.
    pub fn net_keyword(&self) -> Option<K> {
        let data_type = self.data_type.as_ref()?;
        data_type
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .map(|t| t.kind())
            .find(|k| is_net_keyword(*k))
    }

    /// Whether it is a variable rather than a net: declared with a
    /// variable type, or an output port with one.
    pub fn is_variable(&self) -> bool {
        match self.kind {
            Kind::Var => true,
            Kind::Port(Some(Direction::Output)) => {
                self.net_keyword().is_none()
                    && self.data_type.as_ref().is_some_and(|t| {
                        t.has_child(|k| {
                            !k.is_trivia()
                                && !matches!(k, K::DIMENSION | K::SIGNED_KW | K::UNSIGNED_KW)
                        })
                    })
            }
            _ => false,
        }
    }
}

pub fn is_net_keyword(kind: K) -> bool {
    matches!(
        kind,
        K::WIRE_KW
            | K::TRI_KW
            | K::TRI0_KW
            | K::TRI1_KW
            | K::TRIAND_KW
            | K::TRIOR_KW
            | K::TRIREG_KW
            | K::WAND_KW
            | K::WOR_KW
            | K::SUPPLY0_KW
            | K::SUPPLY1_KW
            | K::UWIRE_KW
            | K::INTERCONNECT_KW
    )
}

/// A design unit: a module, interface, program or package.
pub fn is_unit(kind: K) -> bool {
    matches!(
        kind,
        K::MODULE_DECL | K::INTERFACE_DECL | K::PROGRAM_DECL | K::PACKAGE_DECL
    )
}

/// A node whose names are local to it.
fn is_region(kind: K) -> bool {
    is_unit(kind)
        || matches!(
            kind,
            K::SOURCE_FILE
                | K::FUNCTION_DECL
                | K::TASK_DECL
                | K::BLOCK_STMT
                | K::FORK_STMT
                | K::GENERATE_BLOCK
                | K::GENERATE_FOR
                | K::FOR_STMT
                | K::FOREACH_STMT
        )
}

/// A construct kept as a run of tokens, whose names are not modeled.
pub fn is_opaque(kind: K) -> bool {
    matches!(
        kind,
        K::OPAQUE_DECL
            | K::OPAQUE_STMT
            | K::ASSERTION_ITEM
            | K::IMMEDIATE_ASSERT
            | K::CLASS_DECL
            | K::CHECKER_DECL
            | K::DPI_DECL
            | K::EXTERN_DECL
            | K::SPECIFY_BLOCK
            | K::CONFIG_DECL
            | K::PRIMITIVE_DECL
    )
}

fn is_macro(kind: K) -> bool {
    matches!(kind, K::MACRO_ITEM | K::MACRO_EXPR | K::MACRO_ARGS)
}

/// The nearest region holding `node`, not counting `node` itself.
pub fn region_of(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.parent()?.ancestors().find(|n| is_region(n.kind()))
}

/// The name of an identifier token: an escaped identifier without its
/// backslash and the white space that ends it, as `\a ` and `a` are the
/// same name.
pub fn ident(token: &SyntaxToken) -> String {
    let text = token.text();
    match text.strip_prefix('\\') {
        Some(rest) => rest.trim_end().to_owned(),
        None => text.to_owned(),
    }
}

pub fn is_ident(kind: K) -> bool {
    matches!(kind, K::IDENT | K::ESC_IDENT)
}

/// The identifier of a `NAME` child of `node`.
pub fn name_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children()
        .find(|n| n.kind() == K::NAME)?
        .child_token(is_ident)
}

/// The tokens of `node` itself, without trivia.
pub fn tokens(node: &SyntaxNode) -> impl Iterator<Item = SyntaxToken> + '_ {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
}

pub fn has_token(node: &SyntaxNode, kind: K) -> bool {
    tokens(node).any(|t| t.kind() == kind)
}

/// The identifiers in text a construct keeps as tokens, or in code an
/// `` `ifdef `` leaves out.
pub fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .filter(|w| w.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_'))
}

/// The declarations of a unit (or, for the root, of the compilation unit
/// outside units).
#[derive(Debug, Default)]
pub struct Scope {
    pub decls: Vec<Decl>,
    pub by_name: FxHashMap<String, Vec<usize>>,
    /// Packages imported (`import p::*;` or `import p::x;`).
    pub imports: Vec<String>,
    /// Whether a macro used as an item may declare names this scope
    /// cannot see.
    pub open: bool,
    /// Names in constructs kept as tokens and in code `` `ifdef `` leaves
    /// out: taken as declared and as used.
    pub hidden: FxHashSet<String>,
}

impl Scope {
    pub fn collect(scope: &SyntaxNode) -> Scope {
        let mut out = Scope::default();
        let mut walk = scope.preorder_with_tokens();
        while let Some(event) = walk.next() {
            let WalkEvent::Enter(element) = event else {
                continue;
            };
            let node = match element {
                rowan::NodeOrToken::Token(t) => {
                    // Code not compiled, and a macro's body, may use any
                    // name.
                    let define = t.kind() == K::DIRECTIVE && t.text().starts_with("`define");
                    if t.kind() == K::INACTIVE || define {
                        out.hidden.extend(words(t.text()).map(str::to_owned));
                    }
                    continue;
                }
                rowan::NodeOrToken::Node(n) => n,
            };
            if node != *scope && is_unit(node.kind()) {
                if scope.kind() != K::SOURCE_FILE
                    && let Some(name) = name_token(&node)
                {
                    out.push(&name, Kind::Other, &node, None);
                }
                walk.skip_subtree();
                continue;
            }
            if is_opaque(node.kind()) {
                out.hidden
                    .extend(words(&node.to_string()).map(str::to_owned));
                walk.skip_subtree();
                continue;
            }
            match node.kind() {
                K::NAME => out.name(&node, scope),
                K::PORT_LIST => out.port_list(&node),
                // `tagged a '{.v, 0}`: a pattern binds the names it lists.
                K::PATTERN => out
                    .hidden
                    .extend(words(&node.to_string()).map(str::to_owned)),
                K::CASE_ITEM => {
                    let mut dot = false;
                    for t in tokens(&node) {
                        if dot && is_ident(t.kind()) {
                            out.hidden.insert(ident(&t));
                        }
                        dot = t.kind() == K::DOT;
                    }
                }
                K::FOREACH_STMT => {
                    // `foreach (a[i, j])`: the loop variables.
                    for t in tokens(&node).filter(|t| is_ident(t.kind())).skip(1) {
                        let decl = Decl {
                            name: ident(&t),
                            range: t.text_range(),
                            kind: Kind::Var,
                            region: node.clone(),
                            data_type: None,
                            node: node.clone(),
                            unpacked: false,
                            init: true,
                        };
                        out.add(decl);
                    }
                }
                K::GENERATE_FOR | K::FOR_STMT if has_token(&node, K::GENVAR_KW) => {
                    if let Some(name) = node
                        .children()
                        .find(|n| n.kind() == K::ASSIGN_EXPR)
                        .and_then(|a| a.first_child())
                        .filter(|n| n.kind() == K::NAME_REF)
                        .and_then(|n| n.first_token())
                    {
                        out.push_in(&name, Kind::Genvar, &node, node.clone(), None);
                    }
                }
                K::IMPORT_ITEM => {
                    let mut idents = tokens(&node).filter(|t| is_ident(t.kind()));
                    if let Some(package) = idents.next() {
                        out.imports.push(ident(&package));
                        if let Some(name) = idents.next() {
                            out.push(&name, Kind::Import, &node, None);
                        }
                    }
                }
                K::END_LABEL | K::LABEL => {
                    // `begin : name` declares a block; `end : name` repeats it.
                    let declares = node.kind() == K::LABEL
                        || node
                            .prev_sibling_or_token()
                            .into_iter()
                            .flat_map(|e| {
                                std::iter::successors(Some(e), |e| e.prev_sibling_or_token())
                            })
                            .find(|e| !e.kind().is_trivia())
                            .is_some_and(|e| matches!(e.kind(), K::BEGIN_KW | K::FORK_KW));
                    if declares && let Some(name) = tokens(&node).find(|t| is_ident(t.kind())) {
                        out.push(&name, Kind::Label, &node, None);
                    }
                }
                K::MACRO_ITEM => {
                    let item_level = node
                        .parent()
                        .is_some_and(|p| is_unit(p.kind()) || p.kind() == K::GENERATE_BLOCK);
                    if item_level {
                        out.open = true;
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn add(&mut self, decl: Decl) {
        self.by_name
            .entry(decl.name.clone())
            .or_default()
            .push(self.decls.len());
        self.decls.push(decl);
    }

    fn push(&mut self, name: &SyntaxToken, kind: Kind, node: &SyntaxNode, ty: Option<SyntaxNode>) {
        let Some(region) = region_of(node) else {
            return;
        };
        self.push_in(name, kind, node, region, ty);
    }

    fn push_in(
        &mut self,
        name: &SyntaxToken,
        kind: Kind,
        node: &SyntaxNode,
        region: SyntaxNode,
        data_type: Option<SyntaxNode>,
    ) {
        self.add(Decl {
            name: ident(name),
            range: name.text_range(),
            kind,
            region,
            data_type,
            node: node.clone(),
            unpacked: node.children().any(|n| n.kind() == K::DIMENSION),
            init: has_token(node, K::EQ),
        });
    }

    fn name(&mut self, name: &SyntaxNode, scope: &SyntaxNode) {
        let Some(parent) = name.parent() else { return };
        let Some(token) = name.child_token(is_ident) else {
            return;
        };
        let data_type = |n: &SyntaxNode| n.children().find(|c| c.kind() == K::DATA_TYPE);
        match parent.kind() {
            K::DECLARATOR => {
                let Some(decl) = parent.parent() else { return };
                let kind = match decl.kind() {
                    K::PORT_DECL => Kind::Port(direction(&decl)),
                    K::NET_DECL => Kind::Net,
                    K::PARAM_DECL if has_token(&decl, K::LOCALPARAM_KW) => Kind::LocalParam,
                    K::PARAM_DECL => Kind::Param,
                    K::STRUCT_MEMBER => return,
                    K::GENVAR_DECL => Kind::Genvar,
                    _ => Kind::Var,
                };
                self.push(&token, kind, &parent, data_type(&decl));
            }
            K::GENVAR_DECL => self.push(&token, Kind::Genvar, &parent, None),
            K::TYPEDEF_DECL => {
                // `typedef struct s;` declares the name again later.
                let forward = data_type(&parent).is_none_or(|t| {
                    t.children().next().is_none()
                        && tokens(&t).all(|k| {
                            matches!(
                                k.kind(),
                                K::STRUCT_KW | K::UNION_KW | K::ENUM_KW | K::CLASS_KW
                            )
                        })
                });
                let kind = if forward { Kind::Other } else { Kind::Type };
                self.push(&token, kind, &parent, data_type(&parent));
            }
            K::ENUM_MEMBER => self.push(&token, Kind::EnumMember, &parent, None),
            K::FUNCTION_DECL => self.push(&token, Kind::Function, &parent, None),
            K::TASK_DECL => self.push(&token, Kind::Task, &parent, None),
            K::INSTANCE => self.push(&token, Kind::Instance, &parent, None),
            K::MODPORT_ITEM => self.push(&token, Kind::Modport, &parent, None),
            k if is_unit(k) && parent != *scope => self.push(&token, Kind::Other, &parent, None),
            _ => {}
        }
    }

    /// The ports of a module or of a function or task. In an ANSI list a
    /// port without a direction or a type has those of the one before it.
    fn port_list(&mut self, list: &SyntaxNode) {
        let Some(owner) = list.parent() else { return };
        let routine = matches!(owner.kind(), K::FUNCTION_DECL | K::TASK_DECL);
        let ports: Vec<SyntaxNode> = list.children().filter(|n| n.kind() == K::PORT).collect();
        let declares = |p: &SyntaxNode| {
            direction(p).is_some()
                || p.children().any(|c| c.kind() == K::DATA_TYPE)
                || has_token(p, K::INTERFACE_KW)
        };
        let ansi = ports.iter().any(declares);
        let mut previous: (Option<Direction>, Option<SyntaxNode>) = (None, None);
        for port in ports {
            let Some(token) = name_token(&port) else {
                continue;
            };
            let ty = port.children().find(|c| c.kind() == K::DATA_TYPE);
            if routine {
                self.push_in(&token, Kind::Var, &port, owner.clone(), ty);
                continue;
            }
            if !ansi {
                self.push(&token, Kind::HeaderPort, &port, None);
                continue;
            }
            if declares(&port) {
                previous = (direction(&port).or(previous.0), ty);
            }
            self.push(&token, Kind::Port(previous.0), &port, previous.1.clone());
        }
    }

    /// The declarations named `name`.
    pub fn named<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a Decl> + 'a {
        self.by_name
            .get(name)
            .into_iter()
            .flatten()
            .map(|&i| &self.decls[i])
    }

    /// The declarations of `name` visible at `at`: those of the innermost
    /// region holding it.
    pub fn visible<'a>(&'a self, name: &str, at: &SyntaxNode) -> Vec<&'a Decl> {
        let range = at.text_range();
        let mut out: Vec<&Decl> = Vec::new();
        for d in self.named(name) {
            let region = d.region.text_range();
            if !region.contains_range(range) {
                continue;
            }
            match out.first().map(|o| o.region.text_range()) {
                Some(best) if best.len() < region.len() => {}
                Some(best) if best.len() == region.len() => out.push(d),
                _ => out = vec![d],
            }
        }
        out
    }

    /// The declaration `name` most likely refers to from a unit's code: a
    /// signal or a parameter over a local or a label.
    pub fn lookup(&self, name: &str) -> Option<&Decl> {
        let rank = |d: &Decl| match d.kind {
            Kind::Port(_) => 0,
            Kind::Var | Kind::Net if d.is_static() => 1,
            Kind::Param | Kind::LocalParam => 2,
            Kind::HeaderPort => 3,
            _ => 4,
        };
        self.named(name).min_by_key(|d| (rank(d), d.range.start()))
    }
}

/// The direction keyword of a port or port declaration.
pub fn direction(node: &SyntaxNode) -> Option<Direction> {
    tokens(node).find_map(|t| match t.kind() {
        K::INPUT_KW => Some(Direction::Input),
        K::OUTPUT_KW => Some(Direction::Output),
        K::INOUT_KW => Some(Direction::Inout),
        K::REF_KW => Some(Direction::Ref),
        _ => None,
    })
}

/// How a name is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    Both,
}

impl Access {
    pub fn reads(self) -> bool {
        self != Access::Write
    }

    pub fn writes(self) -> bool {
        self != Access::Read
    }
}

/// A name used in an expression.
#[derive(Debug, Clone)]
pub struct Use {
    pub name: String,
    pub range: TextRange,
    pub node: SyntaxNode,
    pub access: Access,
    /// Whether the name must be declared: not a type, a module or a
    /// pattern key, and not under a macro.
    pub checked: bool,
}

/// The `NAME_REF`s of an assignment target: the variables it writes, and
/// whether each is written whole.
pub fn targets(target: &SyntaxNode, out: &mut Vec<(SyntaxNode, bool)>) {
    match target.kind() {
        K::NAME_REF => out.push((target.clone(), true)),
        K::INDEX_EXPR | K::FIELD_EXPR => {
            let mut base = target.clone();
            while matches!(base.kind(), K::INDEX_EXPR | K::FIELD_EXPR) {
                match base.first_child() {
                    Some(b) => base = b,
                    None => return,
                }
            }
            if base.kind() == K::NAME_REF {
                out.push((base, false));
            }
        }
        K::CONCAT_EXPR | K::ASSIGNMENT_PATTERN | K::PATTERN_ITEM | K::PAREN_EXPR => {
            for child in target.children() {
                targets(&child, out);
            }
        }
        // `{<< 8 {a, b}} = p`: the slice size is read, not written.
        K::STREAMING_EXPR => {
            for child in target.children().filter(|c| c.kind() == K::CONCAT_EXPR) {
                targets(&child, out);
            }
        }
        _ => {}
    }
}

/// The name of a `NAME_REF`: its first identifier, or `None` for a
/// system name or a macro.
pub fn ref_name(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.child_token(|k| !k.is_trivia())
        .filter(|t| is_ident(t.kind()))
}

/// Whether a `NAME_REF` is a plain name: not `p::x`, `$unit::x` or an
/// interface type `bus.master`.
pub fn is_simple_ref(node: &SyntaxNode) -> bool {
    !tokens(node).any(|t| matches!(t.kind(), K::COLON2 | K::DOT))
}

/// The direction of each connection of an instance, looked up from the
/// module it instantiates.
pub type PortDirection<'a> = &'a dyn Fn(&SyntaxNode) -> Option<Access>;

/// Every name used in `scope`, with how it is used.
pub fn uses(scope: &SyntaxNode, port_access: PortDirection) -> Vec<Use> {
    let mut out = Vec::new();
    let mut walk = scope.preorder();
    while let Some(event) = walk.next() {
        let WalkEvent::Enter(node) = event else {
            continue;
        };
        if node != *scope && is_unit(node.kind()) || is_opaque(node.kind()) {
            walk.skip_subtree();
            continue;
        }
        if is_macro(node.kind()) || node.kind() == K::DEFPARAM_DECL {
            // Names given to a macro may be read or written.
            for t in node
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .filter(|t| is_ident(t.kind()))
            {
                out.push(Use {
                    name: ident(&t),
                    range: t.text_range(),
                    node: node.clone(),
                    access: Access::Both,
                    checked: false,
                });
            }
            walk.skip_subtree();
            continue;
        }
        if node.kind() != K::NAME_REF {
            continue;
        }
        walk.skip_subtree();
        let Some(token) = ref_name(&node) else {
            continue;
        };
        let checked = is_simple_ref(&node) && !is_type_position(&node);
        out.push(Use {
            name: ident(&token),
            range: token.text_range(),
            access: access(&node, port_access),
            node,
            checked,
        });
    }
    out
}

/// Whether a `NAME_REF` names a type, a module or a pattern key rather
/// than a value.
fn is_type_position(node: &SyntaxNode) -> bool {
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        K::DATA_TYPE | K::TYPE_REFERENCE | K::TYPE_EXPR => true,
        // The module of an instantiation.
        K::INSTANTIATION => parent.first_child().as_ref() == Some(node),
        // `'{a: 1}`: a member name.
        K::PATTERN_ITEM => node
            .next_sibling_or_token()
            .into_iter()
            .flat_map(|e| std::iter::successors(Some(e), |e| e.next_sibling_or_token()))
            .find(|e| !e.kind().is_trivia())
            .is_some_and(|e| e.kind() == K::COLON),
        // `T'(x)`: a type cast.
        K::CAST_EXPR => parent.first_child().as_ref() == Some(node),
        _ => false,
    }
}

fn access(node: &SyntaxNode, port_access: PortDirection) -> Access {
    // Climb through what an assignment target can be made of.
    let mut top = node.clone();
    while let Some(parent) = top.parent() {
        let through = match parent.kind() {
            K::INDEX_EXPR | K::FIELD_EXPR => parent.first_child().as_ref() == Some(&top),
            K::CONCAT_EXPR | K::PAREN_EXPR => true,
            // `{<< 8 {a, b}}`: the concatenation, not the slice size.
            K::STREAMING_EXPR => top.kind() == K::CONCAT_EXPR,
            _ => false,
        };
        if !through {
            break;
        }
        top = parent;
    }
    let Some(parent) = top.parent() else {
        return Access::Read;
    };
    let first = parent.first_child().as_ref() == Some(&top);
    match parent.kind() {
        K::BLOCKING_ASSIGN | K::NONBLOCKING_ASSIGN | K::ASSIGN_EXPR if first => Access::Write,
        K::COMPOUND_ASSIGN if first => Access::Both,
        K::POSTFIX_EXPR => Access::Both,
        K::UNARY_EXPR if has_token(&parent, K::PLUS2) || has_token(&parent, K::MINUS2) => {
            Access::Both
        }
        K::PROCEDURAL_CONTINUOUS | K::EVENT_TRIGGER => Access::Write,
        K::CONNECTION => port_access(&parent).unwrap_or(Access::Both),
        K::ARG_LIST => {
            let call = parent.parent();
            let callee = call
                .as_ref()
                .and_then(|c| c.first_child())
                .and_then(|n| n.first_token())
                .map(|t| t.text().to_owned())
                .unwrap_or_default();
            let statement = call
                .as_ref()
                .and_then(|c| c.parent())
                .is_some_and(|p| p.kind() == K::EXPR_STMT);
            if callee.starts_with('$') {
                // `$readmemh("f", mem)`, `$fscanf(fd, "%d", x)`, `$sformat(s, ...)`.
                let writes = [
                    "$readmem", "$fscanf", "$sscanf", "$fread", "$fgets", "$value$", "$sformat",
                    "$swrite", "$cast",
                ]
                .iter()
                .any(|p| callee.starts_with(p));
                if writes { Access::Both } else { Access::Read }
            } else if statement {
                // A task may write its arguments.
                Access::Both
            } else {
                Access::Read
            }
        }
        _ => Access::Read,
    }
}
