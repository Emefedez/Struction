//! JSON Schema for entity files, generated from the registered Reflect types.
//!
//! schemars derives schemas from `JsonSchema` impls, which reflected types do not have, so this
//! walks `TypeInfo` directly. Fields are never `required`: a child definition may give only the
//! fields it overrides (missing ones are still reported when the merged component is built).

use std::collections::BTreeSet;

use bevy::ecs::reflect::ReflectComponent;
use bevy::reflect::enums::VariantInfo;
use bevy::reflect::{TypeInfo, TypeRegistry};
use serde_json::{Map, Value, json};

use crate::definition::DEFAULT_EXTRA_SECTIONS;
use crate::typeinfo::{Primitive, info_for, is_option, primitive, vector_len};

pub struct SchemaOptions {
    /// Known definition ids, offered as `descendsFrom` completions.
    pub definitions: Vec<String>,
    /// Known preset names, offered as `presets` completions.
    pub presets: Vec<String>,
    /// Extra top-level sections other crates interpret; their content is unconstrained.
    pub extra_sections: Vec<String>,
    /// Registered extensors with their descriptions, offered as `extensors` completions.
    pub extensors: Vec<(String, String)>,
}

impl Default for SchemaOptions {
    fn default() -> Self {
        Self {
            definitions: Vec::new(),
            presets: Vec::new(),
            extra_sections: DEFAULT_EXTRA_SECTIONS.map(String::from).into(),
            extensors: Vec::new(),
        }
    }
}

struct Generator<'r> {
    registry: &'r TypeRegistry,
    defs: Map<String, Value>,
    started: BTreeSet<String>,
}

/// The schema of an entity definition file (draft 2020-12).
pub fn entity_schema(registry: &TypeRegistry, options: &SchemaOptions) -> Value {
    let mut generator = Generator {
        registry,
        defs: Map::new(),
        started: BTreeSet::new(),
    };

    let mut components: Vec<_> = registry
        .iter_with_data::<ReflectComponent>()
        .map(|(registration, _)| registration.type_info())
        .collect();
    components.sort_by_key(|info| info.type_path());
    let mut component_props = Map::new();
    for info in &components {
        let short = info.type_path_table().short_path();
        let name = if registry.is_ambiguous(short) {
            info.type_path()
        } else {
            short
        };
        component_props.insert(
            name.to_owned(),
            json!({
                "anyOf": [generator.schema(info), { "type": "null" }],
                "description": "Object of fields, or null to remove the inherited component.",
            }),
        );
    }

    let transform = components
        .iter()
        .find(|info| info.type_path_table().ident() == Some("Transform"))
        .map_or_else(
            || json!({ "type": "object" }),
            |info| generator.schema(info),
        );

    let string_or_enum = |names: &[String]| {
        if names.is_empty() {
            json!({ "type": "string" })
        } else {
            json!({ "type": "string", "enum": names })
        }
    };

    let mut properties = Map::new();
    properties.insert("$schema".into(), json!({ "type": "string" }));
    properties.insert("descendsFrom".into(), string_or_enum(&options.definitions));
    properties.insert(
        "presets".into(),
        json!({ "type": "array", "items": string_or_enum(&options.presets) }),
    );
    let extensor = if options.extensors.is_empty() {
        json!({ "type": "string" })
    } else {
        let names: Vec<_> = options
            .extensors
            .iter()
            .flat_map(|(name, doc)| {
                [
                    json!({ "const": name, "description": doc }),
                    json!({
                        "const": format!("-{name}"),
                        "description": format!("Drop the inherited {name} extensor and its components"),
                    }),
                ]
            })
            .collect();
        json!({ "oneOf": names })
    };
    properties.insert(
        "extensors".into(),
        json!({
            "type": "array",
            "items": extensor,
            "description": "Packages extending this definition, added to those it inherits.",
        }),
    );
    properties.insert("transform".into(), transform);
    properties.insert(
        "components".into(),
        json!({
            "type": "object",
            "properties": component_props,
            "additionalProperties": false,
        }),
    );
    properties.insert("constraints".into(), json!({ "type": "array" }));
    properties.insert("reactions".into(), json!({ "type": "array" }));
    for section in &options.extra_sections {
        properties.insert(section.clone(), json!({}));
    }

    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Struction entity definition",
        "type": "object",
        "properties": properties,
        "additionalProperties": false,
        "$defs": generator.defs,
    })
}

fn def_key(info: &TypeInfo) -> String {
    info.type_path()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl Generator<'_> {
    fn of(&mut self, ty: bevy::reflect::Type, own: Option<&'static TypeInfo>) -> Value {
        match info_for(self.registry, ty, own) {
            Some(info) => self.schema(info),
            None => json!({}),
        }
    }

    /// Named types go to `$defs` once and are referenced, which also terminates recursion.
    fn define(&mut self, info: &'static TypeInfo, build: impl FnOnce(&mut Self) -> Value) -> Value {
        let key = def_key(info);
        if self.started.insert(key.clone()) {
            let mut schema = build(self);
            if let (Some(docs), Some(object)) = (info.docs(), schema.as_object_mut()) {
                object.insert("description".into(), json!(docs.trim()));
            }
            if let Some(object) = schema.as_object_mut() {
                object.insert("title".into(), json!(info.type_path_table().short_path()));
            }
            self.defs.insert(key.clone(), schema);
        }
        json!({ "$ref": format!("#/$defs/{key}") })
    }

    fn schema(&mut self, info: &'static TypeInfo) -> Value {
        match info {
            TypeInfo::Struct(si) => {
                let reference = self.define(info, |g| {
                    let mut properties = Map::new();
                    for field in si.iter() {
                        let mut schema = g.of(*field.ty(), field.type_info());
                        if let (Some(docs), Some(object)) = (field.docs(), schema.as_object_mut())
                            && !object.contains_key("$ref")
                        {
                            object.insert("description".into(), json!(docs.trim()));
                        }
                        properties.insert(field.name().to_owned(), schema);
                    }
                    json!({ "type": "object", "properties": properties, "additionalProperties": false })
                });
                match info.type_path_table().ident().and_then(vector_len) {
                    Some(n) => json!({ "anyOf": [
                        { "type": "array", "items": { "type": "number" }, "minItems": n, "maxItems": n },
                        reference,
                    ] }),
                    None => reference,
                }
            }
            TypeInfo::TupleStruct(ti) => {
                if ti.field_len() == 1 {
                    let field = ti.field_at(0).expect("one field");
                    return self.of(*field.ty(), field.type_info());
                }
                let items = ti.iter().map(|f| self.of(*f.ty(), f.type_info())).collect();
                tuple_schema(items)
            }
            TypeInfo::Tuple(ti) => {
                let items = ti.iter().map(|f| self.of(*f.ty(), f.type_info())).collect();
                tuple_schema(items)
            }
            TypeInfo::List(li) => {
                json!({ "type": "array", "items": self.of(li.item_ty(), li.item_info()) })
            }
            TypeInfo::Array(ai) => json!({
                "type": "array",
                "items": self.of(ai.item_ty(), ai.item_info()),
                "minItems": ai.capacity(),
                "maxItems": ai.capacity(),
            }),
            TypeInfo::Map(mi) => json!({
                "type": "object",
                "additionalProperties": self.of(mi.value_ty(), mi.value_info()),
            }),
            TypeInfo::Set(si) => json!({
                "type": "array",
                "uniqueItems": true,
                "items": self.of(si.value_ty(), None),
            }),
            TypeInfo::Enum(ei) => {
                if is_option(info) {
                    let inner = match ei.variant("Some") {
                        Some(VariantInfo::Tuple(some)) => {
                            let field = some.field_at(0).expect("Some has one field");
                            self.of(*field.ty(), field.type_info())
                        }
                        _ => json!({}),
                    };
                    return json!({ "anyOf": [{ "type": "null" }, inner] });
                }
                self.define(info, |g| {
                    let mut unit = Vec::new();
                    let mut alternatives = Vec::new();
                    for variant in ei.iter() {
                        let payload = match variant {
                            VariantInfo::Unit(v) => {
                                unit.push(v.name());
                                continue;
                            }
                            VariantInfo::Tuple(v) => {
                                if v.field_len() == 1 {
                                    let field = v.field_at(0).expect("one field");
                                    g.of(*field.ty(), field.type_info())
                                } else {
                                    let items =
                                        v.iter().map(|f| g.of(*f.ty(), f.type_info())).collect();
                                    tuple_schema(items)
                                }
                            }
                            VariantInfo::Struct(v) => {
                                let mut properties = Map::new();
                                for field in v.iter() {
                                    properties.insert(
                                        field.name().to_owned(),
                                        g.of(*field.ty(), field.type_info()),
                                    );
                                }
                                let required: Vec<_> = v.iter().map(|f| f.name()).collect();
                                json!({
                                    "type": "object",
                                    "properties": properties,
                                    "required": required,
                                    "additionalProperties": false,
                                })
                            }
                        };
                        alternatives.push(json!({
                            "type": "object",
                            "properties": { variant.name(): payload },
                            "required": [variant.name()],
                            "additionalProperties": false,
                        }));
                    }
                    if !unit.is_empty() {
                        alternatives.insert(0, json!({ "enum": unit }));
                    }
                    json!({ "oneOf": alternatives })
                })
            }
            TypeInfo::Opaque(_) => match primitive(info.type_id()) {
                Some(Primitive::Bool) => json!({ "type": "boolean" }),
                Some(Primitive::Signed) => json!({ "type": "integer" }),
                Some(Primitive::Unsigned) => json!({ "type": "integer", "minimum": 0 }),
                Some(Primitive::Float) => json!({ "type": "number" }),
                Some(Primitive::Str) => json!({ "type": "string" }),
                Some(Primitive::Char) => {
                    json!({ "type": "string", "minLength": 1, "maxLength": 1 })
                }
                None => json!({ "description": info.type_path_table().short_path() }),
            },
        }
    }
}

fn tuple_schema(items: Vec<Value>) -> Value {
    json!({
        "type": "array",
        "prefixItems": items,
        "minItems": items.len(),
        "maxItems": items.len(),
    })
}
