//! Units written in another language: Verilog and SystemVerilog modules,
//! which VHDL instantiates like entities.
//!
//! Only what VHDL can see of a module is analyzed: its name, parameters
//! (as generics) and ports. Its declarations get no VHDL type, so the type
//! checker leaves the associations of its ports alone; the width of a port
//! whose width is a literal is still checked against what is mapped to it.

use std::path::PathBuf;
use std::sync::Arc;

use vfl_syntax::TextRange;

use crate::analysis::UnitResult;
use crate::decl::{Decl, DeclId, DeclKind, FileId, Mode, Region, UnitId};

/// A file of another language whose modules VHDL can instantiate.
#[derive(Debug, Clone)]
pub struct ForeignInput {
    pub path: PathBuf,
    pub text: Arc<str>,
    /// The libraries its modules belong to.
    pub libraries: Vec<String>,
    /// Its modules. A new version of the file comes with a new list: an
    /// analysis tells unchanged files by pointer.
    pub modules: Arc<[ForeignModule]>,
}

/// A module, seen as an entity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForeignModule {
    /// The name as written.
    pub name: String,
    pub name_range: TextRange,
    /// The module's header: from `module` to the `;` after its ports.
    pub range: TextRange,
    /// The parameters an instance can set, as generics.
    pub generics: Vec<ForeignObject>,
    pub ports: Vec<ForeignObject>,
}

/// A parameter or a port of a module.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForeignObject {
    /// The name as written.
    pub name: String,
    pub name_range: TextRange,
    /// The declaration.
    pub range: TextRange,
    /// A port's mode; `None` for an interface port or one whose direction
    /// is unknown, which VHDL can leave unassociated.
    pub mode: Option<Mode>,
    pub has_default: bool,
    /// The number of bits of a vector port whose width is literal.
    pub width: Option<u32>,
    /// A parameter's default, when it is a literal integer.
    pub value: Option<i64>,
    /// The declaration, as it would be written in a port or parameter
    /// list: `input logic [7:0] d`, for hovers.
    pub declaration: String,
}

/// The lookup key of a name of another language: lower case, as VHDL
/// compares basic identifiers without case. An escaped identifier
/// (`\a+b `) keeps its text without the backslash.
pub fn foreign_key(name: &str) -> String {
    name.strip_prefix('\\')
        .unwrap_or(name)
        .trim_end()
        .to_ascii_lowercase()
}

/// The declarations of a module: the module as an entity (index 0), its
/// generics, then its ports.
pub(crate) fn module_result(file: FileId, unit: UnitId, module: &ForeignModule) -> UnitResult {
    let entity = DeclId { unit, index: 0 };
    let decl = |name: &str, kind, range, full_range| Decl {
        name: foreign_key(name),
        spelling: name.to_owned(),
        kind,
        file,
        range,
        full_range,
        members: None,
        parent: None,
        subtype: None,
        type_def: None,
        params: None,
        width: None,
        value: None,
    };
    let mut decls = vec![decl(
        &module.name,
        DeclKind::Entity,
        module.name_range,
        module.range,
    )];
    let mut region = Region::default();
    let objects = module
        .generics
        .iter()
        .map(|g| (g, true))
        .chain(module.ports.iter().map(|p| (p, false)));
    for (object, generic) in objects {
        let kind = if generic {
            DeclKind::Generic {
                has_default: object.has_default,
            }
        } else {
            match object.mode {
                Some(mode) => DeclKind::Port {
                    mode,
                    has_default: object.has_default,
                },
                // Nothing VHDL could connect: never required.
                None => DeclKind::Port {
                    mode: Mode::InOut,
                    has_default: true,
                },
            }
        };
        let mut d = decl(&object.name, kind, object.name_range, object.range);
        d.parent = Some(entity);
        d.width = object.width;
        d.value = object.value;
        let id = DeclId {
            unit,
            index: decls.len() as u32,
        };
        region.insert(&d.name, id);
        decls.push(d);
    }
    let region = Arc::new(region);
    decls[0].members = Some(region.clone());
    UnitResult {
        decls,
        region,
        ..UnitResult::default()
    }
}
