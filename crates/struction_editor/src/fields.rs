//! Schema-guided additions; all writes use the normal validated source/history path.
use serde::Serialize;
use serde_json::{Value, json};
use struction_data::parse_jsonc;

use crate::{Applied, AuthoringProject, EditRequest, Field, SessionError};

pub fn lookup<'a>(value: &'a Value, path: &[Field]) -> Option<&'a Value> {
    path.iter().try_fold(value, |v, f| match f {
        Field::Key(k) => v.get(k),
        Field::Index(i) => v.get(*i),
    })
}

/// Resolve the generated schema's references and select the branch of the current value.
/// With no current value, prefer a non-null branch so optional fields can be added.
pub fn shape<'a>(root: &'a Value, schema: &'a Value, value: Option<&Value>) -> &'a Value {
    fn resolve<'a>(
        root: &'a Value,
        schema: &'a Value,
        value: Option<&Value>,
        depth: usize,
    ) -> &'a Value {
        if depth > 32 {
            return &Value::Null;
        }
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            return reference
                .strip_prefix('#')
                .and_then(|p| root.pointer(p))
                .map_or(&Value::Null, |s| resolve(root, s, value, depth + 1));
        }
        if let Some(branches) = schema
            .get("anyOf")
            .or_else(|| schema.get("oneOf"))
            .and_then(Value::as_array)
        {
            let choices: Vec<_> = branches
                .iter()
                .map(|s| resolve(root, s, value, depth + 1))
                .collect();
            let matches = |s: &&Value| {
                let Some(v) = value else {
                    return false;
                };
                if let Some(values) = s.get("enum").and_then(Value::as_array) {
                    return values.contains(v);
                }
                if let Some(c) = s.get("const") {
                    return c == v;
                }
                match s.get("type").and_then(Value::as_str) {
                    Some("object") => v.as_object().is_some_and(|o| {
                        s.get("required")
                            .and_then(Value::as_array)
                            .is_none_or(|keys| {
                                keys.iter()
                                    .all(|k| k.as_str().is_some_and(|k| o.contains_key(k)))
                            })
                    }),
                    Some("array") => v.is_array(),
                    Some("string") => v.is_string(),
                    Some("number" | "integer") => v.is_number(),
                    Some("boolean") => v.is_boolean(),
                    Some("null") => v.is_null(),
                    _ => false,
                }
            };
            return choices
                .iter()
                .copied()
                .find(matches)
                .or_else(|| {
                    choices
                        .iter()
                        .find(|s| s.get("type").and_then(Value::as_str) != Some("null"))
                        .copied()
                })
                .unwrap_or(&Value::Null);
        }
        schema
    }
    resolve(root, schema, value, 0)
}

pub fn schema_at<'a>(
    root: &'a Value,
    schema: &'a Value,
    value: &Value,
    path: &[Field],
) -> Option<&'a Value> {
    let mut schema = schema;
    let mut current = Some(value);
    for field in path {
        schema = shape(root, schema, current);
        schema = match field {
            Field::Key(k) => schema
                .get("properties")
                .and_then(|p| p.get(k))
                .or_else(|| schema.get("additionalProperties").filter(|v| v.is_object()))?,
            Field::Index(i) => schema
                .get("prefixItems")
                .and_then(|p| p.get(*i))
                .or_else(|| schema.get("items"))?,
        };
        current = current.and_then(|v| lookup(v, std::slice::from_ref(field)));
    }
    Some(shape(root, schema, current))
}

/// A starting value for the form, not a promise of semantic validity; project validation
/// still checks cross-field constraints, required extensor opt-ins and references.
pub fn initial_value(root: &Value, schema: &Value) -> Option<Value> {
    fn build(root: &Value, schema: &Value, depth: usize) -> Option<Value> {
        if depth > 24 {
            return None;
        }
        let schema = shape(root, schema, None);
        if let Some(v) = schema.get("default").or_else(|| schema.get("const")) {
            return Some(v.clone());
        }
        if let Some(v) = schema.get("enum").and_then(|v| v.get(0)) {
            return Some(v.clone());
        }
        Some(match schema.get("type").and_then(Value::as_str)? {
            "null" => Value::Null,
            "boolean" => json!(false),
            "string" => json!(
                if schema.get("minLength").and_then(Value::as_u64).unwrap_or(0) > 0 {
                    "a"
                } else {
                    ""
                }
            ),
            "number" | "integer" => schema.get("minimum").cloned().unwrap_or(json!(0)),
            "array" => {
                if let Some(items) = schema.get("prefixItems").and_then(Value::as_array) {
                    Value::Array(
                        items
                            .iter()
                            .map(|s| build(root, s, depth + 1))
                            .collect::<Option<_>>()?,
                    )
                } else {
                    let count = schema
                        .get("minItems")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        .min(256);
                    Value::Array(
                        (0..count)
                            .map(|_| build(root, schema.get("items")?, depth + 1))
                            .collect::<Option<_>>()?,
                    )
                }
            }
            "object" => {
                let mut object = serde_json::Map::new();
                if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                    for (key, s) in props {
                        object.insert(key.clone(), build(root, s, depth + 1)?);
                    }
                }
                Value::Object(object)
            }
            _ => return None,
        })
    }
    build(root, schema, 0)
}

#[derive(Debug, Serialize)]
pub struct FieldOptions {
    pub schema: Value,
    pub value: Value,
    pub authored: Option<Value>,
}

fn invalid(message: impl Into<String>) -> SessionError {
    SessionError::InvalidOperation(message.into())
}

fn merge(base: &mut Value, overlay: &Value) {
    if let (Some(base), Some(overlay)) = (base.as_object_mut(), overlay.as_object()) {
        for (k, v) in overlay {
            merge(base.entry(k).or_insert(Value::Null), v);
        }
    } else {
        *base = overlay.clone();
    }
}

impl AuthoringProject {
    fn field_document(
        &self,
        file: &str,
        path: &[Field],
    ) -> Result<(Value, Value, Vec<Field>), SessionError> {
        self.session().path_of(file)?;
        let (definition, relative, overlay) = if let Some(id) = file.strip_suffix("/entity.jsonc") {
            (id.to_owned(), path.to_vec(), Value::Null)
        } else if file.starts_with("scenes/") {
            let text = self.session().read(file)?;
            let doc = parse_jsonc(file, &text)
                .map_err(|e| SessionError::Validation(vec![e]))?
                .to_value();
            let [
                Field::Key(list),
                Field::Key(spawner),
                Field::Key(spawns),
                Field::Key(spawn),
                Field::Key(overrides),
                rest @ ..,
            ] = path
            else {
                return Err(invalid(
                    "Field additions in scenes require a named spawn's overrides",
                ));
            };
            if list != "spawnerList" || spawns != "spawns" || overrides != "overrides" {
                return Err(invalid(
                    "Expected spawnerList.<name>.spawns.<name>.overrides",
                ));
            }
            let spawn = &doc[list][spawner][spawns][spawn];
            let id = spawn
                .get("definition")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("Spawn has no definition"))?;
            (
                id.to_owned(),
                rest.to_vec(),
                spawn.get("overrides").cloned().unwrap_or(Value::Null),
            )
        } else {
            return Err(invalid(
                "Field additions require a definition or a spawn override",
            ));
        };
        let inspected = self.inspect_definition(&definition)?;
        let schema = self.schema();
        let mut document = inspected.resolved.clone();
        // Reflected defaults make omitted lists and struct fields available without replacing
        // inherited entries with empty containers.
        for (name, value) in inspected.components {
            let short = name.rsplit("::").next().unwrap_or(&name);
            let key = if schema["properties"]["components"]["properties"]
                .get(&name)
                .is_some()
            {
                name.as_str()
            } else {
                short
            };
            if short == "Transform" {
                let authored = document.get("transform").cloned().unwrap_or(json!({}));
                document["transform"] = value;
                merge(&mut document["transform"], &authored);
            } else {
                if !document.get("components").is_some_and(Value::is_object) {
                    document["components"] = json!({});
                }
                let authored = document["components"]
                    .get(key)
                    .cloned()
                    .unwrap_or(json!({}));
                document["components"][key] = value;
                merge(&mut document["components"][key], &authored);
            }
        }
        if !overlay.is_null() {
            merge(&mut document, &overlay);
        }
        Ok((schema, document, relative))
    }

    pub fn field_options(&self, file: &str, path: &[Field]) -> Result<FieldOptions, SessionError> {
        let (schema, document, relative) = self.field_document(file, path)?;
        let at = schema_at(&schema, &schema, &document, &relative)
            .ok_or_else(|| invalid("No registered schema at this field"))?;
        let authored = match self.session().read(file) {
            Ok(text) => {
                let source = parse_jsonc(file, &text)
                    .map_err(|e| SessionError::Validation(vec![e]))?
                    .to_value();
                lookup(&source, path).cloned()
            }
            Err(SessionError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(e) => return Err(e),
        };
        // Keep definitions alongside the fragment so clients can follow its nested references.
        let mut fragment = at.clone();
        fragment["$defs"] = schema["$defs"].clone();
        Ok(FieldOptions {
            schema: fragment,
            value: lookup(&document, &relative).cloned().unwrap_or(Value::Null),
            authored,
        })
    }

    pub fn add_field(
        &mut self,
        file: &str,
        path: &[Field],
        key: &str,
        value: Option<Value>,
    ) -> Result<Applied, SessionError> {
        let options = self.field_options(file, path)?;
        if options.authored.as_ref().and_then(|v| v.get(key)).is_some() {
            return Err(invalid(format!(
                "{key} is already authored; edit or reset it instead"
            )));
        }
        if !options.value.is_null() && !options.value.is_object() {
            return Err(invalid("Add field requires an object"));
        }
        let field_schema = options
            .schema
            .get("properties")
            .and_then(|p| p.get(key))
            .or_else(|| {
                options
                    .schema
                    .get("additionalProperties")
                    .filter(|s| s.is_object())
            })
            .ok_or_else(|| invalid(format!("{key} is not a field in this schema")))?;
        let value = value
            .or_else(|| options.value.get(key).cloned())
            .or_else(|| initial_value(&options.schema, field_schema))
            .ok_or_else(|| invalid(format!("Supply an initial value for {key}")))?;
        let mut at = path.to_vec();
        at.push(Field::Key(key.into()));
        self.set_added(file, at, value, format!("Add {key}"))
    }

    pub fn add_entry(
        &mut self,
        file: &str,
        path: &[Field],
        value: Option<Value>,
    ) -> Result<Applied, SessionError> {
        let options = self.field_options(file, path)?;
        if options.schema.get("type").and_then(Value::as_str) != Some("array") {
            return Err(invalid("Add entry requires a list"));
        }
        let mut items = options.value.as_array().cloned().unwrap_or_default();
        if options
            .schema
            .get("maxItems")
            .and_then(Value::as_u64)
            .is_some_and(|max| items.len() >= max as usize)
        {
            return Err(invalid(
                "This array already has its fixed number of entries",
            ));
        }
        let item_schema = options
            .schema
            .get("items")
            .ok_or_else(|| invalid("No entry schema"))?;
        let value = value
            .or_else(|| initial_value(&options.schema, item_schema))
            .ok_or_else(|| invalid("Supply an entry value"))?;
        if options.schema.get("uniqueItems") == Some(&Value::Bool(true)) && items.contains(&value) {
            return Err(invalid("This set already contains that entry"));
        }
        if options.authored.as_ref().is_some_and(Value::is_array) {
            let mut at = path.to_vec();
            at.push(Field::Index(items.len()));
            self.set_added(file, at, value, "Add entry".into())
        } else {
            // Arrays replace on inheritance: materialize the effective list before appending.
            items.push(value);
            self.set_added(file, path.to_vec(), Value::Array(items), "Add entry".into())
        }
    }

    fn set_added(
        &mut self,
        file: &str,
        path: Vec<Field>,
        value: Value,
        label: String,
    ) -> Result<Applied, SessionError> {
        // A nested edit in an inherited list must first materialize that list, as one step.
        if let Some(index) = path.iter().position(|f| matches!(f, Field::Index(_))) {
            let prefix = &path[..index];
            let options = self.field_options(file, prefix)?;
            if options.authored.is_none() {
                let mut array = options.value;
                let mut at = &mut array;
                for segment in &path[index..path.len() - 1] {
                    at = match segment {
                        Field::Key(k) => at.get_mut(k),
                        Field::Index(i) => at.get_mut(*i),
                    }
                    .ok_or_else(|| invalid("Missing parent in inherited list"))?;
                }
                match path.last().expect("nested field") {
                    Field::Key(k) => {
                        at.as_object_mut()
                            .ok_or_else(|| invalid("Expected object"))?
                            .insert(k.clone(), value);
                    }
                    Field::Index(i) => {
                        at.as_array_mut()
                            .ok_or_else(|| invalid("Expected list"))?
                            .insert(*i, value);
                    }
                }
                return self.edit(EditRequest::Set {
                    file: file.into(),
                    path: prefix.to_vec(),
                    value: array,
                    label,
                    group: None,
                    revision: None,
                });
            }
        }
        self.edit(EditRequest::Set {
            file: file.into(),
            path,
            value,
            label,
            group: None,
            revision: None,
        })
    }
}
