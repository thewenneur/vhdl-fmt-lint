//! Type-aware editor features: types in hovers, signature help, and
//! completion of record elements and protected type methods.

use lsp_types::{
    CompletionItemKind, ParameterInformation, ParameterLabel, SignatureHelp, SignatureHelpParams,
    SignatureInformation,
};
use vfl_sema::{Analysis, Decl, DeclId, DeclKind, FileId, Reference, Target, TypeDef};
use vfl_syntax::{SyntaxKind, SyntaxToken, TextRange, TextSize};

use crate::world::World;

/// How far back signature help looks for the opening parenthesis.
const MAX_TOKENS: usize = 4000;

/// The resolved reference with exactly this range, in `file`.
pub fn reference_at(analysis: &Analysis, file: FileId, range: TextRange) -> Option<&Reference> {
    analysis.file_units(file).find_map(|unit| {
        let refs = analysis.unit_references(unit);
        let i = refs.partition_point(|r| r.range.start() < range.start());
        refs[i..]
            .iter()
            .take_while(|r| r.range.start() == range.start())
            .find(|r| r.range == range)
    })
}

fn decl_targets(reference: &Reference) -> impl Iterator<Item = DeclId> + '_ {
    reference.targets.iter().filter_map(|t| match t {
        Target::Decl(id) => Some(*id),
        Target::Library(_) => None,
    })
}

fn is_subprogram(kind: DeclKind) -> bool {
    matches!(
        kind,
        DeclKind::Function | DeclKind::Procedure | DeclKind::GenericSubprogram
    )
}

/// The base type of the value an object, alias or function result has.
fn value_type(analysis: &Analysis, id: DeclId) -> Option<DeclId> {
    let decl = analysis.decl(id)?;
    if decl.kind.is_object()
        || matches!(
            decl.kind,
            DeclKind::Alias | DeclKind::Function | DeclKind::EnumLiteral | DeclKind::PhysicalUnit
        )
    {
        analysis.base_type(decl.subtype?)
    } else {
        None
    }
}

fn type_def(analysis: &Analysis, ty: DeclId) -> Option<&TypeDef> {
    analysis.decl(ty)?.type_def.as_deref()
}

/// The type an access type designates, or the type itself.
fn designated(analysis: &Analysis, ty: DeclId) -> DeclId {
    match type_def(analysis, ty) {
        Some(TypeDef::Access(Some(d))) => analysis.base_type(*d).unwrap_or(ty),
        _ => ty,
    }
}

fn spelling(analysis: &Analysis, id: Option<DeclId>) -> String {
    id.and_then(|id| analysis.decl(id))
        .map_or_else(|| "?".to_owned(), |d| d.spelling.clone())
}

/// A type mark as shown to users: `byte (subtype of std_logic_vector)`.
pub fn mark_text(analysis: &Analysis, mark: DeclId) -> String {
    let name = spelling(analysis, Some(mark));
    match analysis.base_type(mark) {
        Some(base) if base != mark => {
            format!("`{name}` (subtype of `{}`)", spelling(analysis, Some(base)))
        }
        _ => format!("`{name}`"),
    }
}

/// What a type is, in a few lines of Markdown.
pub fn type_summary(analysis: &Analysis, ty: DeclId) -> Option<String> {
    let decl = analysis.decl(ty)?;
    Some(match decl.type_def.as_deref()? {
        TypeDef::Enum(literals) => {
            let mut shown: Vec<&str> = literals.iter().take(16).map(String::as_str).collect();
            if literals.len() > shown.len() {
                shown.push("...");
            }
            format!("enumeration: {}", shown.join(", "))
        }
        TypeDef::Integer => "integer type".into(),
        TypeDef::Real => "floating point type".into(),
        TypeDef::Physical => "physical type".into(),
        TypeDef::Scalar => "scalar type".into(),
        TypeDef::Array { indexes, element } => {
            let indexes: Vec<String> = indexes
                .iter()
                .map(|i| format!("`{}`", spelling(analysis, *i)))
                .collect();
            format!(
                "array ({}) of `{}`",
                indexes.join(", "),
                spelling(analysis, *element)
            )
        }
        TypeDef::Record(elements) => {
            let mut text = "record:".to_owned();
            for e in elements.iter().take(32) {
                text.push_str(&format!(
                    "\n- `{}` : `{}`",
                    e.spelling,
                    spelling(analysis, e.subtype)
                ));
            }
            if elements.len() > 32 {
                text.push_str("\n- ...");
            }
            text
        }
        TypeDef::Access(d) => format!("access to `{}`", spelling(analysis, *d)),
        TypeDef::File(d) => format!("file of `{}`", spelling(analysis, *d)),
        TypeDef::Protected => {
            let methods: Vec<String> = decl
                .members
                .iter()
                .flat_map(|m| m.iter())
                .filter_map(|m| analysis.decl(m))
                .filter(|d| is_subprogram(d.kind))
                .map(|d| format!("`{}`", d.spelling))
                .collect();
            let mut unique = methods.clone();
            unique.dedup();
            format!("protected type; methods: {}", unique.join(", "))
        }
    })
}

/// The type lines of a hover over declaration `id`.
pub fn hover_lines(analysis: &Analysis, id: DeclId) -> Option<String> {
    let decl = analysis.decl(id)?;
    match decl.kind {
        DeclKind::Type => type_summary(analysis, id),
        DeclKind::Subtype => {
            let base = analysis.base_type(decl.subtype?)?;
            let mut text = format!("subtype of `{}`", spelling(analysis, Some(base)));
            if let Some(summary) = type_summary(analysis, base) {
                text.push_str("\n\n");
                text.push_str(&summary);
            }
            Some(text)
        }
        DeclKind::Function => {
            let ret = decl.subtype?;
            Some(format!("returns {}", mark_text(analysis, ret)))
        }
        DeclKind::EnumLiteral | DeclKind::PhysicalUnit => {
            Some(format!("of type `{}`", spelling(analysis, decl.subtype)))
        }
        kind if kind.is_object() || kind == DeclKind::Alias => {
            let mark = decl.subtype?;
            let mut text = format!("type {}", mark_text(analysis, mark));
            if let Some(summary) = analysis
                .base_type(mark)
                .and_then(|t| type_summary(analysis, t))
            {
                text.push_str("\n\n");
                text.push_str(&summary);
            }
            Some(text)
        }
        _ => None,
    }
}

fn significant(token: Option<SyntaxToken>) -> Option<SyntaxToken> {
    let mut token = token?;
    while token.kind().is_trivia() {
        token = token.prev_token()?;
    }
    Some(token)
}

fn is_ident(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::IDENT | SyntaxKind::EXT_IDENT)
}

/// The base type of the value a name ending with `last` has: `a`, `a.b`,
/// `a(i).b`, `p.all`, `f(x)`.
fn name_type(analysis: &Analysis, file: FileId, last: &SyntaxToken, depth: u32) -> Option<DeclId> {
    if depth > 32 {
        return None;
    }
    match last.kind() {
        kind if is_ident(kind) => {
            if let Some(reference) = reference_at(analysis, file, last.text_range()) {
                return decl_targets(reference).find_map(|id| value_type(analysis, id));
            }
            // An element of a record.
            let dot = significant(last.prev_token())?;
            if dot.kind() != SyntaxKind::DOT {
                return None;
            }
            let prefix = significant(dot.prev_token())?;
            let ty = designated(analysis, name_type(analysis, file, &prefix, depth + 1)?);
            let key = vfl_syntax::identifier_key(last.text());
            match type_def(analysis, ty)? {
                TypeDef::Record(elements) => elements
                    .iter()
                    .find(|e| e.name == key)
                    .and_then(|e| analysis.base_type(e.subtype?)),
                TypeDef::Protected => analysis
                    .decl(ty)?
                    .members
                    .as_ref()?
                    .get(&key)
                    .iter()
                    .find_map(|&m| value_type(analysis, m)),
                _ => None,
            }
        }
        SyntaxKind::ALL_KW => {
            let dot = significant(last.prev_token())?;
            let prefix = significant(dot.prev_token())?;
            let ty = name_type(analysis, file, &prefix, depth + 1)?;
            match type_def(analysis, ty)? {
                TypeDef::Access(Some(d)) => analysis.base_type(*d),
                _ => None,
            }
        }
        SyntaxKind::RPAREN => {
            // Skip to the matching parenthesis.
            let mut level = 0u32;
            let mut token = last.clone();
            loop {
                match token.kind() {
                    SyntaxKind::RPAREN => level += 1,
                    SyntaxKind::LPAREN => {
                        level -= 1;
                        if level == 0 {
                            break;
                        }
                    }
                    SyntaxKind::SEMI => return None,
                    _ => {}
                }
                token = token.prev_token()?;
            }
            let prefix = significant(token.prev_token())?;
            if is_ident(prefix.kind())
                && let Some(reference) = reference_at(analysis, file, prefix.text_range())
                && decl_targets(reference).any(|id| {
                    analysis
                        .decl(id)
                        .is_some_and(|d| d.kind == DeclKind::Function)
                })
            {
                // A function call.
                return decl_targets(reference).find_map(|id| value_type(analysis, id));
            }
            let ty = designated(analysis, name_type(analysis, file, &prefix, depth + 1)?);
            match type_def(analysis, ty)? {
                TypeDef::Array {
                    element: Some(e), ..
                } => analysis.base_type(*e),
                _ => None,
            }
        }
        _ => None,
    }
}

/// A completion candidate: label, kind, detail.
pub type Member = (String, CompletionItemKind, String);

/// What can follow `name.` when `name` is a value: record elements,
/// protected type methods, `all` for access values. `None` when the name
/// before the dot is not a value of a known type.
pub fn members_after_dot(world: &World, file: FileId, dot: TextSize) -> Option<Vec<Member>> {
    let analysis = &world.analysis;
    let root = analysis.files().get(file as usize)?.syntax();
    let dot_token = root.token_at_offset(dot).right_biased()?;
    if dot_token.kind() != SyntaxKind::DOT {
        return None;
    }
    let prefix = significant(dot_token.prev_token())?;
    let ty = name_type(analysis, file, &prefix, 0)?;
    let mut members = Vec::new();
    let target = designated(analysis, ty);
    if target != ty {
        members.push(("all".to_owned(), CompletionItemKind::KEYWORD, String::new()));
    }
    match type_def(analysis, target) {
        Some(TypeDef::Record(elements)) => {
            for e in elements {
                members.push((
                    e.spelling.clone(),
                    CompletionItemKind::FIELD,
                    spelling(analysis, e.subtype),
                ));
            }
        }
        Some(TypeDef::Protected) => {
            for m in analysis.decl(target)?.members.iter().flat_map(|m| m.iter()) {
                if let Some(decl) = analysis.decl(m).filter(|d| is_subprogram(d.kind)) {
                    let kind = if decl.kind == DeclKind::Procedure {
                        CompletionItemKind::METHOD
                    } else {
                        CompletionItemKind::FUNCTION
                    };
                    members.push((decl.spelling.clone(), kind, signature(world, decl, None).0));
                }
            }
        }
        _ => {}
    }
    Some(members)
}

/// Hover text for a record element named at `offset`, if any.
pub fn element_hover(world: &World, file: FileId, offset: TextSize) -> Option<(TextRange, String)> {
    let analysis = &world.analysis;
    let root = analysis.files().get(file as usize)?.syntax();
    let token = root.token_at_offset(offset).find(|t| is_ident(t.kind()))?;
    let dot = significant(token.prev_token())?;
    if dot.kind() != SyntaxKind::DOT {
        return None;
    }
    let prefix = significant(dot.prev_token())?;
    let record = designated(analysis, name_type(analysis, file, &prefix, 0)?);
    let Some(TypeDef::Record(elements)) = type_def(analysis, record) else {
        return None;
    };
    let key = vfl_syntax::identifier_key(token.text());
    let element = elements.iter().find(|e| e.name == key)?;
    let mut text = format!(
        "element `{}` of `{}`",
        element.spelling,
        spelling(analysis, Some(record))
    );
    if let Some(mark) = element.subtype {
        text.push_str(&format!("\n\ntype {}", mark_text(analysis, mark)));
        if let Some(summary) = analysis
            .base_type(mark)
            .and_then(|t| type_summary(analysis, t))
        {
            text.push_str("\n\n");
            text.push_str(&summary);
        }
    }
    Some((token.text_range(), text))
}

/// The text of `decl`'s default value, from its source.
fn default_text(world: &World, decl: &Decl) -> Option<String> {
    let text = &world.analysis.files()[decl.file as usize].text;
    let source = &text[std::ops::Range::<usize>::from(decl.full_range)];
    let at = source.find(":=")?;
    let value = source[at + 2..].trim().trim_end_matches([';', ')']).trim();
    let value: String = value.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(if value.chars().count() > 40 {
        format!("{}...", value.chars().take(37).collect::<String>())
    } else {
        value
    })
}

/// One interface element as shown in a signature: `a : in integer := 0`.
fn param_text(world: &World, decl: &Decl) -> String {
    let analysis = &world.analysis;
    let (mode, has_default) = match decl.kind {
        DeclKind::Port { mode, has_default } | DeclKind::Parameter { mode, has_default } => {
            (Some(mode), has_default)
        }
        DeclKind::Generic { has_default } => (None, has_default),
        DeclKind::GenericType => return format!("type {}", decl.spelling),
        _ => (None, false),
    };
    let mut text = format!("{} :", decl.spelling);
    if let Some(mode) = mode {
        text.push(' ');
        text.push_str(mode.as_str());
    }
    if decl.subtype.is_some() {
        text.push(' ');
        text.push_str(&spelling(analysis, decl.subtype));
    }
    if has_default && let Some(value) = default_text(world, decl) {
        text.push_str(" := ");
        text.push_str(&value);
    }
    text
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// A subprogram's or a design entity's signature, and where each
/// parameter is in it. `ports` selects an entity's ports (`Some(true)`) or
/// generics (`Some(false)`).
fn signature(world: &World, decl: &Decl, ports: Option<bool>) -> (String, Vec<[u32; 2]>) {
    let analysis = &world.analysis;
    let params: Vec<DeclId> = match ports {
        None => decl.params.iter().flat_map(|p| p.iter().copied()).collect(),
        Some(ports) => decl
            .members
            .iter()
            .flat_map(|m| m.iter())
            .filter(|&m| {
                analysis.decl(m).is_some_and(|d| match d.kind {
                    DeclKind::Port { .. } => ports,
                    DeclKind::Generic { .. }
                    | DeclKind::GenericType
                    | DeclKind::GenericSubprogram
                    | DeclKind::GenericPackage => !ports,
                    _ => false,
                })
            })
            .collect(),
    };
    let mut label = match ports {
        None => format!("{} {}", decl.kind.describe(), decl.spelling),
        Some(true) => format!("{} {} port map", decl.kind.describe(), decl.spelling),
        Some(false) => format!("{} {} generic map", decl.kind.describe(), decl.spelling),
    };
    let mut offsets = Vec::new();
    if !params.is_empty() {
        label.push_str(if ports.is_some() { " (" } else { "(" });
        for (i, &p) in params.iter().enumerate() {
            if i > 0 {
                label.push_str("; ");
            }
            let Some(param) = analysis.decl(p) else {
                continue;
            };
            let start = utf16_len(&label);
            label.push_str(&param_text(world, param));
            offsets.push([start, utf16_len(&label)]);
        }
        label.push(')');
    }
    if decl.kind == DeclKind::Function && decl.subtype.is_some() {
        label.push_str(" return ");
        label.push_str(&spelling(analysis, decl.subtype));
    }
    (label, offsets)
}

fn param_names(world: &World, decl: &Decl, ports: Option<bool>) -> Vec<String> {
    let analysis = &world.analysis;
    match ports {
        None => decl
            .params
            .iter()
            .flat_map(|p| p.iter())
            .filter_map(|&p| analysis.decl(p).map(|d| d.name.clone()))
            .collect(),
        Some(ports) => decl
            .members
            .iter()
            .flat_map(|m| m.iter())
            .filter_map(|m| analysis.decl(m))
            .filter(|d| match d.kind {
                DeclKind::Port { .. } => ports,
                DeclKind::Generic { .. }
                | DeclKind::GenericType
                | DeclKind::GenericSubprogram
                | DeclKind::GenericPackage => !ports,
                _ => false,
            })
            .map(|d| d.name.clone())
            .collect(),
    }
}

/// Where the cursor is in an association list.
struct Call {
    /// The token before the opening parenthesis.
    before: SyntaxToken,
    /// Commas before the cursor, at the list's level.
    commas: u32,
    /// The formal of the association the cursor is in, if named.
    formal: Option<String>,
}

fn enclosing_call(world: &World, file: FileId, offset: TextSize) -> Option<Call> {
    let root = world.analysis.files().get(file as usize)?.syntax();
    if offset > root.text_range().end() {
        return None;
    }
    // The token before the cursor: `(`, `,` or a word being typed.
    let mut token = root.token_at_offset(offset).left_biased()?;
    // A token after the cursor (the cursor at its start) does not count.
    if token.text_range().start() >= offset {
        token = token.prev_token()?;
    }
    let mut level = 0u32;
    let mut commas = 0u32;
    // Significant tokens of the current association, last first.
    let mut current: Vec<SyntaxToken> = Vec::new();
    let mut in_current = true;
    for _ in 0..MAX_TOKENS {
        let kind = token.kind();
        if !kind.is_trivia() {
            match kind {
                SyntaxKind::RPAREN => level += 1,
                SyntaxKind::LPAREN if level > 0 => level -= 1,
                SyntaxKind::LPAREN => {
                    let before = significant(token.prev_token())?;
                    current.reverse();
                    let formal = match current.as_slice() {
                        [name, arrow, ..] if arrow.kind() == SyntaxKind::ARROW => {
                            Some(vfl_syntax::identifier_key(name.text()))
                        }
                        _ => None,
                    };
                    return Some(Call {
                        before,
                        commas,
                        formal,
                    });
                }
                SyntaxKind::COMMA if level == 0 => {
                    commas += 1;
                    in_current = false;
                }
                SyntaxKind::SEMI | SyntaxKind::BEGIN_KW | SyntaxKind::IS_KW if level == 0 => {
                    return None;
                }
                _ => {}
            }
            if in_current && level == 0 && kind != SyntaxKind::COMMA {
                current.push(token.clone());
            }
        }
        token = token.prev_token()?;
    }
    None
}

/// The entity or component an instantiation's map aspect belongs to, from
/// the `map` keyword back to the start of the statement.
fn instantiated(world: &World, file: FileId, map: &SyntaxToken) -> Option<DeclId> {
    let analysis = &world.analysis;
    let mut token = map.prev_token()?;
    let mut level = 0u32;
    for _ in 0..MAX_TOKENS {
        match token.kind() {
            SyntaxKind::RPAREN => level += 1,
            SyntaxKind::LPAREN => level = level.saturating_sub(1),
            SyntaxKind::SEMI | SyntaxKind::BEGIN_KW | SyntaxKind::COLON if level == 0 => {
                return None;
            }
            kind if level == 0 && is_ident(kind) => {
                if let Some(reference) = reference_at(analysis, file, token.text_range())
                    && let Some(id) = decl_targets(reference).find(|&id| {
                        analysis.decl(id).is_some_and(|d| {
                            matches!(d.kind, DeclKind::Entity | DeclKind::Component)
                        })
                    })
                {
                    return Some(id);
                }
            }
            _ => {}
        }
        token = token.prev_token()?;
    }
    None
}

/// The subprograms a callee name may denote.
fn callees(world: &World, file: FileId, name: &SyntaxToken) -> Vec<DeclId> {
    let analysis = &world.analysis;
    if let Some(reference) = reference_at(analysis, file, name.text_range()) {
        let found: Vec<DeclId> = decl_targets(reference)
            .filter(|&id| analysis.decl(id).is_some_and(|d| is_subprogram(d.kind)))
            .collect();
        if !found.is_empty() {
            return found;
        }
    }
    let key = vfl_syntax::identifier_key(name.text());
    // A protected type method.
    if let Some(dot) = significant(name.prev_token())
        && dot.kind() == SyntaxKind::DOT
    {
        let Some(prefix) = significant(dot.prev_token()) else {
            return Vec::new();
        };
        let Some(ty) = name_type(analysis, file, &prefix, 0) else {
            return Vec::new();
        };
        let ty = designated(analysis, ty);
        if let Some(TypeDef::Protected) = type_def(analysis, ty)
            && let Some(members) = analysis.decl(ty).and_then(|d| d.members.as_ref())
        {
            return members
                .get(&key)
                .iter()
                .copied()
                .filter(|&id| analysis.decl(id).is_some_and(|d| is_subprogram(d.kind)))
                .collect();
        }
        return Vec::new();
    }
    // Not resolved (the call is still being typed): what is visible by
    // that name.
    let mut found = Vec::new();
    crate::handlers::visible_decls(world, file, name.text_range().start(), |id, decl| {
        if decl.name == key && is_subprogram(decl.kind) && !found.contains(&id) {
            found.push(id);
        }
    });
    found
}

pub fn signature_help(world: &World, params: SignatureHelpParams) -> Option<SignatureHelp> {
    let position = &params.text_document_position_params;
    let file = world.file_id(&position.text_document.uri.to_file_path().ok()?)?;
    let offset = world.doc(file).offset(position.position);
    signature_help_at(world, file, offset)
}

/// The signatures of the call or instantiation around `offset`.
pub fn signature_help_at(world: &World, file: FileId, offset: TextSize) -> Option<SignatureHelp> {
    let call = enclosing_call(world, file, offset)?;
    let analysis = &world.analysis;
    let (targets, ports) = match call.before.kind() {
        SyntaxKind::MAP_KW => {
            let aspect = significant(call.before.prev_token())?;
            let ports = match aspect.kind() {
                SyntaxKind::PORT_KW => true,
                SyntaxKind::GENERIC_KW => false,
                _ => return None,
            };
            (vec![instantiated(world, file, &aspect)?], Some(ports))
        }
        kind if is_ident(kind) => (callees(world, file, &call.before), None),
        _ => return None,
    };
    let mut signatures = Vec::new();
    let mut active = None;
    for id in targets {
        let Some(decl) = analysis.decl(id) else {
            continue;
        };
        let (label, offsets) = signature(world, decl, ports);
        let names = param_names(world, decl, ports);
        let parameter = match &call.formal {
            Some(formal) => names.iter().position(|n| n == formal),
            None => Some(call.commas as usize).filter(|&i| i < names.len()),
        };
        if active.is_none() && (parameter.is_some() || names.is_empty() && call.commas == 0) {
            active = Some(signatures.len() as u32);
        }
        signatures.push(SignatureInformation {
            label,
            documentation: None,
            parameters: Some(
                offsets
                    .into_iter()
                    .map(|o| ParameterInformation {
                        label: ParameterLabel::LabelOffsets(o),
                        documentation: None,
                    })
                    .collect(),
            ),
            active_parameter: Some(parameter.unwrap_or(names.len()) as u32),
        });
    }
    if signatures.is_empty() {
        return None;
    }
    let active_signature = active.unwrap_or(0);
    let active_parameter = signatures[active_signature as usize].active_parameter;
    Some(SignatureHelp {
        signatures,
        active_signature: Some(active_signature),
        active_parameter,
    })
}
