//! Comment-preserving edits of JSONC sources, the write path for the editor's undo/redo.
//!
//! Every function works on a source string and returns the new string, so the caller decides
//! when to write the file. Edits go through the `jsonc-parser` CST: comments, blank lines and
//! indentation around untouched values stay as written, and a new top-level section is inserted
//! at its canonical position instead of at the end.

use jsonc_parser::cst::{CstInputValue, CstNode, CstObject, CstRootNode};
use serde_json::Value;
use thiserror::Error;

use crate::definition::CANONICAL_ORDER;
use crate::source::parse_options;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathSegment {
    Key(String),
    Index(usize),
}

impl From<&str> for PathSegment {
    fn from(key: &str) -> Self {
        PathSegment::Key(key.into())
    }
}

impl From<usize> for PathSegment {
    fn from(index: usize) -> Self {
        PathSegment::Index(index)
    }
}

/// Parses `components.Health.hp` or `reactions[0].call` into segments. Keys cannot contain `.`
/// or `[`; build the segments by hand for those.
pub fn parse_path(path: &str) -> Vec<PathSegment> {
    let mut segments = Vec::new();
    for part in path.split('.').filter(|p| !p.is_empty()) {
        let (key, mut rest) = part.split_once('[').map_or((part, ""), |(k, r)| (k, r));
        if !key.is_empty() {
            segments.push(PathSegment::Key(key.into()));
        }
        while let Some((index, tail)) = rest.split_once(']') {
            match index.parse() {
                Ok(i) => segments.push(PathSegment::Index(i)),
                Err(_) => segments.push(PathSegment::Key(index.into())),
            }
            rest = tail.strip_prefix('[').unwrap_or(tail);
        }
    }
    segments
}

fn describe(path: &[PathSegment]) -> String {
    let mut out = String::new();
    for segment in path {
        match segment {
            PathSegment::Key(k) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(k);
            }
            PathSegment::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    if out.is_empty() { "<root>".into() } else { out }
}

/// One recorded change: the unit of the editor's undo stack (together with the file it applies to).
#[derive(Clone, Debug, PartialEq)]
pub struct FieldEdit {
    pub path: Vec<PathSegment>,
    /// `None` when the field did not exist.
    pub previous: Option<Value>,
    /// `None` when the edit removes the field.
    pub next: Option<Value>,
}

impl FieldEdit {
    /// The edit that undoes this one (and, applied again, redoes it).
    pub fn inverse(&self) -> FieldEdit {
        FieldEdit {
            path: self.path.clone(),
            previous: self.next.clone(),
            next: self.previous.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Edited {
    pub text: String,
    pub edit: FieldEdit,
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum EditError {
    #[error("syntax error: {0}")]
    Syntax(String),
    #[error("the document root must be an object")]
    RootNotObject,
    #[error("nothing at {0}")]
    NotFound(String),
    #[error("cannot go through {0}: it is neither an object nor an array")]
    NotContainer(String),
    #[error("cannot set {0}: array index out of range")]
    IndexOutOfRange(String),
}

fn parse(text: &str) -> Result<CstRootNode, EditError> {
    CstRootNode::parse(text, &parse_options()).map_err(|e| {
        EditError::Syntax(format!(
            "{}:{}: {}",
            e.line_display(),
            e.column_display(),
            e.kind()
        ))
    })
}

fn input(value: &Value) -> CstInputValue {
    match value {
        Value::Null => CstInputValue::Null,
        Value::Bool(b) => CstInputValue::Bool(*b),
        Value::Number(n) => CstInputValue::Number(n.to_string()),
        Value::String(s) => CstInputValue::String(s.clone()),
        Value::Array(items) => CstInputValue::Array(items.iter().map(input).collect()),
        Value::Object(map) => {
            CstInputValue::Object(map.iter().map(|(k, v)| (k.clone(), input(v))).collect())
        }
    }
}

fn root_object(root: &CstRootNode) -> Result<CstObject, EditError> {
    root.object_value().ok_or(EditError::RootNotObject)
}

fn child(node: &CstNode, segment: &PathSegment) -> Option<CstNode> {
    match segment {
        PathSegment::Key(key) => node.as_object()?.get(key)?.value(),
        PathSegment::Index(i) => node.as_array()?.elements().into_iter().nth(*i),
    }
}

fn find(root: &CstRootNode, path: &[PathSegment]) -> Result<Option<CstNode>, EditError> {
    let mut node = root.value().ok_or(EditError::RootNotObject)?;
    for segment in path {
        match child(&node, segment) {
            Some(next) => node = next,
            None => return Ok(None),
        }
    }
    Ok(Some(node))
}

fn canonical_rank(key: &str) -> usize {
    CANONICAL_ORDER
        .iter()
        .position(|k| *k == key)
        .unwrap_or(CANONICAL_ORDER.len())
}

/// Adds a property; on the document root, at the position the canonical order gives it.
fn add_property(object: &CstObject, is_root: bool, key: &str, value: CstInputValue) -> CstNode {
    let prop = if is_root {
        let rank = canonical_rank(key);
        let index = object
            .properties()
            .iter()
            .position(|p| canonical_rank(&p.decoded_name().unwrap_or_default()) > rank)
            .unwrap_or_else(|| object.properties().len());
        object.insert(index, key, value)
    } else {
        object.append(key, value)
    };
    prop.value().expect("a property has a value")
}

fn replace(node: CstNode, value: CstInputValue) -> Result<(), EditError> {
    let replaced = match node {
        CstNode::Container(c) => match (c.as_object(), c.as_array()) {
            (Some(o), _) => o.replace_with(value),
            (_, Some(a)) => a.replace_with(value),
            _ => None,
        },
        CstNode::Leaf(_) => {
            if let Some(n) = node.as_string_lit() {
                n.replace_with(value)
            } else if let Some(n) = node.as_number_lit() {
                n.replace_with(value)
            } else if let Some(n) = node.as_boolean_lit() {
                n.replace_with(value)
            } else if let Some(n) = node.as_null_keyword() {
                n.replace_with(value)
            } else {
                node.as_word_lit().and_then(|n| n.replace_with(value))
            }
        }
    };
    replaced
        .map(|_| ())
        .ok_or_else(|| EditError::NotContainer("value".into()))
}

/// Reads the value at `path`, `None` if absent.
pub fn get_value(text: &str, path: &[PathSegment]) -> Result<Option<Value>, EditError> {
    let root = parse(text)?;
    Ok(find(&root, path)?.and_then(|n| n.to_serde_value()))
}

/// Sets the value at `path`, creating missing objects along the way. Returns the new text and the
/// recorded edit (with the previous value) for the undo stack.
pub fn set_value(text: &str, path: &[PathSegment], value: Value) -> Result<Edited, EditError> {
    let root = parse(text)?;
    let object = root_object(&root)?;
    let Some((last, parents)) = path.split_last() else {
        return Err(EditError::NotFound(describe(path)));
    };

    // Walk to the container of the last segment, creating objects for missing keys.
    let mut node: CstNode = object.clone().into();
    for (depth, segment) in parents.iter().enumerate() {
        node = match child(&node, segment) {
            Some(next) => next,
            None => match (segment, node.as_object()) {
                (PathSegment::Key(key), Some(parent)) => {
                    add_property(&parent, depth == 0, key, CstInputValue::Object(Vec::new()))
                }
                _ => return Err(EditError::NotFound(describe(&path[..=depth]))),
            },
        };
        if node.as_object().is_none() && node.as_array().is_none() {
            return Err(EditError::NotContainer(describe(&path[..=depth])));
        }
    }

    let previous = child(&node, last).and_then(|n| n.to_serde_value());
    let new_input = input(&value);
    match (last, node.as_object(), node.as_array()) {
        (PathSegment::Key(key), Some(parent), _) => match parent.get(key) {
            Some(prop) => prop.set_value(new_input),
            None => {
                add_property(&parent, parents.is_empty(), key, new_input);
            }
        },
        (PathSegment::Index(i), _, Some(array)) => {
            let len = array.elements().len();
            match *i {
                i if i < len => replace(array.elements().remove(i), new_input)?,
                i if i == len => {
                    array.append(new_input);
                }
                _ => return Err(EditError::IndexOutOfRange(describe(path))),
            }
        }
        _ => return Err(EditError::NotContainer(describe(parents))),
    }

    Ok(Edited {
        text: root.to_string(),
        edit: FieldEdit {
            path: path.to_vec(),
            previous,
            next: Some(value),
        },
    })
}

/// Removes the value at `path`; errors if there is none.
pub fn remove_value(text: &str, path: &[PathSegment]) -> Result<Edited, EditError> {
    let root = parse(text)?;
    root_object(&root)?;
    let node = find(&root, path)?.ok_or_else(|| EditError::NotFound(describe(path)))?;
    let previous = node.to_serde_value();
    match path.last() {
        Some(PathSegment::Key(key)) => {
            let parent = find(&root, &path[..path.len() - 1])?
                .and_then(|p| p.as_object())
                .and_then(|o| o.get(key))
                .ok_or_else(|| EditError::NotFound(describe(path)))?;
            parent.remove();
        }
        Some(PathSegment::Index(_)) => node.remove(),
        None => return Err(EditError::NotFound(describe(path))),
    }
    Ok(Edited {
        text: root.to_string(),
        edit: FieldEdit {
            path: path.to_vec(),
            previous,
            next: None,
        },
    })
}

/// Applies a recorded edit: sets `next`, or removes the field when `next` is `None`. Applying
/// `edit.inverse()` undoes it.
pub fn apply_edit(text: &str, edit: &FieldEdit) -> Result<Edited, EditError> {
    match &edit.next {
        Some(value) => set_value(text, &edit.path, value.clone()),
        None => remove_value(text, &edit.path),
    }
}

/// Top-level fields of a file, in the order found and in canonical order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderIssue {
    pub found: Vec<String>,
    pub expected: Vec<String>,
}

impl std::fmt::Display for OrderIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "top-level fields are ordered [{}], canonical order is [{}]",
            self.found.join(", "),
            self.expected.join(", ")
        )
    }
}

/// `None` when the top-level fields follow the canonical order (identity and `descendsFrom`,
/// transform, components, constraints, reactions; other fields after those).
pub fn check_canonical_order(text: &str) -> Result<Option<OrderIssue>, EditError> {
    let root = parse(text)?;
    let found: Vec<String> = root_object(&root)?
        .properties()
        .iter()
        .filter_map(|p| p.decoded_name())
        .collect();
    let mut expected = found.clone();
    expected.sort_by_key(|k| canonical_rank(k));
    Ok((found != expected).then_some(OrderIssue { found, expected }))
}

/// Reorders the top-level fields canonically. Comments above a field, a comment on its line and
/// blank-line-separated headings move with it as `jsonc-parser` defines for sorting.
pub fn canonicalize(text: &str) -> Result<String, EditError> {
    let root = parse(text)?;
    root_object(&root)?
        .sort_properties()
        .pin_comment_headers()
        .by_key(|p| canonical_rank(&p.decoded_name().unwrap_or_default()));
    Ok(root.to_string())
}
