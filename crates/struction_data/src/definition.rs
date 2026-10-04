//! The shape of a definition file, and the resolved result runtimes consume.

use std::collections::BTreeSet;

use bevy::ecs::reflect::ReflectComponent;
use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use serde_json::Value;
use struction_core::{Definition, DefinitionPath, StateRules};

use crate::build::ComponentValue;
use crate::error::{DataError, ErrorKind};
use crate::extensors::{DroppedExtensor, ExtensorUse, Named};
use crate::source::{Member, Node, NodeValue, Span};

/// Sections in canonical file order: identity and `descendsFrom`, extensors, transform,
/// components, constraints, reactions, and last `states`, which switch components over time. The
/// order is a convention for readers, never semantics.
pub const CANONICAL_ORDER: [&str; 9] = [
    "$schema",
    "descendsFrom",
    "presets",
    "extensors",
    "transform",
    "components",
    "constraints",
    "reactions",
    "states",
];

/// Sections other crates own (behavior trees, master grants) that are kept as raw data.
pub const DEFAULT_EXTRA_SECTIONS: [&str; 3] = ["brain", "sensing", "grantsToWards"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayerKind {
    Entity,
    Preset,
    /// Scene or spawn overrides applied over a resolved definition.
    Override,
}

impl LayerKind {
    fn label(self) -> &'static str {
        match self {
            LayerKind::Entity => "entity definition",
            LayerKind::Preset => "preset",
            LayerKind::Override => "override",
        }
    }
}

/// One parsed file: references to other definitions plus the data it contributes.
#[derive(Clone, Debug)]
pub(crate) struct Layer {
    pub root_span: Span,
    /// The prose written above the file's value, the layer's own description.
    pub doc: Option<String>,
    pub descends_from: Option<(String, Span)>,
    pub presets: Vec<(String, Span)>,
    pub extensors: Vec<(String, Span)>,
    /// `components` (with `transform` folded in as the `Transform` component), `constraints`,
    /// `reactions` and extra sections.
    pub body: Node,
}

/// `node` when `ok`, else a type mismatch naming `what` was expected.
pub(crate) fn expect<'n>(node: &'n Node, what: &str, ok: bool) -> Result<&'n Node, DataError> {
    if ok {
        Ok(node)
    } else {
        Err(DataError::at(
            ErrorKind::TypeMismatch {
                expected: what.into(),
                found: node.kind_name().into(),
            },
            &node.span,
        ))
    }
}

pub(crate) fn parse_layer(
    root: Node,
    doc: Option<String>,
    kind: LayerKind,
    extra_sections: &BTreeSet<String>,
) -> Result<Layer, DataError> {
    let root_span = root.span.clone();
    let NodeValue::Object(members) = root.value else {
        return Err(DataError::at(
            ErrorKind::TypeMismatch {
                expected: format!("object for {}", kind.label()),
                found: root.kind_name().into(),
            },
            &root_span,
        ));
    };
    let mut descends_from = None;
    let mut presets = Vec::new();
    let mut extensors = Vec::new();
    let mut transform: Option<Member> = None;
    let mut body = Vec::new();
    for member in members {
        let value = &member.value;
        match member.key.as_str() {
            "$schema" => {}
            "descendsFrom" if kind == LayerKind::Entity => {
                expect(value, "string", value.as_str().is_some())?;
                descends_from = Some((value.as_str().unwrap().to_owned(), value.span.clone()));
            }
            "presets" => {
                let items = expect(value, "array of preset names", value.as_array().is_some())?
                    .as_array()
                    .unwrap();
                for item in items {
                    expect(item, "string", item.as_str().is_some())?;
                    presets.push((item.as_str().unwrap().to_owned(), item.span.clone()));
                }
            }
            "extensors" => {
                let items = expect(value, "array of extensor names", value.as_array().is_some())?
                    .as_array()
                    .unwrap();
                for item in items {
                    expect(item, "string", item.as_str().is_some())?;
                    extensors.push((item.as_str().unwrap().to_owned(), item.span.clone()));
                }
            }
            "transform" => {
                expect(value, "object", value.as_object().is_some())?;
                transform = Some(member);
            }
            "components" => {
                expect(value, "object", value.as_object().is_some())?;
                body.push(member);
            }
            "constraints" | "reactions" => {
                expect(value, "array", value.as_array().is_some())?;
                body.push(member);
            }
            "states" => {
                expect(value, "object of states", value.as_object().is_some())?;
                body.push(member);
            }
            other if extra_sections.contains(other) => body.push(member),
            _ => {
                return Err(DataError::at(
                    ErrorKind::UnknownSection(member.key, kind.label()),
                    &member.key_span,
                ));
            }
        }
    }
    if let Some(transform) = transform {
        let components = body.iter_mut().find(|m| m.key == "components");
        let entry = Member {
            key: "Transform".into(),
            key_span: transform.key_span.clone(),
            value: transform.value,
        };
        match components {
            Some(Member {
                value:
                    Node {
                        value: NodeValue::Object(items),
                        ..
                    },
                ..
            }) => {
                if items.iter().any(|m| m.key == "Transform") {
                    return Err(DataError::at(
                        ErrorKind::DuplicateComponent("Transform".into()),
                        &transform.key_span,
                    ));
                }
                items.push(entry);
            }
            _ => body.push(Member {
                key: "components".into(),
                key_span: transform.key_span,
                value: Node {
                    span: entry.value.span.clone(),
                    value: NodeValue::Object(vec![entry]),
                },
            }),
        }
    }
    Ok(Layer {
        body: Node {
            span: root_span.clone(),
            value: NodeValue::Object(body),
        },
        root_span,
        doc,
        descends_from,
        presets,
        extensors,
    })
}

/// Removes components a layer set to `null`. Presets keep their nulls until they are applied, so
/// a preset can strip a component from what it is applied to.
pub(crate) fn strip_removed_components(body: &mut Node) {
    if let NodeValue::Object(members) = &mut body.value
        && let Some(components) = members.iter_mut().find(|m| m.key == "components")
        && let NodeValue::Object(items) = &mut components.value.value
    {
        items.retain(|m| !m.value.is_null());
    }
}

pub(crate) fn is_primordial(id: &str) -> bool {
    id.rsplit('/')
        .next()
        .and_then(|name| name.chars().next())
        .is_some_and(char::is_uppercase)
}

/// A definition after inheritance, presets and overrides, with components built and validated.
#[derive(Debug)]
pub struct Resolved {
    pub id: String,
    /// Ancestors, the primordial type first; a definition's own path is not part of it.
    pub lineage: Vec<String>,
    /// Extensors in use: named ones first, then those inferred from components or requirements.
    pub extensors: Vec<ExtensorUse>,
    /// Extensors named by an ancestor or preset that a later layer dropped with `"-name"`.
    pub dropped: Vec<DroppedExtensor>,
    pub components: Vec<ComponentValue>,
    /// What the `states` section switches, applied by the packages owning the states.
    pub states: Option<StateRules>,
    pub(crate) body: Node,
    /// The `extensors` entries as written, which instances extend with their overrides.
    pub(crate) named: Vec<Named>,
    pub(crate) dropped_entries: Vec<Named>,
    value: Value,
}

impl Resolved {
    pub(crate) fn new(
        id: String,
        lineage: Vec<String>,
        (named, dropped, extensors): (Vec<Named>, Vec<Named>, Vec<ExtensorUse>),
        components: Vec<ComponentValue>,
        states: Option<StateRules>,
        body: Node,
    ) -> Self {
        let value = body.to_value();
        Self {
            id,
            lineage,
            extensors,
            dropped: dropped
                .iter()
                .map(|d| DroppedExtensor {
                    name: d.name.clone(),
                    by: d.by.clone(),
                })
                .collect(),
            components,
            states,
            body,
            named,
            dropped_entries: dropped,
            value,
        }
    }

    /// Raw data of a non-component section (`constraints`, `reactions`, `brain`, ...), spans
    /// included, for the crates that own those sections.
    pub fn section(&self, name: &str) -> Option<&Node> {
        self.body.get(name)
    }

    /// The merged data without spans. Two resolutions are equal when this, the lineage and the
    /// extensors are.
    pub fn data(&self) -> &Value {
        &self.value
    }

    pub fn same_data(&self, other: &Resolved) -> bool {
        self.lineage == other.lineage
            && self.extensors == other.extensors
            && self.dropped == other.dropped
            && self.value == other.value
    }

    /// The core [`Definition`] component: this definition's path and its lineage, so queries
    /// like "descends from `minions/ogre`" need no data lookup.
    pub fn definition(&self) -> Definition {
        Definition::new(
            self.id.as_str(),
            self.lineage.iter().map(DefinitionPath::new).collect(),
        )
    }

    /// The built component of type `T`, if the definition has one.
    pub fn component<T: Reflect>(&self) -> Option<&T> {
        self.components
            .iter()
            .find_map(|c| c.value.downcast_ref::<T>())
    }

    /// Inserts every component plus the core [`Definition`]. Also what a runtime does to refresh a live
    /// instance after a reload (insertion replaces the previous value).
    pub fn insert_into(&self, entity: &mut EntityWorldMut, registry: &TypeRegistry) {
        for component in &self.components {
            let reflect = registry
                .get_type_data::<ReflectComponent>(component.type_id)
                .expect("checked when the component was built");
            reflect.insert(entity, &*component.value, registry);
        }
        entity.insert(self.definition());
        if let Some(states) = &self.states {
            entity.insert(states.clone());
        }
    }
}
