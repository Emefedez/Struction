//! Spanned JSONC values. Every value remembers the file, line and column it came from, so errors
//! can point at it even after inheritance merged it into another definition.

use std::sync::Arc;

use jsonc_parser::ParseOptions;
use jsonc_parser::ast;
use jsonc_parser::common::Range;
use jsonc_parser::{CollectOptions, parse_to_ast};
use serde_json::{Number, Value};

use crate::error::{DataError, ErrorKind, Location};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    /// Byte offset into the file.
    pub offset: usize,
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub file: Arc<str>,
    pub start: Pos,
    pub end: Pos,
}

impl Span {
    pub fn start_location(&self) -> Location {
        Location {
            file: self.file.clone(),
            line: self.start.line,
            column: self.start.column,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Node {
    pub span: Span,
    pub value: NodeValue,
}

#[derive(Clone, Debug)]
pub enum NodeValue {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Node>),
    /// Members keep source order; keys are unique.
    Object(Vec<Member>),
}

#[derive(Clone, Debug)]
pub struct Member {
    pub key: String,
    pub key_span: Span,
    pub value: Node,
}

impl Node {
    pub fn empty_object(span: Span) -> Self {
        Self {
            span,
            value: NodeValue::Object(Vec::new()),
        }
    }

    pub fn as_object(&self) -> Option<&[Member]> {
        match &self.value {
            NodeValue::Object(members) => Some(members),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Node]> {
        match &self.value {
            NodeValue::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            NodeValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self.value, NodeValue::Null)
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.as_object()?
            .iter()
            .find(|m| m.key == key)
            .map(|m| &m.value)
    }

    /// JSON type name for error messages.
    pub fn kind_name(&self) -> &'static str {
        match self.value {
            NodeValue::Null => "null",
            NodeValue::Bool(_) => "boolean",
            NodeValue::Number(_) => "number",
            NodeValue::String(_) => "string",
            NodeValue::Array(_) => "array",
            NodeValue::Object(_) => "object",
        }
    }

    /// Drops the spans. Two nodes with equal values are the same data whatever their formatting.
    pub fn to_value(&self) -> Value {
        match &self.value {
            NodeValue::Null => Value::Null,
            NodeValue::Bool(b) => Value::Bool(*b),
            NodeValue::Number(n) => Value::Number(n.clone()),
            NodeValue::String(s) => Value::String(s.clone()),
            NodeValue::Array(items) => Value::Array(items.iter().map(Node::to_value).collect()),
            NodeValue::Object(members) => Value::Object(
                members
                    .iter()
                    .map(|m| (m.key.clone(), m.value.to_value()))
                    .collect(),
            ),
        }
    }

    /// Deep merge of `over` onto `self`: objects merge field by field, everything else (arrays
    /// included) is replaced. Keys keep the base order; new keys are appended.
    pub fn merge(&mut self, over: Node) {
        match (&mut self.value, over.value) {
            (NodeValue::Object(base), NodeValue::Object(over_members)) => {
                for member in over_members {
                    match base.iter_mut().find(|m| m.key == member.key) {
                        Some(existing) => {
                            existing.key_span = member.key_span;
                            existing.value.merge(member.value);
                        }
                        None => base.push(member),
                    }
                }
                // Keep the span of the layer that touched the object last.
                self.span = over.span;
            }
            (slot, value) => {
                *slot = value;
                self.span = over.span;
            }
        }
    }
}

/// Parse options shared by the loader and the editing API: JSON with comments and trailing commas.
pub(crate) fn parse_options() -> ParseOptions {
    ParseOptions {
        allow_comments: true,
        allow_loose_object_property_names: false,
        allow_trailing_commas: true,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

struct Lines<'a> {
    text: &'a str,
    file: Arc<str>,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    fn new(file: &str, text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self {
            text,
            file: file.into(),
            starts,
        }
    }

    fn pos(&self, offset: usize) -> Pos {
        let line = self.starts.partition_point(|&s| s <= offset) - 1;
        let column = self.text[self.starts[line]..offset].chars().count() + 1;
        Pos {
            offset,
            line: line as u32 + 1,
            column: column as u32,
        }
    }

    fn span(&self, range: Range) -> Span {
        Span {
            file: self.file.clone(),
            start: self.pos(range.start),
            end: self.pos(range.end),
        }
    }

    fn error(&self, kind: ErrorKind, range: Range) -> DataError {
        DataError::at(kind, &self.span(range))
    }

    fn node(&self, value: &ast::Value) -> Result<Node, DataError> {
        let (range, value) = match value {
            ast::Value::NullKeyword(n) => (n.range, NodeValue::Null),
            ast::Value::BooleanLit(b) => (b.range, NodeValue::Bool(b.value)),
            ast::Value::StringLit(s) => (s.range, NodeValue::String(s.value.to_string())),
            ast::Value::NumberLit(n) => {
                let number = serde_json::from_str::<Number>(n.value).map_err(|e| {
                    self.error(ErrorKind::Syntax(format!("invalid number: {e}")), n.range)
                })?;
                (n.range, NodeValue::Number(number))
            }
            ast::Value::Array(a) => {
                let items = a
                    .elements
                    .iter()
                    .map(|v| self.node(v))
                    .collect::<Result<_, _>>()?;
                (a.range, NodeValue::Array(items))
            }
            ast::Value::Object(o) => {
                let mut members: Vec<Member> = Vec::with_capacity(o.properties.len());
                for prop in &o.properties {
                    let (key, key_range) = match &prop.name {
                        ast::ObjectPropName::String(s) => (s.value.to_string(), s.range),
                        ast::ObjectPropName::Word(w) => (w.value.to_string(), w.range),
                    };
                    if members.iter().any(|m| m.key == key) {
                        return Err(self.error(ErrorKind::DuplicateKey(key), key_range));
                    }
                    members.push(Member {
                        key,
                        key_span: self.span(key_range),
                        value: self.node(&prop.value)?,
                    });
                }
                (o.range, NodeValue::Object(members))
            }
        };
        Ok(Node {
            span: self.span(range),
            value,
        })
    }
}

/// Parses JSONC text. `file` is the name errors will report, normally a project-relative path.
pub fn parse_jsonc(file: &str, text: &str) -> Result<Node, DataError> {
    let lines = Lines::new(file, text);
    let parsed = parse_to_ast(text, &CollectOptions::default(), &parse_options()).map_err(|e| {
        DataError::at(
            ErrorKind::Syntax(e.kind().to_string()),
            &lines.span(e.range()),
        )
    })?;
    match parsed.value {
        Some(value) => lines.node(&value),
        None => Err(DataError::new(
            ErrorKind::Syntax("file is empty".into()),
            Some(Location {
                file: file.into(),
                line: 1,
                column: 1,
            }),
        )),
    }
}

/// The prose a file starts with: the block of `//` comments written directly above its root
/// value, which is how a definition file describes itself.
pub fn leading_doc(text: &str, root: usize) -> Option<String> {
    let head = &text[..root];
    let lines: Vec<&str> = head.lines().collect();
    let described = |line: &str| {
        let line = line.trim_start();
        line.strip_prefix("//").map(|rest| rest.trim().to_owned())
    };
    // Only the run of comments against the value is its description; anything further up was
    // written about something else.
    let first = lines
        .iter()
        .rposition(|line| described(line).is_none())
        .map_or(0, |above| above + 1);
    let doc: Vec<String> = lines[first..]
        .iter()
        .filter_map(|line| described(line))
        .collect();
    (!doc.is_empty()).then(|| doc.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_have_lines_and_character_columns() {
        let text = "{\n  \"név\": 1,\n  \"b\": [true, null]\n}";
        let root = parse_jsonc("f.jsonc", text).unwrap();
        let members = root.as_object().unwrap();
        assert_eq!(
            (
                members[0].key_span.start.line,
                members[0].key_span.start.column
            ),
            (2, 3)
        );
        // The column counts characters, not bytes (é is two bytes).
        let value = &members[0].value.span;
        assert_eq!((value.start.line, value.start.column), (2, 10));
        let b = &members[1].value.as_array().unwrap()[1];
        assert_eq!((b.span.start.line, b.span.start.column), (3, 15));
        assert_eq!(&*b.span.file, "f.jsonc");
    }

    #[test]
    fn comments_and_trailing_commas_are_allowed_but_not_json5() {
        assert!(parse_jsonc("f", "{ /* c */ \"a\": [1, 2,], // x\n}").is_ok());
        assert!(parse_jsonc("f", "{ a: 1 }").is_err());
        assert!(parse_jsonc("f", "{ \"a\": 0x10 }").is_err());
        assert!(parse_jsonc("f", "{ \"a\": 1 \"b\": 2 }").is_err());
        assert!(parse_jsonc("f", "  // nothing\n").is_err());
    }

    #[test]
    fn the_prose_above_a_value_is_its_description() {
        let text = "// A walking thing.\n// It swings its arms.\n{\n  \"a\": 1\n}\n";
        assert_eq!(
            leading_doc(text, text.find('{').unwrap()).as_deref(),
            Some("A walking thing.\nIt swings its arms.")
        );
        // Only the run against the value; a blank line ends it.
        let apart = "// A heading.\n\n// A field's own note.\n{ \"a\": 1 }\n";
        assert_eq!(
            leading_doc(apart, apart.find('{').unwrap()).as_deref(),
            Some("A field's own note.")
        );
        assert_eq!(leading_doc("{ }", 0), None);
        assert_eq!(leading_doc("// nothing but prose\n", 0), None);
        assert_eq!(
            leading_doc("/* not a line comment */\n{ }", 23).as_deref(),
            None
        );
    }

    #[test]
    fn merge_is_deep_for_objects_and_replaces_everything_else() {
        let mut base = parse_jsonc("a", r#"{ "x": { "p": 1, "q": [1, 2] }, "y": 1 }"#).unwrap();
        let over = parse_jsonc("b", r#"{ "x": { "q": [3], "r": null }, "z": 2 }"#).unwrap();
        base.merge(over);
        assert_eq!(
            base.to_value(),
            serde_json::json!({ "x": { "p": 1, "q": [3], "r": null }, "y": 1, "z": 2 })
        );
        // Each value keeps the file it came from.
        assert_eq!(&*base.get("y").unwrap().span.file, "a");
        assert_eq!(&*base.get("x").unwrap().get("q").unwrap().span.file, "b");
        assert_eq!(&*base.get("x").unwrap().get("p").unwrap().span.file, "a");
    }
}
