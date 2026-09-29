mod common;

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use common::*;
use struction_core::*;

fn wards_of(app: &App, master: Entity) -> Vec<Entity> {
    app.world()
        .get::<Wards>(master)
        .map(|w| w.to_vec())
        .unwrap_or_default()
}

fn master_of(app: &App, ward: Entity) -> Option<Entity> {
    app.world().get::<MasterIs>(ward).map(|m| m.0)
}

#[test]
fn master_is_populates_wards() {
    let mut app = test_app();
    let boss = named(&mut app, "boss");
    let a = app.world_mut().spawn(MasterIs(boss)).id();
    let b = app.world_mut().spawn(MasterIs(boss)).id();
    assert_eq!(wards_of(&app, boss), vec![a, b]);
    assert_eq!(master_of(&app, a), Some(boss));
}

#[test]
fn reassigning_moves_the_ward_between_masters() {
    let mut app = test_app();
    let player = named(&mut app, "player");
    let boss = named(&mut app, "boss");
    let ogre = app.world_mut().spawn(MasterIs(boss)).id();

    app.world_mut().entity_mut(ogre).insert(MasterIs(player));

    assert_eq!(master_of(&app, ogre), Some(player));
    assert_eq!(wards_of(&app, player), vec![ogre]);
    assert!(wards_of(&app, boss).is_empty());
}

#[test]
fn removing_the_relation_orphans_the_ward() {
    let mut app = test_app();
    let boss = named(&mut app, "boss");
    let ogre = app.world_mut().spawn(MasterIs(boss)).id();

    app.world_mut().entity_mut(ogre).remove::<MasterIs>();

    assert!(app.world().get_entity(ogre).is_ok());
    assert_eq!(master_of(&app, ogre), None);
    assert!(wards_of(&app, boss).is_empty());
}

#[test]
fn despawning_the_master_orphans_wards_by_default() {
    let mut app = test_app();
    let boss = named(&mut app, "boss");
    let a = app.world_mut().spawn(MasterIs(boss)).id();
    let b = app.world_mut().spawn(MasterIs(boss)).id();

    app.world_mut().entity_mut(boss).despawn();

    assert!(app.world().get_entity(a).is_ok());
    assert!(app.world().get_entity(b).is_ok());
    assert_eq!(master_of(&app, a), None);
    assert_eq!(master_of(&app, b), None);
}

#[test]
fn despawning_a_ward_updates_its_master() {
    let mut app = test_app();
    let boss = named(&mut app, "boss");
    let a = app.world_mut().spawn(MasterIs(boss)).id();
    let b = app.world_mut().spawn(MasterIs(boss)).id();

    app.world_mut().entity_mut(a).despawn();

    assert_eq!(wards_of(&app, boss), vec![b]);
}

#[test]
fn wards_of_a_master_that_descend_from_a_type() {
    let mut app = test_app();
    let player = named(&mut app, "player");
    let ogre = app
        .world_mut()
        .spawn((actor("minions/ogre", &["Actor"]), MasterIs(player)))
        .id();
    let small = app
        .world_mut()
        .spawn((
            actor("minions/small_ogre", &["minions/ogre", "Actor"]),
            MasterIs(player),
        ))
        .id();
    let dog = app
        .world_mut()
        .spawn((actor("pets/dog", &["Actor"]), MasterIs(player)))
        .id();
    // An ogre that is somebody else's ward is not included.
    let other = named(&mut app, "other");
    app.world_mut()
        .spawn((actor("minions/ogre", &["Actor"]), MasterIs(other)));

    let (ogres, everyone, master, is_pet) = app
        .world_mut()
        .run_system_once(move |relations: Relations| {
            let ogre_path = DefinitionPath::new("minions/ogre");
            (
                relations
                    .wards_descending_from(player, &ogre_path)
                    .collect::<Vec<_>>(),
                relations.wards_of(player).collect::<Vec<_>>(),
                relations.master_of(small),
                relations.descends_from(dog, &DefinitionPath::new("pets/dog")),
            )
        })
        .unwrap();

    assert_eq!(ogres, vec![ogre, small]);
    assert_eq!(everyone, vec![ogre, small, dog]);
    assert_eq!(master, Some(player));
    assert!(is_pet);
}

#[test]
fn unloading_is_not_removal() {
    let mut app = test_app();
    let boss = named(&mut app, "boss");
    let ogre = app.world_mut().spawn(MasterIs(boss)).id();

    // Streaming unload disables entities instead of deleting or re-parenting them.
    app.world_mut().entity_mut(boss).insert(Disabled);
    assert_eq!(master_of(&app, ogre), Some(boss));
    assert_eq!(wards_of(&app, boss), vec![ogre]);

    app.world_mut().entity_mut(boss).remove::<Disabled>();
    assert_eq!(master_of(&app, ogre), Some(boss));
    assert_eq!(wards_of(&app, boss), vec![ogre]);
}
