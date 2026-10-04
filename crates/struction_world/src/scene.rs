//! Spawn descriptions: `scenes/**.jsonc` files with `zones` and a `spawnerList`.
//!
//! ```jsonc
//! {
//!   // Optional: a zone's origin in world coordinates. Undeclared zones sit at the origin.
//!   "zones": { "Fortress/LeftCourtYard": { "position": [100, 0, 0], "rotation": [0, 90, 0] } },
//!   "spawnerList": {
//!     "courtyard_guards": {
//!       "zone": "Fortress/LeftCourtYard",
//!       "position": [4, 0, 6],       // in zone coordinates
//!       "rotation": [0, 90, 0],      // optional, Euler degrees, rotates the offsets
//!       "spawns": {
//!         "fireman1": {
//!           "definition": "minions/fireman",
//!           "offset": [0.5, -0.2, 0.0],
//!           "rotation": [0, 0, 0],   // optional, the spawn's own facing
//!           "masterIs": "Fortress/Keep/ogre_lord",
//!           "overrides": { "components": { "Health": { "current": 5 } } }
//!         }
//!       }
//!     }
//!   }
//! }
//! ```
//!
//! A spawner's path is `<zone>/<key>` and a named spawn's `<spawner path>/<key>`. Positions are
//! world or zone coordinates; the streaming cell is derived from them (there is no authored
//! `tile`, so changing the cell size never breaks a spawner).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use struction_data::source::{Member, Span};
use struction_data::{DataError, DefinitionStore, ErrorKind, Node, NodeValue, parse_jsonc};

use crate::identity::EntityPath;

pub const SCENES_DIR: &str = "scenes";

/// Whether a project-relative file is a scene this crate reads, by the same rule as
/// [`SceneCatalog::load_with_sources`]. An editor asks rather than repeating the rule.
pub fn is_scene_file(rel: &str) -> bool {
    rel.starts_with(&format!("{SCENES_DIR}/")) && rel.ends_with(".jsonc")
}

#[derive(Clone, Debug)]
pub struct ZoneDef {
    pub path: EntityPath,
    pub position: Vec3,
    pub rotation: Quat,
}

impl ZoneDef {
    pub fn transform(&self) -> Transform {
        Transform::from_translation(self.position).with_rotation(self.rotation)
    }
}

#[derive(Clone, Debug)]
pub struct SpawnDef {
    /// Authored key location for editors and tool diagnostics.
    pub source: Span,
    /// Key in `spawns`.
    pub name: String,
    pub path: EntityPath,
    pub definition: String,
    /// In the spawner's frame: rotated by the spawner.
    pub offset: Vec3,
    pub rotation: Quat,
    pub master_is: Option<EntityPath>,
    /// Scene overrides for `DefinitionStore::instantiate`, spans pointing into the scene file.
    pub overrides: Option<Node>,
    definition_span: Option<Span>,
    master_span: Option<Span>,
}

#[derive(Clone, Debug)]
pub struct SpawnerDef {
    /// Key in the source file’s `spawnerList`.
    pub name: String,
    pub source: Span,
    pub path: EntityPath,
    pub zone: EntityPath,
    /// In zone coordinates.
    pub position: Vec3,
    pub rotation: Quat,
    pub spawns: Vec<SpawnDef>,
}

impl SpawnerDef {
    /// The spawner's world placement, given its zone's.
    pub fn world_transform(&self, zone: &Transform) -> Transform {
        zone.mul_transform(Transform::from_translation(self.position).with_rotation(self.rotation))
    }

    /// Where a spawn is placed in the world: its offset and rotation in the spawner's frame.
    pub fn spawn_transform(&self, zone: &Transform, spawn: &SpawnDef) -> Transform {
        self.world_transform(zone)
            .mul_transform(Transform::from_translation(spawn.offset).with_rotation(spawn.rotation))
    }
}

/// Every zone and spawner of a project, validated against its definitions.
#[derive(Resource, Default, Debug)]
pub struct SceneCatalog {
    zones: BTreeMap<EntityPath, ZoneDef>,
    spawners: BTreeMap<EntityPath, SpawnerDef>,
    errors: Vec<DataError>,
}

impl SceneCatalog {
    /// Reads `<root>/scenes/**.jsonc`. Invalid spawners and spawns are left out and reported in
    /// [`Self::errors`]; everything else loads.
    pub fn load(root: &Path, store: &DefinitionStore, types: &TypeRegistry) -> Self {
        Self::load_with_sources(root, store, types, &BTreeMap::new())
    }

    /// Compiles candidate scene sources without writing them to disk.
    pub fn load_with_sources(
        root: &Path,
        store: &DefinitionStore,
        types: &TypeRegistry,
        sources: &BTreeMap<String, String>,
    ) -> Self {
        let mut files = Vec::new();
        walk(
            &root.join(SCENES_DIR),
            &format!("{SCENES_DIR}/"),
            &mut files,
        );
        files.extend(
            sources
                .keys()
                .filter(|file| is_scene_file(file))
                .map(|file| (file.clone(), root.join(file))),
        );
        files.sort();
        files.dedup();
        let mut catalog = Self::default();
        for (rel, full) in files {
            let parsed = sources
                .get(&rel)
                .map_or_else(|| fs::read_to_string(&full), |source| Ok(source.clone()))
                .map_err(|e| DataError::new(ErrorKind::Io(format!("cannot read {rel}: {e}")), None))
                .and_then(|text| parse_jsonc(&rel, &text));
            match parsed {
                Ok(root) => catalog.read_file(&root),
                Err(e) => catalog.errors.push(e),
            }
        }
        catalog.validate(store, types);
        for spawner in catalog.spawners.values() {
            catalog
                .zones
                .entry(spawner.zone.clone())
                .or_insert_with(|| ZoneDef {
                    path: spawner.zone.clone(),
                    position: Vec3::ZERO,
                    rotation: Quat::IDENTITY,
                });
        }
        catalog
    }

    pub fn errors(&self) -> &[DataError] {
        &self.errors
    }

    /// Declared zones and those spawners refer to, sorted by path.
    pub fn zones(&self) -> impl Iterator<Item = &ZoneDef> {
        self.zones.values()
    }

    pub fn zone_transform(&self, path: &EntityPath) -> Transform {
        self.zones
            .get(path)
            .map_or(Transform::IDENTITY, ZoneDef::transform)
    }

    /// Sorted by path.
    pub fn spawners(&self) -> impl Iterator<Item = &SpawnerDef> {
        self.spawners.values()
    }

    pub fn spawner(&self, path: &EntityPath) -> Option<&SpawnerDef> {
        self.spawners.get(path)
    }

    fn read_file(&mut self, root: &Node) {
        let Some(members) = root.as_object() else {
            self.errors.push(mismatch("object for a scene file", root));
            return;
        };
        for member in members {
            let result = match member.key.as_str() {
                "$schema" => Ok(()),
                "zones" => self.read_zones(&member.value),
                "spawnerList" => self.read_spawners(&member.value),
                _ => Err(unknown(member, "scene file")),
            };
            if let Err(e) = result {
                self.errors.push(e);
            }
        }
    }

    fn read_zones(&mut self, node: &Node) -> Result<(), DataError> {
        for member in object(node, "object of zones")? {
            let result = self.read_zone(member);
            if let Err(e) = result {
                self.errors.push(e);
            }
        }
        Ok(())
    }

    fn read_zone(&mut self, member: &Member) -> Result<(), DataError> {
        let path = EntityPath::new(&member.key);
        let mut zone = ZoneDef {
            path: path.clone(),
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        };
        for field in object(&member.value, "zone object")? {
            match field.key.as_str() {
                "position" => zone.position = vec3(&field.value)?,
                "rotation" => zone.rotation = rotation(&field.value)?,
                _ => return Err(unknown(field, "zone")),
            }
        }
        if self.zones.insert(path.clone(), zone).is_some() {
            return Err(invalid(
                "zone",
                format!("zone \"{path}\" is declared twice"),
                &member.key_span,
            ));
        }
        Ok(())
    }

    fn read_spawners(&mut self, node: &Node) -> Result<(), DataError> {
        for member in object(node, "object of spawners")? {
            match read_spawner(member, &mut self.errors) {
                Ok(spawner) => {
                    if self.spawners.contains_key(&spawner.path) {
                        self.errors.push(invalid(
                            "spawner",
                            format!("another spawner already has the path \"{}\"", spawner.path),
                            &member.key_span,
                        ));
                    } else {
                        self.spawners.insert(spawner.path.clone(), spawner);
                    }
                }
                Err(e) => self.errors.push(e),
            }
        }
        Ok(())
    }

    /// Checks references once every file is read: definitions exist and overrides build, and
    /// `masterIs` names a spawn.
    fn validate(&mut self, store: &DefinitionStore, types: &TypeRegistry) {
        let spawn_paths: BTreeSet<EntityPath> = self
            .spawners
            .values()
            .flat_map(|s| s.spawns.iter().map(|spawn| spawn.path.clone()))
            .collect();
        let errors = &mut self.errors;
        for spawner in self.spawners.values_mut() {
            spawner.spawns.retain(|spawn| {
                let mut ok = true;
                if store.get(&spawn.definition).is_none() {
                    errors.push(DataError::new(
                        ErrorKind::MissingReference {
                            field: "definition".into(),
                            path: spawn.definition.clone(),
                        },
                        spawn.definition_span.as_ref().map(Span::start_location),
                    ));
                    ok = false;
                } else if let Err(e) =
                    store.instantiate(&spawn.definition, spawn.overrides.as_ref(), types)
                {
                    errors.extend(e);
                    ok = false;
                }
                if let Some(master) = &spawn.master_is
                    && !spawn_paths.contains(master)
                {
                    errors.push(DataError::new(
                        ErrorKind::InvalidValue {
                            ty: "masterIs".into(),
                            message: format!("no spawn has the path \"{master}\""),
                        },
                        spawn.master_span.as_ref().map(Span::start_location),
                    ));
                    ok = false;
                }
                ok
            });
        }
    }
}

fn read_spawner(member: &Member, errors: &mut Vec<DataError>) -> Result<SpawnerDef, DataError> {
    check_name(member, "spawner")?;
    let mut zone = None;
    let mut position = None;
    let mut rotation_value = Quat::IDENTITY;
    let mut spawns_node = None;
    for field in object(&member.value, "spawner object")? {
        let value = &field.value;
        match field.key.as_str() {
            "zone" => zone = Some(EntityPath::new(string(value)?)),
            "position" => position = Some(vec3(value)?),
            "rotation" => rotation_value = rotation(value)?,
            "spawns" => spawns_node = Some(value),
            "tile" => {
                return Err(invalid(
                    "spawner",
                    "\"tile\" is derived from the position; remove it".into(),
                    &field.key_span,
                ));
            }
            _ => return Err(unknown(field, "spawner")),
        }
    }
    let missing: Vec<String> = [("zone", zone.is_none()), ("position", position.is_none())]
        .into_iter()
        .filter(|(_, missing)| *missing)
        .map(|(name, _)| name.to_owned())
        .collect();
    let (Some(zone), Some(position)) = (zone, position) else {
        return Err(DataError::at(
            ErrorKind::MissingField {
                fields: missing,
                ty: "spawner".into(),
            },
            &member.key_span,
        ));
    };
    let path = zone.join(&member.key);
    let mut spawns = Vec::new();
    if let Some(node) = spawns_node {
        for spawn in object(node, "object of spawns")? {
            match read_spawn(spawn, &path) {
                Ok(spawn) => spawns.push(spawn),
                Err(e) => errors.push(e),
            }
        }
    }
    Ok(SpawnerDef {
        name: member.key.clone(),
        source: member.key_span.clone(),
        path,
        zone,
        position,
        rotation: rotation_value,
        spawns,
    })
}

fn read_spawn(member: &Member, spawner: &EntityPath) -> Result<SpawnDef, DataError> {
    check_name(member, "spawn")?;
    let mut spawn = SpawnDef {
        source: member.key_span.clone(),
        name: member.key.clone(),
        path: spawner.join(&member.key),
        definition: String::new(),
        offset: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        master_is: None,
        overrides: None,
        definition_span: None,
        master_span: None,
    };
    for field in object(&member.value, "spawn object")? {
        let value = &field.value;
        match field.key.as_str() {
            "definition" => {
                spawn.definition = string(value)?.to_owned();
                spawn.definition_span = Some(value.span.clone());
            }
            "offset" => spawn.offset = vec3(value)?,
            "rotation" => spawn.rotation = rotation(value)?,
            "masterIs" => {
                spawn.master_is = Some(EntityPath::new(string(value)?));
                spawn.master_span = Some(value.span.clone());
            }
            "overrides" => {
                object(value, "object of overrides")?;
                spawn.overrides = Some(value.clone());
            }
            _ => return Err(unknown(field, "spawn")),
        }
    }
    if spawn.definition_span.is_none() {
        return Err(DataError::at(
            ErrorKind::MissingField {
                fields: vec!["definition".into()],
                ty: "spawn".into(),
            },
            &member.key_span,
        ));
    }
    Ok(spawn)
}

fn check_name(member: &Member, what: &str) -> Result<(), DataError> {
    if member.key.is_empty() || member.key.contains('/') {
        return Err(invalid(
            what,
            format!("name \"{}\" must be non-empty and without '/'", member.key),
            &member.key_span,
        ));
    }
    Ok(())
}

fn mismatch(expected: &str, node: &Node) -> DataError {
    DataError::at(
        ErrorKind::TypeMismatch {
            expected: expected.into(),
            found: node.kind_name().into(),
        },
        &node.span,
    )
}

fn invalid(ty: &str, message: String, span: &Span) -> DataError {
    DataError::at(
        ErrorKind::InvalidValue {
            ty: ty.into(),
            message,
        },
        span,
    )
}

fn unknown(member: &Member, ty: &str) -> DataError {
    DataError::at(
        ErrorKind::UnknownField {
            field: member.key.clone(),
            ty: ty.into(),
        },
        &member.key_span,
    )
}

fn object<'n>(node: &'n Node, what: &str) -> Result<&'n [Member], DataError> {
    node.as_object().ok_or_else(|| mismatch(what, node))
}

fn string(node: &Node) -> Result<&str, DataError> {
    node.as_str().ok_or_else(|| mismatch("string", node))
}

fn vec3(node: &Node) -> Result<Vec3, DataError> {
    let numbers: Option<Vec<f32>> = node.as_array().and_then(|items| {
        items
            .iter()
            .map(|item| match &item.value {
                NodeValue::Number(n) => n.as_f64().map(|f| f as f32),
                _ => None,
            })
            .collect()
    });
    match numbers.as_deref() {
        Some(&[x, y, z]) => Ok(Vec3::new(x, y, z)),
        _ => Err(mismatch("array of 3 numbers", node)),
    }
}

/// Euler angles in degrees, `[x, y, z]`, applied yaw (Y) first, then pitch (X), then roll (Z).
fn rotation(node: &Node) -> Result<Quat, DataError> {
    let degrees = vec3(node)?;
    Ok(Quat::from_euler(
        EulerRot::YXZ,
        degrees.y.to_radians(),
        degrees.x.to_radians(),
        degrees.z.to_radians(),
    ))
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, std::path::PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            walk(&path, &format!("{prefix}{name}/"), out);
        } else if name.ends_with(".jsonc") {
            out.push((format!("{prefix}{name}"), path));
        }
    }
}
