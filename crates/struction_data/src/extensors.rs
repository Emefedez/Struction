//! Which extensors a resolved definition uses, and why.
//!
//! Named extensors accumulate along `descendsFrom`, presets and overrides instead of replacing
//! each other like arrays do. Components of an opt-in extensor are refused unless it is named;
//! naming one adds the components it supplies with their defaults. Every other extensor is
//! inferred from the components it owns or from an extensor that requires it.

use bevy::reflect::TypeRegistry;
use bevy::reflect::std_traits::ReflectDefault;
use struction_core::{ExtensorRegistry, Participation};

use crate::build::ComponentValue;
use crate::error::{DataError, ErrorKind};
use crate::source::{Node, Span};

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
