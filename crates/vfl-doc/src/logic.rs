//! The logic of an architecture as expression trees: for each signal its
//! concurrent assignment, or the value its process gives it, with the
//! `if`, `case` and conditional assignments as multiplexers.
//!
//! A process is evaluated statement by statement: a signal assigned in
//! both branches of an `if` gets a multiplexer, one assigned in one branch
//! only keeps its value in the other (a register, or a latch). Variables
//! are replaced by the value they hold where they are read. A clocked
//! process is evaluated from its clock edge on (the reset branch sets
//! constants), so each of its signals is a register fed by the tree.

use vfl_syntax::{SyntaxKind, SyntaxNode, identifier_key};

use crate::model::{base_name, compact, edge};

/// Past this many nodes a tree is too large to draw.
pub const MAX_NODES: usize = 400;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// A name: a signal, port, constant, generic or enumeration literal.
    Name(String),
    /// A literal, an aggregate of literals or an attribute, as written.
    Value(String),
    /// An operator in lower case (`and`, `+`, `=`, `not`) and its operands.
    /// `(3) :=` updates part of its first operand with its second.
    Op(String, Vec<Expr>),
    /// A function call, or an indexed or sliced name: the name, the
    /// arguments, and the whole as written.
    Call(String, Vec<Expr>, String),
    /// A multiplexer: its select, and the value of each choice (`1` and `0`
    /// for a condition).
    Mux(Box<Expr>, Vec<(String, Expr)>),
}

impl Expr {
    pub fn size(&self) -> usize {
        match self {
            Expr::Name(_) | Expr::Value(_) => 1,
            Expr::Op(_, args) | Expr::Call(_, args, _) => {
                1 + args.iter().map(Expr::size).sum::<usize>()
            }
            Expr::Mux(s, arms) => 1 + s.size() + arms.iter().map(|(_, e)| e.size()).sum::<usize>(),
        }
    }
}

/// The value of one signal.
#[derive(Debug, Clone)]
pub struct Cone {
    pub target: String,
    pub expr: Expr,
    /// The clock of a register; `None` for combinational logic.
    pub clock: Option<String>,
    pub falling: bool,
}

/// The cones of an architecture: its concurrent assignments, then its
/// processes, in source order.
pub(crate) fn cones(arch: &SyntaxNode) -> Vec<Cone> {
    let mut out = Vec::new();
    concurrent(arch, &mut out);
    for p in arch
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::PROCESS_STMT)
    {
        process(&p, &mut out);
    }
    out
}

fn concurrent(node: &SyntaxNode, out: &mut Vec<Cone>) {
    for child in node.children() {
        match child.kind() {
            SyntaxKind::SIGNAL_ASSIGN_STMT | SyntaxKind::SELECTED_ASSIGN_STMT => {
                let env = Env::default();
                if let Some((target, _, value)) = assignment(&child, &env) {
                    out.push(Cone {
                        target,
                        expr: simplify(value),
                        clock: None,
                        falling: false,
                    });
                }
            }
            SyntaxKind::FOR_GENERATE_STMT
            | SyntaxKind::IF_GENERATE_STMT
            | SyntaxKind::CASE_GENERATE_STMT
            | SyntaxKind::BLOCK_STMT
            | SyntaxKind::GENERATE_BODY
            | SyntaxKind::CASE_ALTERNATIVE => concurrent(&child, out),
            _ => {}
        }
    }
}

/// What one assignment set: key, spelling, value.
type Slot = (String, String, Expr);

/// The objects assigned so far in a branch, over those of the branches
/// around it: a branch only holds what it changes, so that nesting costs
/// what it assigns, not the size of the process.
#[derive(Debug, Clone, Copy, Default)]
struct Scope<'a> {
    parent: Option<&'a Env<'a>>,
    variables: &'a [String],
}

#[derive(Debug, Default)]
struct Env<'a> {
    slots: Vec<Slot>,
    scope: Scope<'a>,
}

impl<'a> Env<'a> {
    fn child(&'a self) -> Env<'a> {
        Env {
            slots: Vec::new(),
            scope: Scope {
                parent: Some(self),
                variables: self.scope.variables,
            },
        }
    }

    fn is_variable(&self, key: &str) -> bool {
        self.scope.variables.iter().any(|v| v == key)
    }

    fn get(&self, key: &str) -> Option<&Expr> {
        match self.slots.iter().find(|s| s.0 == key) {
            Some(slot) => Some(&slot.2),
            None => self.scope.parent.and_then(|p| p.get(key)),
        }
    }

    /// The value of `name` here: itself when nothing assigned it.
    fn current(&self, key: &str, name: &str) -> Expr {
        self.get(key)
            .cloned()
            .unwrap_or_else(|| Expr::Name(name.to_owned()))
    }

    fn set(&mut self, key: String, name: String, value: Expr) {
        match self.slots.iter_mut().find(|s| s.0 == key) {
            Some(slot) => slot.2 = value,
            None => self.slots.push((key, name, value)),
        }
    }
}

/// The value of `key` in a branch that changed `slots` of `env`.
fn value_in(slots: &[Slot], env: &Env, key: &str, name: &str) -> Expr {
    match slots.iter().find(|s| s.0 == key) {
        Some(slot) => slot.2.clone(),
        None => env.current(key, name),
    }
}

/// The keys `branches` assign, each once, in order.
fn keys_of<'s>(branches: impl Iterator<Item = &'s [Slot]>) -> Vec<(String, String)> {
    let mut keys: Vec<(String, String)> = Vec::new();
    for slots in branches {
        for (key, name, _) in slots {
            if !keys.iter().any(|k| &k.0 == key) {
                keys.push((key.clone(), name.clone()));
            }
        }
    }
    keys
}

fn process(node: &SyntaxNode, out: &mut Vec<Cone>) {
    if node
        .descendants()
        .any(|n| n.kind() == SyntaxKind::WAIT_STMT)
    {
        return;
    }
    let mut variables: Vec<String> = Vec::new();
    for decl in node
        .children()
        .filter(|n| n.kind() == SyntaxKind::VARIABLE_DECL)
    {
        for list in decl
            .children()
            .filter(|n| n.kind() == SyntaxKind::IDENT_LIST)
        {
            variables.extend(
                list.children_with_tokens()
                    .filter(|e| e.kind() == SyntaxKind::IDENT)
                    .map(|e| identifier_key(&e.to_string())),
            );
        }
    }
    let (body, clock): (Vec<SyntaxNode>, Option<(String, bool)>) = match clocked_body(node) {
        Some((stmts, clock)) => (stmts, Some(clock)),
        None => {
            let mut begun = false;
            let stmts = node
                .children_with_tokens()
                .filter(|e| {
                    begun |= e.kind() == SyntaxKind::BEGIN_KW;
                    begun
                })
                .filter_map(|e| e.into_node())
                .collect();
            (stmts, None)
        }
    };
    let mut env = Env {
        slots: Vec::new(),
        scope: Scope {
            parent: None,
            variables: &variables,
        },
    };
    statements(&body, &mut env);
    for (key, name, value) in env.slots {
        if !variables.contains(&key) {
            out.push(Cone {
                target: name,
                expr: simplify(value),
                clock: clock.as_ref().map(|c| c.0.clone()),
                falling: clock.as_ref().is_some_and(|c| c.1),
            });
        }
    }
}

/// The statements under the clock edge of a clocked process, and its
/// clock.
fn clocked_body(process: &SyntaxNode) -> Option<(Vec<SyntaxNode>, (String, bool))> {
    for stmt in process
        .descendants()
        .filter(|n| n.kind() == SyntaxKind::IF_STMT)
    {
        for (condition, stmts) in arms(&stmt) {
            if let Some(clock) = condition.as_ref().and_then(|c| {
                std::iter::once(c.clone())
                    .chain(c.descendants())
                    .find_map(|n| edge(&n))
            }) {
                return Some((stmts, clock));
            }
        }
    }
    None
}

/// The branches of an `if` statement: condition (none for `else`) and
/// statements.
fn arms(stmt: &SyntaxNode) -> Vec<(Option<SyntaxNode>, Vec<SyntaxNode>)> {
    let mut out: Vec<(Option<SyntaxNode>, Vec<SyntaxNode>)> = Vec::new();
    let mut want_condition = false;
    for element in stmt.children_with_tokens() {
        match element.kind() {
            SyntaxKind::IF_KW | SyntaxKind::ELSIF_KW => want_condition = true,
            SyntaxKind::ELSE_KW => out.push((None, Vec::new())),
            SyntaxKind::END_KW => break,
            _ => {
                if let Some(node) = element.into_node() {
                    if want_condition {
                        out.push((Some(node), Vec::new()));
                        want_condition = false;
                    } else if let Some(last) = out.last_mut() {
                        last.1.push(node);
                    }
                }
            }
        }
    }
    out
}

fn statements(stmts: &[SyntaxNode], env: &mut Env) {
    for stmt in stmts {
        match stmt.kind() {
            SyntaxKind::SIGNAL_ASSIGN_STMT
            | SyntaxKind::VARIABLE_ASSIGN_STMT
            | SyntaxKind::SELECTED_ASSIGN_STMT => {
                if let Some((name, partial, value)) = assignment(stmt, env) {
                    let key = identifier_key(&name);
                    let value = match partial {
                        Some(part) => {
                            Expr::Op(format!("{part} :="), vec![env.current(&key, &name), value])
                        }
                        None => value,
                    };
                    env.set(key, name, value);
                }
            }
            SyntaxKind::IF_STMT => if_statement(stmt, env),
            SyntaxKind::CASE_STMT => case_statement(stmt, env),
            // shortcut: a loop body is evaluated once, with the loop
            // parameter as a name; unroll it if diagrams need the bits.
            SyntaxKind::LOOP_STMT => {
                let body: Vec<SyntaxNode> = stmt
                    .children()
                    .filter(|n| n.kind() != SyntaxKind::FOR_SCHEME)
                    .collect();
                statements(&body, env);
            }
            _ => {}
        }
    }
}

fn if_statement(stmt: &SyntaxNode, env: &mut Env) {
    let mut branches: Vec<(Option<Expr>, Vec<Slot>)> = arms(stmt)
        .into_iter()
        .map(|(condition, stmts)| {
            let mut e = env.child();
            statements(&stmts, &mut e);
            (condition.map(|c| expr(&c, env)), e.slots)
        })
        .collect();
    if branches.last().is_none_or(|b| b.0.is_some()) {
        branches.push((None, Vec::new()));
    }
    // From the last branch up: each condition selects its branch or the
    // ones after it.
    let mut acc = branches.pop().map(|b| b.1).unwrap_or_default();
    while let Some((condition, slots)) = branches.pop() {
        let condition = condition.unwrap_or_else(|| Expr::Value("true".into()));
        let keys = keys_of([slots.as_slice(), acc.as_slice()].into_iter());
        acc = keys
            .into_iter()
            .map(|(key, name)| {
                let x = value_in(&slots, env, &key, &name);
                let y = value_in(&acc, env, &key, &name);
                let value = if x == y {
                    x
                } else {
                    Expr::Mux(
                        Box::new(condition.clone()),
                        vec![("1".into(), x), ("0".into(), y)],
                    )
                };
                (key, name, value)
            })
            .collect();
    }
    for (key, name, value) in acc {
        env.set(key, name, value);
    }
}

fn case_statement(stmt: &SyntaxNode, env: &mut Env) {
    let Some(select) = stmt
        .children()
        .find(|n| n.kind() != SyntaxKind::LABEL)
        .map(|n| expr(&n, env))
    else {
        return;
    };
    let mut branches: Vec<(String, Vec<Slot>)> = Vec::new();
    for alternative in stmt
        .children()
        .filter(|n| n.kind() == SyntaxKind::CASE_ALTERNATIVE)
    {
        let choices = alternative
            .children()
            .find(|n| n.kind() == SyntaxKind::CHOICES)
            .map(|c| compact(&c))
            .unwrap_or_default();
        let stmts: Vec<SyntaxNode> = alternative
            .children()
            .filter(|n| n.kind() != SyntaxKind::CHOICES)
            .collect();
        let mut e = env.child();
        statements(&stmts, &mut e);
        branches.push((choices, e.slots));
    }
    let keys = keys_of(branches.iter().map(|b| b.1.as_slice()));
    let mut out = Vec::with_capacity(keys.len());
    for (key, name) in keys {
        // Choices giving the same value share an input.
        let mut inputs: Vec<(String, Expr)> = Vec::new();
        for (choices, slots) in &branches {
            let value = value_in(slots, env, &key, &name);
            match inputs.iter_mut().find(|i| i.1 == value) {
                Some(input) => {
                    input.0.push_str(" | ");
                    input.0.push_str(choices);
                }
                None => inputs.push((choices.clone(), value)),
            }
        }
        let value = if inputs.len() == 1 {
            inputs.remove(0).1
        } else {
            Expr::Mux(Box::new(select.clone()), inputs)
        };
        out.push((key, name, value));
    }
    for (key, name, value) in out {
        env.set(key, name, value);
    }
}

/// The object an assignment sets, the part of it (`(3)`) when it sets
/// only a part, and the value.
fn assignment(stmt: &SyntaxNode, env: &Env) -> Option<(String, Option<String>, Expr)> {
    let mut nodes = Vec::new();
    let mut after = None;
    for element in stmt.children_with_tokens() {
        match element.kind() {
            SyntaxKind::LTE | SyntaxKind::COLON_EQ => after = Some(nodes.len()),
            SyntaxKind::LABEL => {}
            _ => {
                if let Some(n) = element.into_node() {
                    nodes.push(n);
                }
            }
        }
    }
    let after = after?;
    let target = nodes.get(after.checked_sub(1)?)?;
    let name = base_name(target)?;
    let text = compact(target);
    let partial = (target.kind() != SyntaxKind::NAME)
        .then(|| text.get(name.len()..).unwrap_or_default().to_owned());
    let value = nodes.get(after)?;
    let value = if stmt.kind() == SyntaxKind::SELECTED_ASSIGN_STMT {
        let select = expr(nodes.first()?, env);
        selected(value, select, env)
    } else {
        waveforms(value, &name, env)
    };
    Some((name, partial, value))
}

/// A waveform, a conditional one, or an expression.
fn waveforms(node: &SyntaxNode, target: &str, env: &Env) -> Expr {
    match node.kind() {
        SyntaxKind::WAVEFORM => node
            .children()
            .next()
            .and_then(|element| element.children().next())
            .map_or_else(|| Expr::Name(target.to_owned()), |e| expr(&e, env)),
        SyntaxKind::CONDITIONAL_WAVEFORMS | SyntaxKind::CONDITIONAL_EXPR => {
            // value when condition else value when condition else value
            let parts: Vec<SyntaxNode> = node.children().collect();
            let mut acc = if parts.len() % 2 == 1 {
                waveforms(&parts[parts.len() - 1], target, env)
            } else {
                Expr::Name(target.to_owned())
            };
            for pair in parts.chunks_exact(2).rev() {
                acc = Expr::Mux(
                    Box::new(expr(&pair[1], env)),
                    vec![
                        ("1".into(), waveforms(&pair[0], target, env)),
                        ("0".into(), acc),
                    ],
                );
            }
            acc
        }
        _ => expr(node, env),
    }
}

/// `with select select target <= value when choices, ...`.
fn selected(node: &SyntaxNode, select: Expr, env: &Env) -> Expr {
    let parts: Vec<SyntaxNode> = node.children().collect();
    let mut inputs: Vec<(String, Expr)> = Vec::new();
    for pair in parts.chunks_exact(2) {
        let value = waveforms(&pair[0], "", env);
        let choices = compact(&pair[1]);
        match inputs.iter_mut().find(|i| i.1 == value) {
            Some(input) => {
                input.0.push_str(" | ");
                input.0.push_str(&choices);
            }
            None => inputs.push((choices, value)),
        }
    }
    Expr::Mux(Box::new(select), inputs)
}

fn operator(node: &SyntaxNode) -> String {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| {
            !matches!(
                t.kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT
            )
        })
        .map(|t| t.text().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Operators whose nested uses flatten into one operator with more inputs.
fn associative(op: &str) -> bool {
    matches!(op, "and" | "or" | "xor" | "&" | "+" | "*")
}

fn expr(node: &SyntaxNode, env: &Env) -> Expr {
    match node.kind() {
        SyntaxKind::BINARY_EXPR => {
            let op = operator(node);
            let mut args = Vec::new();
            for child in node.children() {
                match expr(&child, env) {
                    Expr::Op(inner, more) if inner == op && associative(&op) => args.extend(more),
                    e => args.push(e),
                }
            }
            Expr::Op(op, args)
        }
        SyntaxKind::UNARY_EXPR => Expr::Op(
            operator(node),
            node.children().map(|c| expr(&c, env)).collect(),
        ),
        SyntaxKind::PAREN_EXPR => node
            .children()
            .next()
            .map_or_else(|| Expr::Value(compact(node)), |c| expr(&c, env)),
        SyntaxKind::NAME => {
            let text = compact(node);
            let key = identifier_key(&text);
            match env.get(&key) {
                // A variable built on itself (`v := v + v`) would double each
                // time: past the size of a drawable tree it stays a name.
                Some(value) if env.is_variable(&key) && value.size() <= MAX_NODES => value.clone(),
                _ => Expr::Name(text),
            }
        }
        SyntaxKind::SELECTED_NAME => Expr::Name(compact(node)),
        SyntaxKind::CALL_EXPR => {
            let name = node
                .children()
                .next()
                .map(|c| compact(&c))
                .unwrap_or_default();
            let args = node
                .children()
                .filter(|n| n.kind() == SyntaxKind::ASSOCIATION_LIST)
                .flat_map(|list| list.children())
                .filter_map(|element| element.children().last())
                .map(|e| expr(&e, env))
                .collect();
            Expr::Call(name, args, compact(node))
        }
        SyntaxKind::QUALIFIED_EXPR => node
            .children()
            .last()
            .map_or_else(|| Expr::Value(compact(node)), |c| expr(&c, env)),
        SyntaxKind::AGGREGATE if node.descendants().any(|n| n.kind() == SyntaxKind::NAME) => {
            Expr::Op(
                "( )".into(),
                node.children()
                    .filter_map(|e| {
                        if e.kind() == SyntaxKind::ELEMENT_ASSOCIATION {
                            e.children().last()
                        } else {
                            Some(e)
                        }
                    })
                    .map(|e| expr(&e, env))
                    .collect(),
            )
        }
        _ => Expr::Value(compact(node)),
    }
}

/// Type conversions and resizes, which carry their first argument
/// through.
pub fn is_conversion(name: &str) -> bool {
    let name = name.rsplit('.').next().unwrap_or(name).to_ascii_lowercase();
    matches!(
        name.as_str(),
        "unsigned"
            | "signed"
            | "std_logic_vector"
            | "std_ulogic_vector"
            | "to_unsigned"
            | "to_signed"
            | "to_integer"
            | "resize"
            | "integer"
            | "natural"
            | "to_slv"
            | "to_stdlogicvector"
            | "to_stdulogicvector"
            | "to_bitvector"
            | "to_01"
    )
}

fn bit(e: &Expr, value: &str) -> bool {
    matches!(e, Expr::Value(v) if v == value)
}

/// Rewrites what synthesis would: `'1' when c else '0'` is `c`, `s = '1'`
/// is `s`, `s = '0'` is `not s`, `not not s` is `s`.
pub(crate) fn simplify(e: Expr) -> Expr {
    match e {
        Expr::Op(op, args) => {
            let mut args: Vec<Expr> = args.into_iter().map(simplify).collect();
            if op == "=" && args.len() == 2 {
                for (one, other) in [(1, 0), (0, 1)] {
                    if bit(&args[one], "'1'") {
                        return args.swap_remove(other);
                    }
                    if bit(&args[one], "'0'") {
                        return not(args.swap_remove(other));
                    }
                }
            }
            if op == "not" && args.len() == 1 {
                return not(args.remove(0));
            }
            Expr::Op(op, args)
        }
        Expr::Call(name, args, text) => {
            Expr::Call(name, args.into_iter().map(simplify).collect(), text)
        }
        Expr::Mux(select, arms) => {
            let select = simplify(*select);
            let arms: Vec<(String, Expr)> =
                arms.into_iter().map(|(c, e)| (c, simplify(e))).collect();
            if arms.len() == 2 && arms[0].0 == "1" && arms[1].0 == "0" {
                if bit(&arms[0].1, "'1'") && bit(&arms[1].1, "'0'") {
                    return select;
                }
                if bit(&arms[0].1, "'0'") && bit(&arms[1].1, "'1'") {
                    return not(select);
                }
            }
            Expr::Mux(Box::new(select), arms)
        }
        e => e,
    }
}

fn not(e: Expr) -> Expr {
    match e {
        Expr::Op(op, mut args) if op == "not" && args.len() == 1 => args.remove(0),
        e => Expr::Op("not".into(), vec![e]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cones_of(body: &str) -> Vec<Cone> {
        let text = format!("architecture a of e is\nbegin\n{body}\nend;\n");
        let parse = vfl_syntax::parse(&text, vfl_syntax::Standard::Vhdl2008);
        let root = parse.syntax();
        let arch = root
            .descendants()
            .find(|n| n.kind() == SyntaxKind::ARCHITECTURE_BODY)
            .unwrap();
        cones(&arch)
    }

    fn name(s: &str) -> Expr {
        Expr::Name(s.into())
    }

    fn op(o: &str, args: Vec<Expr>) -> Expr {
        Expr::Op(o.into(), args)
    }

    fn mux2(s: Expr, a: Expr, b: Expr) -> Expr {
        Expr::Mux(Box::new(s), vec![("1".into(), a), ("0".into(), b)])
    }

    #[test]
    fn concurrent_assignments_become_trees() {
        let c = cones_of(
            "y <= (a and b and c) or not d;\n\
             r <= '1' when s = '1' and v = '0' else '0';\n\
             z <= a when t else b;",
        );
        assert_eq!(c[0].target, "y");
        assert_eq!(
            c[0].expr,
            op(
                "or",
                vec![
                    op("and", vec![name("a"), name("b"), name("c")]),
                    op("not", vec![name("d")])
                ]
            )
        );
        assert_eq!(
            c[1].expr,
            op("and", vec![name("s"), op("not", vec![name("v")])])
        );
        assert_eq!(c[2].expr, mux2(name("t"), name("a"), name("b")));
    }

    #[test]
    fn processes_merge_branches_into_multiplexers() {
        let c = cones_of(
            "p : process (clk, rst) is\n variable v : integer;\nbegin\n\
             if rst = '1' then q <= '0';\n\
             elsif rising_edge(clk) then\n\
               v := a + 1;\n\
               if en = '1' then q <= v; end if;\n\
               case s is when X => n <= a; when Y | Z => n <= b; when others => n <= a; end case;\n\
             end if;\nend process;",
        );
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].target, "q");
        assert_eq!(c[0].clock.as_deref(), Some("clk"));
        assert_eq!(
            c[0].expr,
            mux2(
                name("en"),
                op("+", vec![name("a"), Expr::Value("1".into())]),
                name("q")
            )
        );
        let Expr::Mux(_, arms) = &c[1].expr else {
            panic!("{:?}", c[1].expr)
        };
        assert_eq!(arms[0].0, "X | others");
        assert_eq!(arms[1].0, "Y | Z");
    }
}
