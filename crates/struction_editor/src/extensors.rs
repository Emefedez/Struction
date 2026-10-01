//! Extensor provenance for inspection, and the operations that opt a definition into or out of
//! an extensor. Both edit only the definition's own source, as one undoable step.

use serde::Serialize;
use serde_json::{Value, json};
use struction_data::edit::PathSegment;
use struction_data::{
    DefinitionStore, DroppedExtensor, ExtensorReason, ExtensorUse, Suggestion, parse_jsonc,
};

use crate::{Applied, AuthoringProject, SessionError};

/// An extensor a definition uses, and why.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ExtensorEntry {
    pub name: String,
    pub reason: ExtensorWhy,
    /// Components of the definition the extensor owns.
    pub components: Vec<String>,
    /// Those it added with their defaults.
    pub supplied: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensorWhy {
    /// Named in `extensors` by this definition, preset or override.
    NamedBy(String),
    Owns(String),
    RequiredBy(String),
}

impl From<&ExtensorUse> for ExtensorEntry {
    fn from(used: &ExtensorUse) -> Self {
        Self {
            name: used.name.clone(),
            reason: match &used.reason {
                ExtensorReason::Named { by } => ExtensorWhy::NamedBy(by.clone()),
                ExtensorReason::Owns(component) => ExtensorWhy::Owns(component.clone()),
                ExtensorReason::RequiredBy(extensor) => ExtensorWhy::RequiredBy(extensor.clone()),
            },
            components: used.components.clone(),
            supplied: used.supplied.clone(),
        }
    }
}

/// An inherited extensor dropped with `"-name"`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DroppedEntry {
    pub name: String,
    pub by: String,
}

impl From<&DroppedExtensor> for DroppedEntry {
    fn from(dropped: &DroppedExtensor) -> Self {
        Self {
            name: dropped.name.clone(),
            by: dropped.by.clone(),
        }
    }
}

/// An opt-in extensor the definition could use: it builds only on extensors already in use.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SuggestedExtensor {
    pub name: String,
    pub doc: String,
    pub because: Vec<String>,
    /// Components adding it would supply with their defaults.
    pub supplies: Vec<String>,
}

impl From<Suggestion> for SuggestedExtensor {
    fn from(suggestion: Suggestion) -> Self {
        Self {
            name: suggestion.name,
            doc: suggestion.doc,
            because: suggestion.because,
            supplies: suggestion.supplies,
        }
    }
}

/// The parts of a definition's own source these operations change.
struct Own {
    file: String,
    has_list: bool,
    extensors: Vec<String>,
    components: Vec<String>,
}

impl AuthoringProject {
    /// Names `extensor` in the definition at `path`, replacing its own `"-extensor"` drop. Named
    /// extensors add the components they supply, so this is the whole opt-in.
    pub fn add_extensor(&mut self, path: &str, extensor: &str) -> Result<Applied, SessionError> {
        self.refresh()?;
        let store = self.preview().resource::<DefinitionStore>();
        known(store, extensor)?;
        let resolved = definition(store, path)?;
        if resolved
            .extensors
            .iter()
            .any(|e| e.name == extensor && e.is_named())
        {
            return Ok(Applied {
                label: format!("Add extensor {extensor}"),
                files: vec![],
            });
        }
        let parent_names = resolved.lineage.first().is_some_and(|parent| {
            store.get(parent).is_some_and(|p| {
                p.extensors
                    .iter()
                    .any(|e| e.name == extensor && e.is_named())
            })
        });
        let created = self.ensure_override(path)?;
        let own = match self.own(path) {
            Ok(own) => own,
            Err(error) => return self.finish_override(created, Err(error)),
        };
        let mut list: Vec<String> = own
            .extensors
            .iter()
            .filter(|e| e.strip_prefix('-') != Some(extensor))
            .cloned()
            .collect();
        if !parent_names && !list.iter().any(|e| e == extensor) {
            list.push(extensor.to_owned());
        }
        let edits = vec![list_edit(&own, list)].into_iter().flatten().collect();
        self.edit_fields(
            &own.file,
            &format!("Add extensor {extensor}"),
            edits,
            created,
        )
    }

    /// Removes `extensor` from the definition at `path`, with its own components of that
    /// extensor; an inherited one is dropped with `"-extensor"`, which also drops the components
    /// it brought along.
    pub fn remove_extensor(&mut self, path: &str, extensor: &str) -> Result<Applied, SessionError> {
        self.refresh()?;
        let store = self.preview().resource::<DefinitionStore>();
        let meta = known(store, extensor)?.clone();
        let resolved = definition(store, path)?;
        if let Some(user) = resolved.extensors.iter().find(|used| {
            store
                .extensors()
                .get(&used.name)
                .is_some_and(|m| m.requires.iter().any(|r| r == extensor))
        }) {
            return Err(SessionError::InvalidOperation(format!(
                "the {} extensor builds on {extensor}; remove {} first",
                user.name, user.name
            )));
        }
        let used = resolved
            .extensors
            .iter()
            .find(|e| e.name == extensor)
            .map(|e| e.reason.clone());
        let created = self.ensure_override(path)?;
        let own = match self.own(path) {
            Ok(own) => own,
            Err(error) => return self.finish_override(created, Err(error)),
        };
        let owned = |key: &str| {
            meta.components
                .iter()
                .any(|c| key == c.name || key == c.type_path)
        };
        // Named by a library layer (`engine:...`), an ancestor or a preset: drop it here.
        let inherited = match used {
            None if own.extensors.iter().any(|e| e == extensor) => false,
            None => {
                let error = SessionError::InvalidOperation(format!(
                    "{path} does not use the {extensor} extensor"
                ));
                return self.finish_override(created, Err(error));
            }
            Some(ExtensorReason::Named { by }) => by != path,
            Some(ExtensorReason::Owns(component)) => !own.components.contains(&component),
            Some(ExtensorReason::RequiredBy(_)) => unreachable!("refused above"),
        };
        let mut list: Vec<String> = own
            .extensors
            .iter()
            .filter(|e| e.as_str() != extensor)
            .cloned()
            .collect();
        let drop = format!("-{extensor}");
        if inherited && !list.contains(&drop) {
            list.push(drop);
        }
        let mut edits: Vec<_> = list_edit(&own, list).into_iter().collect();
        edits.extend(own.components.iter().filter(|c| owned(c)).map(|c| {
            (
                vec![
                    PathSegment::Key("components".into()),
                    PathSegment::Key(c.clone()),
                ],
                None,
            )
        }));
        self.edit_fields(
            &own.file,
            &format!("Remove extensor {extensor}"),
            edits,
            created,
        )
    }

    fn own(&self, path: &str) -> Result<Own, SessionError> {
        let file = format!("{path}/entity.jsonc");
        let text = self.session().read(&file)?;
        let root = parse_jsonc(&file, &text).map_err(|e| SessionError::Validation(vec![e]))?;
        let strings = |node: Option<&struction_data::Node>| -> Vec<String> {
            node.and_then(|n| n.as_array())
                .unwrap_or_default()
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        };
        Ok(Own {
            has_list: root.get("extensors").is_some(),
            extensors: strings(root.get("extensors")),
            components: root
                .get("components")
                .and_then(|c| c.as_object())
                .unwrap_or_default()
                .iter()
                .map(|m| m.key.clone())
                .collect(),
            file,
        })
    }
}

fn known<'s>(
    store: &'s DefinitionStore,
    extensor: &str,
) -> Result<&'s struction_core::ExtensorMeta, SessionError> {
    store.extensors().get(extensor).ok_or_else(|| {
        let names: Vec<_> = store.extensors().names().collect();
        SessionError::InvalidOperation(format!(
            "unknown extensor {extensor}; registered: {}",
            names.join(", ")
        ))
    })
}

fn definition<'s>(
    store: &'s DefinitionStore,
    path: &str,
) -> Result<&'s struction_data::Resolved, SessionError> {
    store
        .get(path)
        .ok_or_else(|| SessionError::InvalidOperation(format!("unknown definition: {path}")))
}

/// Sets the `extensors` list, or removes the section when it would be empty.
fn list_edit(own: &Own, list: Vec<String>) -> Option<(Vec<PathSegment>, Option<Value>)> {
    let path = vec![PathSegment::Key("extensors".into())];
    match (list.is_empty(), own.has_list) {
        (true, false) => None,
        (true, true) => Some((path, None)),
        (false, _) => Some((path, Some(json!(list)))),
    }
}
