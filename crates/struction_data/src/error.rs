use std::fmt;
use std::sync::Arc;

use thiserror::Error;

/// Where in a source file a value starts. Lines and columns are 1-based; columns count characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub file: Arc<str>,
    pub line: u32,
    pub column: u32,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ErrorKind {
    #[error("{0}")]
    Io(String),
    #[error("syntax error: {0}")]
    Syntax(String),
    #[error("duplicate key \"{0}\"")]
    DuplicateKey(String),
    #[error("unknown field \"{0}\" in {1}")]
    UnknownSection(String, &'static str),
    #[error("unknown component \"{0}\"")]
    UnknownComponent(String),
    #[error("\"{0}\" is a registered type but not a component")]
    NotAComponent(String),
    #[error("component name \"{name}\" is ambiguous, use one of: {}", .candidates.join(", "))]
    AmbiguousComponent {
        name: String,
        candidates: Vec<String>,
    },
    #[error("component {0} is given twice")]
    DuplicateComponent(String),
    #[error("unknown field \"{field}\" in {ty}")]
    UnknownField { field: String, ty: String },
    #[error("missing field{} {} in {ty}", if .fields.len() == 1 { "" } else { "s" }, .fields.join(", "))]
    MissingField { fields: Vec<String>, ty: String },
    #[error("unknown variant \"{variant}\" in {ty}")]
    UnknownVariant { variant: String, ty: String },
    #[error("expected {expected}, found {found}")]
    TypeMismatch { expected: String, found: String },
    #[error("invalid value for {ty}: {message}")]
    InvalidValue { ty: String, message: String },
    #[error("{0} cannot be loaded from data: no reflection support for it")]
    UnsupportedType(String),
    #[error("descendsFrom refers to missing definition \"{0}\"")]
    MissingDefinition(String),
    #[error("unknown preset \"{0}\"")]
    MissingPreset(String),
    #[error("descendsFrom cycle: {}", .0.join(" -> "))]
    DefinitionCycle(Vec<String>),
    #[error("preset cycle: {}", .0.join(" -> "))]
    PresetCycle(Vec<String>),
    #[error("\"{field}\" refers to missing definition \"{path}\"")]
    MissingReference { field: String, path: String },
    /// An action reference that does not resolve or does not match the action's signature.
    #[error("{0}")]
    Action(String),
    #[error("unknown extensor \"{name}\"{}", if .known.is_empty() { String::new() } else { format!(", registered: {}", .known.join(", ")) })]
    UnknownExtensor { name: String, known: Vec<String> },
    #[error("{component} belongs to the opt-in extensor \"{extensor}\"; add it to \"extensors\"")]
    ExtensorNotNamed { component: String, extensor: String },
    #[error("extensor \"{extensor}\" needs \"{requires}\"; add it to \"extensors\"")]
    ExtensorRequires { extensor: String, requires: String },
    #[error("unknown state \"{name}\"{}", if .known.is_empty() { String::new() } else { format!(", registered: {}", .known.join(", ")) })]
    UnknownState { name: String, known: Vec<String> },
    #[error("state {state} comes from the opt-in extensor \"{extensor}\"; add it to \"extensors\"")]
    StateNeedsExtensor { state: String, extensor: String },
    #[error(
        "preset \"{name}\" is defined by the {library} library; give the project's another name"
    )]
    LibraryPreset { name: String, library: String },
    #[error(
        "\"{id}\" overrides the {library} definition and keeps its parent; remove descendsFrom or give it another path"
    )]
    OverrideParent { id: String, library: String },
    #[error(
        "definition \"{0}\" has no descendsFrom and is not primordial (primordial names are capitalized)"
    )]
    NotPrimordial(String),
}

/// A data error with the source position of the offending value, when known.
///
/// Displays as `ogre/entity.jsonc:12: unknown field "hp" in Health`; the column is in
/// [`Location`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataError {
    pub kind: ErrorKind,
    pub location: Option<Location>,
}

impl DataError {
    pub fn new(kind: ErrorKind, location: Option<Location>) -> Self {
        Self { kind, location }
    }

    pub fn at(kind: ErrorKind, span: &crate::source::Span) -> Self {
        Self::new(kind, Some(span.start_location()))
    }
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.location {
            Some(loc) => write!(f, "{}:{}: {}", loc.file, loc.line, self.kind),
            None => write!(f, "{}", self.kind),
        }
    }
}

impl std::error::Error for DataError {}
