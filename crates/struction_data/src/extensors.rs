//! Which extensors a resolved definition uses, and why.
//!
//! Named extensors accumulate along `descendsFrom`, presets and overrides instead of replacing
//! each other like arrays do. Components of an opt-in extensor are refused unless it is named;
//! naming one adds the components it supplies with their defaults. Every other extensor is
//! inferred from the components it owns or from an extensor that requires it.

use std::sync::Arc;

use bevy::reflect::TypeRegistry;
use bevy::reflect::std_traits::ReflectDefault;
use struction_core::{ExtensorRegistry, Participation, StateRule, StateRules};

use crate::build::{Builder, ComponentValue};
use crate::definition::Resolved;
use crate::error::{DataError, ErrorKind};
use crate::source::{Node, Span};
use crate::store::build_component_map;

/// An `extensors` entry as written, with the definition or preset that wrote it.
#[derive(Clone, Debug)]
pub(crate) struct Named {
    pub name: String,
    pub span: Span,
    pub by: String,
}

/// Why a definition uses an extensor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensorReason {
    /// Named in `extensors` by this definition (or the ancestor, preset or override given).
    Named { by: String },
    /// Owns this component of the definition.
    Owns(String),
    /// Another extensor in use builds on it.
    RequiredBy(String),
}

/// An extensor a resolved definition uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensorUse {
    pub name: String,
    pub reason: ExtensorReason,
    /// Components of the definition it owns, by the name definitions write.
    pub components: Vec<String>,
    /// Those of `components` it added with their defaults because the definition left them out.
    pub supplied: Vec<String>,
}

/// An extensor an ancestor or preset named that a later layer dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DroppedExtensor {
    pub name: String,
    /// The definition, preset or override that wrote `"-name"`.
    pub by: String,
}

/// An opt-in extensor a definition does not use but could: every extensor it builds on is in
/// use. Only a suggestion; nothing is added until the author names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub name: String,
    pub doc: String,
    /// The extensors in use it builds on.
    pub because: Vec<String>,
    /// Components naming it would add with their defaults.
    pub supplies: Vec<String>,
}

impl Resolved {
    /// Opt-in extensors whose requirements this definition meets, except those it dropped.
    pub fn suggested_extensors(&self, extensors: &ExtensorRegistry) -> Vec<Suggestion> {
        let in_use = |name: &str| self.extensors.iter().any(|u| u.name == name);
        extensors
            .iter()
            .filter(|meta| {
                meta.participation == Participation::OptIn
                    && !meta.requires.is_empty()
                    && !in_use(&meta.name)
                    && !self.dropped.iter().any(|d| d.name == meta.name)
                    && meta.requires.iter().all(|r| in_use(r))
            })
            .map(|meta| Suggestion {
                name: meta.name.clone(),
                doc: meta.doc.clone(),
                because: meta.requires.clone(),
                supplies: meta
                    .components
                    .iter()
                    .filter(|c| c.supplied)
                    .map(|c| c.name.to_owned())
                    .collect(),
            })
            .collect()
    }
}

impl ExtensorUse {
    pub fn is_named(&self) -> bool {
        matches!(self.reason, ExtensorReason::Named { .. })
    }
}

/// Validates the extensors of a merged definition, adds the components named extensors supply
/// and returns every extensor in use: named ones in declaration order, then inferred ones.
pub(crate) fn resolve(
    named: &[Named],
    body: &Node,
    components: &mut Vec<ComponentValue>,
    extensors: &ExtensorRegistry,
    registry: &TypeRegistry,
) -> Result<Vec<ExtensorUse>, Vec<DataError>> {
    let mut errors = Vec::new();
    let mut uses: Vec<ExtensorUse> = Vec::new();
    let mut spans: Vec<Option<Span>> = Vec::new();
    for entry in named {
        if uses.iter().any(|u| u.name == entry.name) {
            continue;
        }
        if extensors.get(&entry.name).is_none() {
            errors.push(DataError::at(
                ErrorKind::UnknownExtensor {
                    name: entry.name.clone(),
                    known: extensors.names().map(str::to_owned).collect(),
                },
                &entry.span,
            ));
            continue;
        }
        uses.push(use_of(
            &entry.name,
            ExtensorReason::Named {
                by: entry.by.clone(),
            },
        ));
        spans.push(Some(entry.span.clone()));
    }

    let written = body
        .get("components")
        .and_then(Node::as_object)
        .unwrap_or_default();
    for component in components.iter() {
        let Some(owner) = extensors.owner(component.type_id) else {
            continue;
        };
        let name = short_name(component, registry);
        let span = written
            .iter()
            .find(|m| m.key == name || m.key == component.type_path)
            .map(|m| m.key_span.clone());
        let index = match uses.iter().position(|u| u.name == owner.name) {
            Some(index) => index,
            None if owner.participation == Participation::OptIn => {
                errors.push(DataError::new(
                    ErrorKind::ExtensorNotNamed {
                        component: name.into(),
                        extensor: owner.name.clone(),
                    },
                    span.map(|s| s.start_location()),
                ));
                continue;
            }
            None => {
                uses.push(use_of(&owner.name, ExtensorReason::Owns(name.into())));
                spans.push(span);
                uses.len() - 1
            }
        };
        uses[index].components.push(name.into());
    }

    for (index, entry) in named.iter().enumerate() {
        let Some(meta) = extensors.get(&entry.name) else {
            continue;
        };
        if named[..index].iter().any(|n| n.name == entry.name) {
            continue;
        }
        for owned in meta.components.iter().filter(|c| c.supplied) {
            if components.iter().any(|c| c.type_id == owned.type_id) {
                continue;
            }
            let Some((registration, default)) = registry
                .get(owned.type_id)
                .and_then(|r| Some((r, r.data::<ReflectDefault>()?)))
            else {
                errors.push(DataError::at(
                    ErrorKind::UnsupportedType(format!(
                        "{} (supplied by the {} extensor) without a registered Default",
                        owned.name, meta.name
                    )),
                    &entry.span,
                ));
                continue;
            };
            components.push(ComponentValue {
                type_id: owned.type_id,
                type_path: registration.type_info().type_path(),
                value: default.default(),
            });
            let used = uses
                .iter_mut()
                .find(|u| u.name == meta.name)
                .expect("named extensors are in use");
            used.components.push(owned.name.into());
            used.supplied.push(owned.name.into());
        }
    }

    // Requirements of inferred extensors are inferred in turn.
    let mut next = 0;
    while next < uses.len() {
        let user = uses[next].name.clone();
        let span = spans[next].clone();
        next += 1;
        for required in &extensors.get(&user).expect("in use").requires {
            if uses.iter().any(|u| &u.name == required) {
                continue;
            }
            match extensors.get(required) {
                Some(meta) if meta.participation == Participation::Inferred => {
                    uses.push(use_of(required, ExtensorReason::RequiredBy(user.clone())));
                    spans.push(span.clone());
                }
                // Explaining a package another app does not include is not the author's problem.
                None => {}
                Some(_) => errors.push(DataError::new(
                    ErrorKind::ExtensorRequires {
                        extensor: user.clone(),
                        requires: required.clone(),
                    },
                    span.as_ref().map(Span::start_location),
                )),
            }
        }
    }

    if errors.is_empty() {
        Ok(uses)
    } else {
        Err(errors)
    }
}

fn use_of(name: &str, reason: ExtensorReason) -> ExtensorUse {
    ExtensorUse {
        name: name.into(),
        reason,
        components: Vec::new(),
        supplied: Vec::new(),
    }
}

fn short_name<'r>(component: &ComponentValue, registry: &'r TypeRegistry) -> &'r str {
    registry
        .get(component.type_id)
        .map_or("?", |r| r.type_info().type_path_table().short_path())
}

/// Builds the `states` section: for each state an extensor contributes, the components it
/// enables and disables. A state of an opt-in extensor needs that extensor named, and so does an
/// enabled component it owns.
pub(crate) fn resolve_states(
    body: &Node,
    uses: &[ExtensorUse],
    extensors: &ExtensorRegistry,
    registry: &TypeRegistry,
) -> Result<Option<StateRules>, Vec<DataError>> {
    let Some(states) = body.get("states").and_then(Node::as_object) else {
        return Ok(None);
    };
    let named = |name: &str| uses.iter().any(|u| u.name == name && u.is_named());
    let mut errors = Vec::new();
    let mut rules = Vec::new();
    for member in states {
        let state = &member.key;
        match extensors.state_owner(state) {
            None => {
                errors.push(DataError::at(
                    ErrorKind::UnknownState {
                        name: state.clone(),
                        known: extensors.states().map(str::to_owned).collect(),
                    },
                    &member.key_span,
                ));
                continue;
            }
            Some(owner) if owner.participation == Participation::OptIn && !named(&owner.name) => {
                errors.push(DataError::at(
                    ErrorKind::StateNeedsExtensor {
                        state: state.clone(),
                        extensor: owner.name.clone(),
                    },
                    &member.key_span,
                ));
                continue;
            }
            Some(_) => {}
        }
        let Some(fields) = member.value.as_object() else {
            errors.push(DataError::at(
                ErrorKind::TypeMismatch {
                    expected: "object with enable and disable".into(),
                    found: member.value.kind_name().into(),
                },
                &member.value.span,
            ));
            continue;
        };
        let mut rule = StateRule {
            state: state.clone(),
            enable: Vec::new(),
            disable: Vec::new(),
        };
        for field in fields {
            match field.key.as_str() {
                "enable" => match build_component_map(&field.value, registry) {
                    Ok(values) => {
                        for value in values {
                            if let Some(owner) = extensors.owner(value.type_id)
                                && owner.participation == Participation::OptIn
                                && !named(&owner.name)
                            {
                                errors.push(DataError::at(
                                    ErrorKind::ExtensorNotNamed {
                                        component: short_name(&value, registry).into(),
                                        extensor: owner.name.clone(),
                                    },
                                    &field.key_span,
                                ));
                            }
                            rule.enable.push((value.type_id, value.value));
                        }
                    }
                    Err(build) => errors.extend(build),
                },
                "disable" => {
                    let Some(items) = field.value.as_array() else {
                        errors.push(DataError::at(
                            ErrorKind::TypeMismatch {
                                expected: "array of component names".into(),
                                found: field.value.kind_name().into(),
                            },
                            &field.value.span,
                        ));
                        continue;
                    };
                    let builder = Builder { registry };
                    for item in items {
                        let found = match item.as_str() {
                            Some(name) => builder
                                .component_registration(name, &item.span)
                                .map(|registration| registration.type_id()),
                            None => Err(DataError::at(
                                ErrorKind::TypeMismatch {
                                    expected: "component name".into(),
                                    found: item.kind_name().into(),
                                },
                                &item.span,
                            )),
                        };
                        match found {
                            Ok(type_id) => rule.disable.push(type_id),
                            Err(error) => errors.push(error),
                        }
                    }
                }
                other => errors.push(DataError::at(
                    ErrorKind::UnknownField {
                        field: other.into(),
                        ty: format!("state {state}"),
                    },
                    &field.key_span,
                )),
            }
        }
        rules.push(rule);
    }
    if errors.is_empty() {
        Ok((!rules.is_empty()).then(|| StateRules(Arc::from(rules))))
    } else {
        Err(errors)
    }
}
