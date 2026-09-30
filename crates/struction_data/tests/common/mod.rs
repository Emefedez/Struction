#![allow(dead_code)]

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::reflect::TypeRegistry;

/// Hit points of a living thing.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Health {
    /// Current hit points.
    pub current: f32,
    pub max: f32,
}

/// Marker-ish component with a defaulted field.
#[derive(Component, Reflect, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Flammable {
    pub ignition_temperature: f32,
}

impl Default for Flammable {
    fn default() -> Self {
        Self {
            ignition_temperature: 250.0,
        }
    }
}

#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Faction(pub String);

#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Loot {
    pub items: Vec<String>,
    pub gold: Option<u32>,
}

/// No `Default`: every field must be given once merged.
#[derive(Component, Reflect, Debug, PartialEq)]
#[reflect(Component)]
pub struct Stats {
    pub strength: u32,
    pub agility: u32,
}

#[derive(Reflect, Default, Debug, PartialEq, Clone, Copy)]
pub enum DamageKind {
    #[default]
    Physical,
    Fire,
}

#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Damage {
    pub per_second: f32,
    pub kind: DamageKind,
}

#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Volume {
    pub half_extents: Vec3,
}

#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Surface {
    pub friction: f32,
    pub drag: f32,
}

#[derive(Component, Reflect, Debug, PartialEq)]
#[reflect(Component)]
pub enum Shape {
    Sphere { radius: f32 },
    Box(Vec3),
    Point,
}

/// Registered but not a component.
#[derive(Reflect, Default)]
pub struct Plain {
    pub value: f32,
}

pub fn registry() -> TypeRegistry {
    let mut registry = TypeRegistry::default();
    registry.register::<Health>();
    registry.register::<Flammable>();
    registry.register::<Faction>();
    registry.register::<Loot>();
    registry.register::<Stats>();
    registry.register::<Damage>();
    registry.register::<DamageKind>();
    registry.register::<Volume>();
    registry.register::<Surface>();
    registry.register::<Shape>();
    registry.register::<Plain>();
    registry.register::<Transform>();
    registry
}

pub fn fixture_project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project")
}

/// Writes `files` (relative path, content) into a fresh temp directory.
pub fn write_project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let full = dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, content).unwrap();
    }
    dir
}
