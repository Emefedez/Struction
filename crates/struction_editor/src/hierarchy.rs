//! Relationship trees and scene authoring shared by the GUI and JSONL clients.
use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::Vec3;
use serde::Serialize;
use serde_json::json;
use struction_world::SceneCatalog;

use crate::{Applied, AuthoringProject, EditRequest, Field, SessionError};

#[derive(Clone, Debug, Serialize)]
pub struct HierarchyNode {
    pub key: String,
    pub children: Vec<HierarchyNode>,
}

// Keep malformed/runtime cycles visible once, without unbounded recursion.
fn forest(parents: BTreeMap<String, Option<String>>) -> Vec<HierarchyNode> {
    fn branch(
        key: &str,
        parents: &BTreeMap<String, Option<String>>,
        seen: &mut BTreeSet<String>,
    ) -> Option<HierarchyNode> {
        if !seen.insert(key.into()) {
            return None;
        }
        Some(HierarchyNode {
            key: key.into(),
            children: parents
                .iter()
                .filter(|(_, parent)| parent.as_deref() == Some(key))
                .filter_map(|(child, _)| branch(child, parents, seen))
                .collect(),
        })
    }
    let mut seen = BTreeSet::new();
    let mut roots: Vec<_> = parents
        .iter()
        .filter(|(_, parent)| parent.as_ref().is_none_or(|p| !parents.contains_key(p)))
        .filter_map(|(key, _)| branch(key, &parents, &mut seen))
        .collect();
    for key in parents.keys() {
        if let Some(node) = branch(key, &parents, &mut seen) {
            roots.push(node);
        }
    }
    roots
}

impl AuthoringProject {
    /// Instances nested beneath their `masterIs`, independently of source folders/spawners.
    pub fn master_hierarchy(&self, playing: bool) -> Result<Vec<HierarchyNode>, SessionError> {
        Ok(forest(
            self.entities(playing)?
                .into_iter()
                .filter(|e| e.is_instance())
                .map(|e| (e.key().to_owned(), e.master))
                .collect(),
        ))
    }

    /// Definitions nested by `descendsFrom`; this is type inheritance, not ownership.
    pub fn definition_hierarchy(&self) -> Vec<HierarchyNode> {
        forest(
            self.definitions()
                .into_iter()
                .map(|path| {
                    let parent = self
                        .inspect_definition(&path)
                        .ok()
                        .and_then(|d| d.lineage.into_iter().find(|ancestor| ancestor != &path))
                        .or_else(|| {
                            let file = format!("{path}/entity.jsonc");
                            let source = self.session().read(&file).ok()?;
                            let data = struction_data::parse_jsonc(&file, &source).ok()?.to_value();
                            data.get("descendsFrom")?.as_str().map(str::to_owned)
                        });
                    (path, parent)
                })
                .collect(),
        )
    }

    pub fn set_master(
        &mut self,
        path: &str,
        master: Option<&str>,
    ) -> Result<Applied, SessionError> {
        if self.session().is_playing() {
            return Err(SessionError::Playing);
        }
        self.refresh()?;
        self.check_master(path, master)?;
        let entry = self
            .entities(false)?
            .into_iter()
            .find(|e| e.key() == path && e.is_named_spawn())
            .ok_or_else(|| {
                SessionError::InvalidOperation(format!("unknown authored spawn: {path}"))
            })?;
        let source = entry.source.expect("named spawn");
        let mut fields: Vec<_> = source.path.into_iter().map(Field::Key).collect();
        fields.push(Field::Key("masterIs".into()));
        self.edit(match master {
            Some(master) => EditRequest::Set {
                file: source.file,
                path: fields,
                value: json!(master),
                label: "Set master".into(),
                group: None,
                revision: None,
            },
            None => EditRequest::Remove {
                file: source.file,
                path: fields,
                label: "Clear master".into(),
                revision: None,
            },
        })
    }

    fn check_master(&self, path: &str, master: Option<&str>) -> Result<(), SessionError> {
        let entries = self.entities(false)?;
        let Some(mut key) = master else {
            return Ok(());
        };
        if !entries.iter().any(|e| e.key() == key && e.is_named_spawn()) {
            return Err(SessionError::InvalidOperation(format!(
                "master must name an authored spawn: {key}"
            )));
        }
        let mut seen = BTreeSet::from([path.to_owned()]);
        loop {
            if !seen.insert(key.to_owned()) {
                return Err(SessionError::InvalidOperation(format!(
                    "masterIs would create a cycle through {key}"
                )));
            }
            match entries
                .iter()
                .find(|e| e.key() == key)
                .and_then(|e| e.master.as_deref())
            {
                Some(parent) => key = parent,
                None => return Ok(()),
            }
        }
    }

    /// Creates a named instance in an existing spawner. Undo restores the scene byte for byte.
    /// `offset` is relative to that spawner, like the scene format.
    pub fn create_spawn(
        &mut self,
        spawner: &str,
        name: &str,
        definition: &str,
        master: Option<&str>,
        offset: Vec3,
    ) -> Result<Applied, SessionError> {
        if self.session().is_playing() {
            return Err(SessionError::Playing);
        }
        if name.trim().is_empty()
            || name.contains(['/', '\\'])
            || name == "."
            || name == ".."
            || name.chars().any(char::is_control)
        {
            return Err(SessionError::InvalidOperation(
                "spawn name must be one nonempty path segment".into(),
            ));
        }
        if !offset.is_finite() {
            return Err(SessionError::InvalidOperation(
                "offset must be finite".into(),
            ));
        }
        self.refresh()?;
        let path = format!("{spawner}/{name}");
        self.check_master(&path, master)?;
        let scenes = self.preview().resource::<SceneCatalog>();
        let spawner = scenes
            .spawners()
            .find(|s| s.path.as_str() == spawner)
            .ok_or_else(|| SessionError::InvalidOperation(format!("unknown spawner: {spawner}")))?;
        if spawner.spawns.iter().any(|s| s.name == name) {
            return Err(SessionError::InvalidOperation(format!(
                "spawn already exists: {path}"
            )));
        }
        let mut value = json!({"definition": definition, "offset": offset.to_array()});
        if let Some(master) = master {
            value["masterIs"] = json!(master);
        }
        self.edit(EditRequest::Set {
            file: spawner.source.file.to_string(),
            path: ["spawnerList", &spawner.name, "spawns", name]
                .into_iter()
                .map(|k| Field::Key(k.into()))
                .collect(),
            value,
            label: format!("Create {path}"),
            group: None,
            revision: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cycles_and_missing_parents_stay_visible() {
        let roots = forest(BTreeMap::from([
            ("a".into(), Some("b".into())),
            ("b".into(), Some("a".into())),
            ("c".into(), Some("missing".into())),
        ]));
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].key, "c");
        assert_eq!(roots[1].children[0].key, "b");
    }
}
