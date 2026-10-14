//! Semantic tokens: names colored by what the analysis resolved them to
//! (signal, port, constant, type, ...) rather than by the grammar.

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens, SemanticTokensDelta,
    SemanticTokensDeltaParams, SemanticTokensEdit, SemanticTokensFullDeltaResult,
    SemanticTokensLegend, SemanticTokensParams, SemanticTokensRangeParams,
    SemanticTokensRangeResult, SemanticTokensResult,
};
use vfl_sema::{Decl, DeclKind, FileId, Mode, Target};
use vfl_syntax::{LineIndex, TextRange, TextSize};

use crate::world::{Doc, World};

/// What a name is, as colored. The order is the legend's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A library, package or context.
    Namespace,
    /// A type or subtype.
    Type,
    /// A generic type.
    TypeParameter,
    /// An entity, architecture or configuration.
    Entity,
    Component,
    Function,
    Procedure,
    EnumMember,
    Variable,
    /// A subprogram parameter.
    Parameter,
    Signal,
    Port,
    Constant,
    Generic,
    Label,
    /// A physical unit, like `ns`.
    Unit,
    /// A user-defined attribute.
    Attribute,
}

impl Kind {
    pub const ALL: [Kind; 17] = [
        Kind::Namespace,
        Kind::Type,
        Kind::TypeParameter,
        Kind::Entity,
        Kind::Component,
        Kind::Function,
        Kind::Procedure,
        Kind::EnumMember,
        Kind::Variable,
        Kind::Parameter,
        Kind::Signal,
        Kind::Port,
        Kind::Constant,
        Kind::Generic,
        Kind::Label,
        Kind::Unit,
        Kind::Attribute,
    ];

    /// The token type's name in the legend. Types LSP does not define are
    /// declared by the VS Code extension, each with a standard fallback.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Namespace => "namespace",
            Kind::Type => "type",
            Kind::TypeParameter => "typeParameter",
            Kind::Entity => "class",
            Kind::Component => "interface",
            Kind::Function => "function",
            Kind::Procedure => "procedure",
            Kind::EnumMember => "enumMember",
            Kind::Variable => "variable",
            Kind::Parameter => "parameter",
            Kind::Signal => "signal",
            Kind::Port => "port",
            Kind::Constant => "constant",
            Kind::Generic => "generic",
            Kind::Label => "label",
            Kind::Unit => "unit",
            Kind::Attribute => "attribute",
        }
    }
}

/// Token modifiers, as bits in the legend's order.
pub mod modifier {
    pub const DECLARATION: u32 = 1 << 0;
    /// Constants, generics, loop parameters, and ports and parameters of
    /// mode `in`.
    pub const READONLY: u32 = 1 << 1;
    /// Declared in the bundled `std` and `ieee` libraries.
    pub const DEFAULT_LIBRARY: u32 = 1 << 2;
    /// Written here: an assignment target, an `out` actual.
    pub const MODIFICATION: u32 = 1 << 3;
    pub const IN: u32 = 1 << 4;
    pub const OUT: u32 = 1 << 5;
    pub const INOUT: u32 = 1 << 6;
    pub const BUFFER: u32 = 1 << 7;
    pub const LINKAGE: u32 = 1 << 8;

    pub const NAMES: [&str; 9] = [
        "declaration",
        "readonly",
        "defaultLibrary",
        "modification",
        "in",
        "out",
        "inout",
        "buffer",
        "linkage",
    ];
}

/// A colored name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub range: TextRange,
    pub kind: Kind,
    /// [`modifier`] bits.
    pub modifiers: u32,
}

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: Kind::ALL
            .iter()
            .map(|k| SemanticTokenType::new(k.name()))
            .collect(),
        token_modifiers: modifier::NAMES
            .iter()
            .map(|m| SemanticTokenModifier::new(m))
            .collect(),
    }
}

/// The kind and modifiers of a declaration's name.
fn classify(decl: &Decl, builtin: bool) -> Option<(Kind, u32)> {
    use modifier::*;
    let mode = |mode: Mode| match mode {
        Mode::In => IN | READONLY,
        Mode::Out => OUT,
        Mode::InOut => INOUT,
        Mode::Buffer => BUFFER,
        Mode::Linkage => LINKAGE,
        Mode::View => 0,
    };
    let (kind, modifiers) = match decl.kind {
        DeclKind::Package
        | DeclKind::PackageBody
        | DeclKind::PackageInstance
        | DeclKind::GenericPackage
        | DeclKind::Context => (Kind::Namespace, 0),
        DeclKind::Entity | DeclKind::Architecture | DeclKind::Configuration => (Kind::Entity, 0),
        DeclKind::Component => (Kind::Component, 0),
        DeclKind::Type
        | DeclKind::IncompleteType
        | DeclKind::ProtectedBody
        | DeclKind::Subtype
        | DeclKind::View => (Kind::Type, 0),
        DeclKind::GenericType => (Kind::TypeParameter, 0),
        DeclKind::Function | DeclKind::GenericSubprogram => (Kind::Function, 0),
        DeclKind::Procedure => (Kind::Procedure, 0),
        DeclKind::EnumLiteral => (Kind::EnumMember, 0),
        DeclKind::PhysicalUnit => (Kind::Unit, 0),
        DeclKind::Signal => (Kind::Signal, 0),
        DeclKind::Variable | DeclKind::File => (Kind::Variable, 0),
        DeclKind::LoopParameter | DeclKind::GenerateParameter => (Kind::Variable, READONLY),
        DeclKind::Constant => (Kind::Constant, READONLY),
        DeclKind::Generic { .. } => (Kind::Generic, READONLY),
        DeclKind::Port { mode: m, .. } => (Kind::Port, mode(m)),
        DeclKind::Parameter { mode: m, .. } => (Kind::Parameter, mode(m)),
        DeclKind::Label => (Kind::Label, 0),
        DeclKind::Attribute => (Kind::Attribute, 0),
        // An object alias reads like a variable; what other aliases stand
        // for is not recorded.
        DeclKind::Alias if decl.subtype.is_some() => (Kind::Variable, 0),
        DeclKind::Alias | DeclKind::Group => return None,
    };
    Some((kind, modifiers | if builtin { DEFAULT_LIBRARY } else { 0 }))
}

/// Whether a name is worth coloring: operators (`"+"`) and character
/// literals (`'0'`) are left to the grammar.
fn colorable(decl: &Decl) -> bool {
    !decl.name.starts_with(['"', '\'']) && !decl.spelling.is_empty()
}

/// The tokens of a file, in order, within `within` when given.
pub fn tokens(world: &World, file: FileId, within: Option<TextRange>) -> Vec<Token> {
    let analysis = &world.analysis;
    let text = world.doc(file).text;
    let keep = |r: TextRange| {
        within.is_none_or(|w| r.start() < w.end() && w.start() < r.end())
            // A token spans one line.
            && !text[r].contains('\n')
    };
    // By start offset: a file in several libraries has its units, and so
    // its names, once per library; a declaration wins over a reference.
    let mut by_start: HashMap<TextSize, Token> = HashMap::new();
    let builtin = |f: FileId| analysis.files().get(f as usize).is_some_and(|f| f.builtin);
    for unit in analysis.file_units(file) {
        for decl in analysis.unit_decls(unit) {
            if decl.file != file || !colorable(decl) || decl.range.is_empty() || !keep(decl.range) {
                continue;
            }
            if let Some((kind, modifiers)) = classify(decl, builtin(decl.file)) {
                by_start.insert(
                    decl.range.start(),
                    Token {
                        range: decl.range,
                        kind,
                        modifiers: modifiers | modifier::DECLARATION,
                    },
                );
            }
        }
    }
    for unit in analysis.file_units(file) {
        for reference in analysis.unit_references(unit) {
            if reference.range.is_empty()
                || by_start.contains_key(&reference.range.start())
                || !keep(reference.range)
            {
                continue;
            }
            let class = reference.targets.iter().find_map(|t| match t {
                Target::Library(_) => Some((Kind::Namespace, 0)),
                Target::Decl(id) => {
                    let decl = analysis.decl(*id)?;
                    if !colorable(decl) {
                        return None;
                    }
                    classify(decl, builtin(decl.file))
                }
            });
            let Some((kind, mut modifiers)) = class else {
                continue;
            };
            if reference.access.writes() {
                modifiers |= modifier::MODIFICATION;
            }
            by_start.insert(
                reference.range.start(),
                Token {
                    range: reference.range,
                    kind,
                    modifiers,
                },
            );
        }
    }
    let mut tokens: Vec<Token> = by_start.into_values().collect();
    tokens.sort_unstable_by_key(|t| t.range.start());
    // Drop the odd token that overlaps the one before it.
    let mut end = TextSize::from(0);
    tokens.retain(|t| {
        let keep = t.range.start() >= end;
        if keep {
            end = t.range.end();
        }
        keep
    });
    tokens
}

/// The tokens in LSP's relative encoding: lines and UTF-16 columns.
fn encode(doc: Doc, tokens: &[Token]) -> Vec<SemanticToken> {
    let mut out = Vec::with_capacity(tokens.len());
    let (mut line, mut column) = (0, 0);
    for token in tokens {
        let start = doc.position(token.range.start());
        let length = doc.text[token.range].encode_utf16().count() as u32;
        let delta_line = start.line - line;
        let delta_start = if delta_line == 0 {
            start.character - column
        } else {
            start.character
        };
        out.push(SemanticToken {
            delta_line,
            delta_start,
            length,
            token_type: token.kind as u32,
            token_modifiers_bitset: token.modifiers,
        });
        line = start.line;
        column = start.character;
    }
    out
}

/// The last tokens sent for each file, for answering with a delta.
#[derive(Default)]
pub struct Sent {
    files: HashMap<PathBuf, (String, Vec<SemanticToken>)>,
    next_id: u64,
}

impl Sent {
    fn remember(&mut self, path: PathBuf, data: Vec<SemanticToken>) -> String {
        self.next_id += 1;
        let id = self.next_id.to_string();
        self.files.insert(path, (id.clone(), data));
        id
    }

    pub fn forget(&mut self, path: &std::path::Path) {
        self.files.remove(path);
    }
}

fn file_tokens(world: &World, uri: &lsp_types::Url) -> Option<(PathBuf, Vec<SemanticToken>)> {
    let path = uri.to_file_path().ok()?;
    let data = encoded(world, &path, None)?;
    Some((path, data))
}

/// The encoded tokens of the file at `path`, within a range of positions
/// when given.
fn encoded(
    world: &World,
    path: &std::path::Path,
    within: Option<lsp_types::Range>,
) -> Option<Vec<SemanticToken>> {
    let range = |doc: Doc| within.map(|r| TextRange::new(doc.offset(r.start), doc.offset(r.end)));
    if let Some(file) = world.file_id(path) {
        let doc = world.doc(file);
        return Some(encode(doc, &tokens(world, file, range(doc))));
    }
    let file = crate::verilog::file(world, path)?;
    let index = LineIndex::new(&file.text);
    let doc = Doc {
        text: &file.text,
        index: &index,
    };
    let tokens = crate::verilog::tokens(&file.parse.syntax(), range(doc));
    Some(encode(doc, &tokens))
}

pub fn full(world: &mut World, params: SemanticTokensParams) -> Option<SemanticTokensResult> {
    let (path, data) = file_tokens(world, &params.text_document.uri)?;
    let result_id = world.semantic.remember(path, data.clone());
    Some(SemanticTokensResult::Tokens(SemanticTokens {
        result_id: Some(result_id),
        data,
    }))
}

pub fn delta(
    world: &mut World,
    params: SemanticTokensDeltaParams,
) -> Option<SemanticTokensFullDeltaResult> {
    let (path, data) = file_tokens(world, &params.text_document.uri)?;
    let previous = world
        .semantic
        .files
        .get(&path)
        .filter(|(id, _)| *id == params.previous_result_id)
        .map(|(_, data)| data.clone());
    let result_id = world.semantic.remember(path, data.clone());
    let Some(old) = previous else {
        return Some(SemanticTokensFullDeltaResult::Tokens(SemanticTokens {
            result_id: Some(result_id),
            data,
        }));
    };
    // One edit replacing what changed between the common prefix and
    // suffix; the edit counts integers, five per token.
    let prefix = old.iter().zip(&data).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(data[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let edits = if prefix == old.len() && old.len() == data.len() {
        Vec::new()
    } else {
        vec![SemanticTokensEdit {
            start: 5 * prefix as u32,
            delete_count: 5 * (old.len() - prefix - suffix) as u32,
            data: Some(data[prefix..data.len() - suffix].to_vec()),
        }]
    };
    Some(SemanticTokensFullDeltaResult::TokensDelta(
        SemanticTokensDelta {
            result_id: Some(result_id),
            edits,
        },
    ))
}

pub fn range(
    world: &mut World,
    params: SemanticTokensRangeParams,
) -> Option<SemanticTokensRangeResult> {
    let path = params.text_document.uri.to_file_path().ok()?;
    let data = encoded(world, &path, Some(params.range))?;
    Some(SemanticTokensRangeResult::Tokens(SemanticTokens {
        result_id: None,
        data,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `text` with each token's kind and modifiers after it.
    fn annotated(text: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.vhd");
        std::fs::write(&path, text).unwrap();
        let mut world = World::new(dir.path());
        world.refresh();
        let file = world.file_id(&path).unwrap();
        let mut out = String::new();
        for token in tokens(&world, file, None) {
            let modifiers: Vec<&str> = modifier::NAMES
                .iter()
                .enumerate()
                .filter(|(i, _)| token.modifiers & (1 << i) != 0)
                .map(|(_, m)| *m)
                .collect();
            out.push_str(&format!(
                "{} {}{}\n",
                &text[token.range],
                token.kind.name(),
                modifiers
                    .iter()
                    .map(|m| format!(" {m}"))
                    .collect::<String>()
            ));
        }
        out
    }

    #[test]
    fn names_by_kind() {
        let text = "\
library ieee;
use ieee.std_logic_1164.all;
entity counter is
  generic (WIDTH : natural := 8);
  port (clk : in std_logic; q : out std_logic_vector(WIDTH - 1 downto 0));
end entity;
architecture rtl of counter is
  type state_t is (idle, run);
  constant ZERO : integer := 0;
  signal state : state_t;
  function inc(x : integer) return integer is begin return x + 1; end function;
begin
  main : process (clk)
    variable n : integer := ZERO;
  begin
    if rising_edge(clk) then
      n := inc(n);
      state <= run;
    end if;
  end process;
  q <= (others => '0');
end architecture;
";
        let out = annotated(text);
        for expected in [
            "ieee namespace\n",
            "std_logic_1164 namespace defaultLibrary\n",
            "counter class declaration\n",
            "WIDTH generic declaration readonly\n",
            "clk port declaration readonly in\n",
            "std_logic type defaultLibrary\n",
            "q port declaration out\n",
            "WIDTH generic readonly\n",
            "state_t type declaration\n",
            "idle enumMember declaration\n",
            "ZERO constant declaration readonly\n",
            "state signal declaration\n",
            "inc function declaration\n",
            "x parameter declaration readonly in\n",
            "main label declaration\n",
            "n variable declaration\n",
            "rising_edge function defaultLibrary\n",
            "n variable modification\n",
            "state signal modification\n",
            "run enumMember\n",
            "q port modification out\n",
        ] {
            assert!(out.contains(expected), "missing {expected:?} in\n{out}");
        }
    }
}
