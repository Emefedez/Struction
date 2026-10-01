//! Identity: paths for authored things, UUIDs for runtime-created entities, and type lineage.

use std::fmt;
use std::sync::Arc;

use bevy::prelude::*;
use uuid::Uuid;

/// Path of an authored definition, such as `minions/ogre` or the primordial `Actor`.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct DefinitionPath(Arc<str>);

impl DefinitionPath {
    pub fn new(path: impl AsRef<str>) -> Self {
        Self(path.as_ref().into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Primordial engine types are a single capitalized segment (`Actor`, `Terrain`).
    pub fn is_primordial(&self) -> bool {
        !self.0.contains('/') && self.0.chars().next().is_some_and(char::is_uppercase)
    }
}

impl fmt::Display for DefinitionPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for DefinitionPath {
    fn from(path: &str) -> Self {
        Self::new(path)
    }
}

impl From<String> for DefinitionPath {
    fn from(path: String) -> Self {
        Self::new(path)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IdentityError {
    #[error("lineage of `{path}` contains `{repeated}` more than once or contains the path itself")]
    LineageCycle {
        path: DefinitionPath,
        repeated: DefinitionPath,
    },
    #[error("lineage of `{path}` begins in `{root}`, which is not a primordial (capitalized) type")]
    NotPrimordial {
        path: DefinitionPath,
        root: DefinitionPath,
    },
    #[error(
        "`{path}` is a primordial type inside the lineage of `{child}`, but only the first entry may be one"
    )]
    PrimordialInside {
        child: DefinitionPath,
        path: DefinitionPath,
    },
}

/// The definition an instance was spawned from, with its resolved `descendsFrom` chain.
///
/// `descendsFrom` is data and never changes at runtime, so this component is filled by whatever
/// loads the data and treated as immutable afterwards. It is derived from the definition, not saved.
#[derive(Component, Clone, PartialEq, Eq, Debug)]
pub struct Definition {
    pub path: DefinitionPath,
    /// Ancestors, the primordial type first (`small_ogre` -> `[Actor, minions/ogre]`), so a chain
    /// reads from the root down to the definition itself.
    pub lineage: Vec<DefinitionPath>,
}

impl Definition {
    pub fn new(path: impl Into<DefinitionPath>, lineage: Vec<DefinitionPath>) -> Self {
        Self {
            path: path.into(),
            lineage,
        }
    }

    /// Whether this definition is `ancestor` or has it in its lineage. An instance of
    /// `minions/small_ogre` is a `minions/ogre`, so a match includes the definition itself.
    pub fn descends_from(&self, ancestor: &DefinitionPath) -> bool {
        self.path == *ancestor || self.lineage.contains(ancestor)
    }

    /// Checks the lineage invariants: no repeats and a single primordial root at the start.
    pub fn validate(&self) -> Result<(), IdentityError> {
        for (i, ancestor) in self.lineage.iter().enumerate() {
            if *ancestor == self.path || self.lineage[..i].contains(ancestor) {
                return Err(IdentityError::LineageCycle {
                    path: self.path.clone(),
                    repeated: ancestor.clone(),
                });
            }
            if ancestor.is_primordial() && i > 0 {
                return Err(IdentityError::PrimordialInside {
                    child: self.path.clone(),
                    path: ancestor.clone(),
                });
            }
        }
        let root = self.lineage.first().unwrap_or(&self.path);
        if !root.is_primordial() {
            return Err(IdentityError::NotPrimordial {
                path: self.path.clone(),
                root: root.clone(),
            });
        }
        Ok(())
    }
}

/// Identity of a runtime-created entity, stable across saves.
///
/// Authored entities are identified by path instead.
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct StableId(pub Uuid);

impl fmt::Display for StableId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Seeded source of [`StableId`]s, so a simulation replays with the same identities.
///
/// The output is a well-formed version 4 UUID; only the entropy source differs.
#[derive(Resource, Clone, Debug)]
pub struct StableIdGenerator {
    state: u64,
}

impl StableIdGenerator {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_id(&mut self) -> StableId {
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&self.next_u64().to_le_bytes());
        bytes[8..].copy_from_slice(&self.next_u64().to_le_bytes());
        StableId(uuid::Builder::from_random_bytes(bytes).into_uuid())
    }

    // SplitMix64.
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

impl Default for StableIdGenerator {
    fn default() -> Self {
        Self::new(0)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn path(s: &str) -> DefinitionPath {
        DefinitionPath::new(s)
    }

    fn small_ogre() -> Definition {
        Definition::new(
            "minions/small_ogre",
            vec![path("minions/ogre"), path("Actor")],
        )
    }

    #[test]
    fn descends_from_includes_self_and_ancestors() {
        let def = small_ogre();
        assert!(def.descends_from(&path("minions/small_ogre")));
        assert!(def.descends_from(&path("minions/ogre")));
        assert!(def.descends_from(&path("Actor")));
        assert!(!def.descends_from(&path("player")));
        assert!(!def.descends_from(&path("Terrain")));
    }

    #[test]
    fn primordial_types_are_single_capitalized_segments() {
        assert!(path("Actor").is_primordial());
        assert!(!path("minions/Ogre").is_primordial());
        assert!(!path("player").is_primordial());
        assert!(!path("").is_primordial());
    }

    #[test]
    fn lineage_must_end_in_a_primordial() {
        assert_eq!(small_ogre().validate(), Ok(()));
        assert_eq!(Definition::new("Actor", vec![]).validate(), Ok(()));
        assert_eq!(
            Definition::new("minions/ogre", vec![]).validate(),
            Err(IdentityError::NotPrimordial {
                path: path("minions/ogre"),
                root: path("minions/ogre"),
            })
        );
        assert!(matches!(
            Definition::new("a/b", vec![path("a/c")]).validate(),
            Err(IdentityError::NotPrimordial { .. })
        ));
    }

    #[test]
    fn lineage_rejects_cycles_and_inner_primordials() {
        let cyclic = Definition::new("a/b", vec![path("a/c"), path("a/b"), path("Actor")]);
        assert!(matches!(
            cyclic.validate(),
            Err(IdentityError::LineageCycle { .. })
        ));
        let inner = Definition::new("a/b", vec![path("Actor"), path("a/c")]);
        assert!(matches!(
            inner.validate(),
            Err(IdentityError::PrimordialInside { .. })
        ));
    }

    #[test]
    fn stable_ids_are_unique_v4_and_replayable() {
        let mut a = StableIdGenerator::new(7);
        let mut b = StableIdGenerator::new(7);
        let ids: Vec<_> = (0..1000).map(|_| a.next_id()).collect();
        assert!(ids.iter().all(|id| id.0.get_version_num() == 4));
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        assert!(ids.iter().all(|id| *id == b.next_id()));
        assert_ne!(StableIdGenerator::new(8).next_id(), ids[0]);
    }
}
