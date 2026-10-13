//! Declarations, the regions that hold them, and what a name resolves to.

use std::collections::HashMap;
use std::sync::Arc;

use vfl_syntax::TextRange;

/// A file of the analysis: an index into [`crate::Analysis::files`].
pub type FileId = u32;
/// A design unit of the analysis: an index into [`crate::Analysis::units`].
pub type UnitId = u32;

/// A declaration: `(unit, index)` into that unit's declarations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeclId {
    pub unit: UnitId,
    pub index: u32,
}

/// Port and parameter modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    In,
    Out,
    InOut,
    Buffer,
    Linkage,
    /// A 2019 mode view.
    View,
}

impl Mode {
    /// The mode as written in VHDL.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::In => "in",
            Mode::Out => "out",
            Mode::InOut => "inout",
            Mode::Buffer => "buffer",
            Mode::Linkage => "linkage",
            Mode::View => "view",
        }
    }

    /// Whether an actual associated with this mode is read by the instance.
    pub fn reads_actual(self) -> bool {
        !matches!(self, Mode::Out)
    }

    /// Whether an actual associated with this mode is driven by the instance.
    pub fn writes_actual(self) -> bool {
        !matches!(self, Mode::In)
    }
}

/// The kind of a declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeclKind {
    Entity,
    Architecture,
    Package,
    PackageBody,
    PackageInstance,
    Context,
    Configuration,
    Component,
    Signal,
    Variable,
    Constant,
    File,
    Port {
        mode: Mode,
        has_default: bool,
    },
    Generic {
        has_default: bool,
    },
    GenericType,
    GenericSubprogram,
    GenericPackage,
    Parameter {
        mode: Mode,
        has_default: bool,
    },
    Type,
    /// `type t;`, completed by a later full declaration.
    IncompleteType,
    /// `type t is protected body`.
    ProtectedBody,
    Subtype,
    EnumLiteral,
    PhysicalUnit,
    Function,
    Procedure,
    Alias,
    Attribute,
    Label,
    LoopParameter,
    GenerateParameter,
    Group,
    View,
}

impl DeclKind {
    /// Overloadable declarations may share a name within a region.
    pub fn is_overloadable(self) -> bool {
        matches!(
            self,
            DeclKind::Function
                | DeclKind::Procedure
                | DeclKind::EnumLiteral
                | DeclKind::GenericSubprogram
                | DeclKind::Alias
        )
    }

    /// Objects whose value is read or written: what usage checks look at.
    pub fn is_object(self) -> bool {
        matches!(
            self,
            DeclKind::Signal
                | DeclKind::Variable
                | DeclKind::Constant
                | DeclKind::File
                | DeclKind::Port { .. }
                | DeclKind::Generic { .. }
                | DeclKind::Parameter { .. }
                | DeclKind::LoopParameter
                | DeclKind::GenerateParameter
        )
    }

    /// Signal-like objects: what sensitivity lists and drivers are about.
    pub fn is_signal(self) -> bool {
        matches!(self, DeclKind::Signal | DeclKind::Port { .. })
    }

    pub fn describe(self) -> &'static str {
        match self {
            DeclKind::Entity => "entity",
            DeclKind::Architecture => "architecture",
            DeclKind::Package | DeclKind::PackageInstance => "package",
            DeclKind::PackageBody => "package body",
            DeclKind::Context => "context",
            DeclKind::Configuration => "configuration",
            DeclKind::Component => "component",
            DeclKind::Signal => "signal",
            DeclKind::Variable => "variable",
            DeclKind::Constant => "constant",
            DeclKind::File => "file",
            DeclKind::Port { .. } => "port",
            DeclKind::Generic { .. } => "generic",
            DeclKind::GenericType => "generic type",
            DeclKind::GenericSubprogram => "generic subprogram",
            DeclKind::GenericPackage => "generic package",
            DeclKind::Parameter { .. } => "parameter",
            DeclKind::Type | DeclKind::IncompleteType | DeclKind::ProtectedBody => "type",
            DeclKind::Subtype => "subtype",
            DeclKind::EnumLiteral => "enumeration literal",
            DeclKind::PhysicalUnit => "unit",
            DeclKind::Function => "function",
            DeclKind::Procedure => "procedure",
            DeclKind::Alias => "alias",
            DeclKind::Attribute => "attribute",
            DeclKind::Label => "label",
            DeclKind::LoopParameter => "loop parameter",
            DeclKind::GenerateParameter => "generate parameter",
            DeclKind::Group => "group",
            DeclKind::View => "view",
        }
    }
}

/// A named declaration.
#[derive(Debug, Clone)]
pub struct Decl {
    /// The identifier key (lower case unless extended); operator symbols
    /// keep their quotes (`"+"`).
    pub name: String,
    /// The name as written.
    pub spelling: String,
    pub kind: DeclKind,
    pub file: FileId,
    /// The range of the name.
    pub range: TextRange,
    /// The range of the whole declaration.
    pub full_range: TextRange,
    /// What the declaration contains, for names selected through it
    /// (`pkg.x`) and for instantiations: a package's declarations, an
    /// entity's or component's generics and ports.
    pub members: Option<Arc<Region>>,
    /// The enclosing declaration (unit, subprogram, process, ...), if any.
    pub parent: Option<DeclId>,
    /// The type or subtype this declaration refers to: an object's or an
    /// alias's subtype, a subtype's type mark, a function's return type, an
    /// enumeration literal's or a unit's type, an incomplete type's full
    /// declaration. `None` when unknown.
    pub subtype: Option<DeclId>,
    /// What a type declaration defines.
    pub type_def: Option<Arc<TypeDef>>,
    /// A subprogram's parameters, in order.
    pub params: Option<Arc<[DeclId]>>,
}

/// The definition of a type, as far as type checking needs it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeDef {
    /// The literals, as lookup keys: identifiers in lower case, character
    /// literals with their quotes (`'0'`).
    Enum(Vec<String>),
    Integer,
    Real,
    Physical,
    /// A range type whose bounds do not tell integer from real.
    Scalar,
    Array {
        /// The index subtypes; `None` where unknown.
        indexes: Vec<Option<DeclId>>,
        element: Option<DeclId>,
    },
    Record(Vec<Element>),
    Access(Option<DeclId>),
    File(Option<DeclId>),
    Protected,
}

impl TypeDef {
    /// Calls `f` with every declaration the definition refers to.
    pub(crate) fn decl_ids(&self, mut f: impl FnMut(DeclId)) {
        match self {
            TypeDef::Array { indexes, element } => {
                indexes.iter().flatten().copied().for_each(&mut f);
                element.iter().copied().for_each(f);
            }
            TypeDef::Record(elements) => {
                elements.iter().filter_map(|e| e.subtype).for_each(f);
            }
            TypeDef::Access(Some(id)) | TypeDef::File(Some(id)) => f(*id),
            _ => {}
        }
    }
}

/// A record element.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Element {
    pub name: String,
    pub spelling: String,
    pub subtype: Option<DeclId>,
}

/// The declarations of one declarative region, by name, in source order.
#[derive(Debug, Clone, Default)]
pub struct Region {
    names: HashMap<String, Vec<DeclId>>,
    order: Vec<DeclId>,
    /// Set when the region could not be built completely (an unknown
    /// generic package, say): lookups that fail are not errors.
    pub opaque: bool,
}

impl Region {
    /// A region whose contents are unknown.
    pub fn opaque() -> Self {
        Region {
            opaque: true,
            ..Region::default()
        }
    }

    pub fn insert(&mut self, name: &str, id: DeclId) {
        self.names.entry(name.to_owned()).or_default().push(id);
        self.order.push(id);
    }

    pub fn get(&self, name: &str) -> &[DeclId] {
        self.names.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Declarations in source order.
    pub fn iter(&self) -> impl Iterator<Item = DeclId> + '_ {
        self.order.iter().copied()
    }

    pub fn contains_operators(&self) -> bool {
        self.names.keys().any(|k| k.starts_with('"'))
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Feeds the region's names and declarations to `h`.
    pub(crate) fn hash_into(&self, h: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        let mut names: Vec<(&String, &Vec<DeclId>)> = self.names.iter().collect();
        names.sort_unstable_by_key(|(k, _)| *k);
        names.hash(h);
        self.order.hash(h);
        self.opaque.hash(h);
    }
}

/// What a name refers to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    Decl(DeclId),
    Library(String),
}

/// How a reference uses what it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Access {
    Read,
    Write,
    ReadWrite,
    /// Named without reading or writing a value: a type mark, a formal in an
    /// association, the prefix of an attribute like `'range`, an attribute
    /// specification.
    Other,
}

impl Access {
    pub fn reads(self) -> bool {
        matches!(self, Access::Read | Access::ReadWrite)
    }

    pub fn writes(self) -> bool {
        matches!(self, Access::Write | Access::ReadWrite)
    }
}

/// A resolved reference.
#[derive(Debug, Clone)]
pub struct Reference {
    pub range: TextRange,
    pub targets: Vec<Target>,
    pub access: Access,
}
