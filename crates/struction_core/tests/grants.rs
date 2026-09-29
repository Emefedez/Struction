mod common;

use bevy::prelude::*;
use common::*;
use struction_core::*;

#[derive(Component, Reflect, Default, Clone, PartialEq, Debug)]
#[reflect(Component)]
struct Follower {
    distance: f32,
}

#[derive(Component, Reflect, Default, Clone, PartialEq, Debug)]
#[reflect(Component)]
struct Rallied;

fn grant_app() -> App {
    let mut app = test_app();
    app.register_type::<Follower>().register_type::<Rallied>();
    app
}

fn player(app: &mut App) -> Entity {
    let rules = GrantsToWards(vec![
        GrantRule::new("minions/ogre")
            .component(Follower { distance: 3.0 })
            .action("fetch")
            .action("guard"),
    ]);
    app.world_mut().spawn((Name::new("player"), rules)).id()
}

fn ogre(app: &mut App, master: Entity) -> Entity {
    app.world_mut()
        .spawn((actor("minions/ogre", &["Actor"]), MasterIs(master)))
        .id()
}

fn follower(app: &App, e: Entity) -> Option<f32> {
    app.world().get::<Follower>(e).map(|f| f.distance)
}

fn actions(app: &App, e: Entity) -> Vec<String> {
    app.world()
        .get::<ActionSet>(e)
        .map(|s| s.iter().map(|a| a.to_string()).collect())
        .unwrap_or_default()
}

#[test]
fn grants_apply_when_the_relation_starts_and_end_with_it() {
    let mut app = grant_app();
    let player = player(&mut app);
    let ward = ogre(&mut app, player);

    assert_eq!(follower(&app, ward), Some(3.0));
    assert_eq!(actions(&app, ward), ["fetch", "guard"]);

    app.world_mut().entity_mut(ward).remove::<MasterIs>();

    assert_eq!(follower(&app, ward), None);
    assert!(actions(&app, ward).is_empty());
    assert!(app.world().get::<GrantRecord>(ward).is_none());
}

#[test]
fn lineage_filter_includes_descendants_and_excludes_others() {
    let mut app = grant_app();
    let player = player(&mut app);
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
    let unknown = app.world_mut().spawn(MasterIs(player)).id();

    assert_eq!(follower(&app, small), Some(3.0));
    assert_eq!(follower(&app, dog), None);
    assert_eq!(follower(&app, unknown), None);
    assert!(actions(&app, dog).is_empty());
}

#[test]
fn a_component_the_ward_already_had_is_not_removed() {
    let mut app = grant_app();
    let player = player(&mut app);
    let ward = app
        .world_mut()
        .spawn((
            actor("minions/ogre", &["Actor"]),
            Follower { distance: 9.0 },
            ActionSet::from_iter([ActionName::new("fetch")]),
        ))
        .id();

    app.world_mut().entity_mut(ward).insert(MasterIs(player));

    // Its own value wins; only what was really added is recorded.
    assert_eq!(follower(&app, ward), Some(9.0));
    assert_eq!(actions(&app, ward), ["fetch", "guard"]);
    let record = app.world().get::<GrantRecord>(ward).unwrap();
    assert!(record.components.is_empty());
    assert_eq!(record.actions, [ActionName::new("guard")]);

    app.world_mut().entity_mut(ward).remove::<MasterIs>();

    assert_eq!(follower(&app, ward), Some(9.0));
    assert_eq!(actions(&app, ward), ["fetch"]);
}

#[test]
fn a_ward_can_refuse_grants() {
    let mut app = grant_app();
    let player = player(&mut app);

    let picky = app
        .world_mut()
        .spawn((
            actor("minions/ogre", &["Actor"]),
            RefusesGrants::default()
                .component::<Follower>()
                .action("guard"),
            MasterIs(player),
        ))
        .id();
    assert_eq!(follower(&app, picky), None);
    assert_eq!(actions(&app, picky), ["fetch"]);

    let stubborn = app
        .world_mut()
        .spawn((
            actor("minions/ogre", &["Actor"]),
            RefusesGrants::everything(),
            MasterIs(player),
        ))
        .id();
    assert_eq!(follower(&app, stubborn), None);
    assert!(actions(&app, stubborn).is_empty());
}

#[test]
fn changing_master_swaps_the_grants() {
    let mut app = grant_app();
    let player = player(&mut app);
    let boss = app
        .world_mut()
        .spawn(GrantsToWards(vec![
            GrantRule::new("minions/ogre")
                .component(Follower { distance: 5.0 })
                .component(Rallied)
                .action("rally"),
        ]))
        .id();
    let ward = ogre(&mut app, player);

    app.world_mut().entity_mut(ward).insert(MasterIs(boss));

    assert_eq!(follower(&app, ward), Some(5.0));
    assert!(app.world().get::<Rallied>(ward).is_some());
    assert_eq!(actions(&app, ward), ["rally"]);

    app.world_mut().entity_mut(ward).insert(MasterIs(player));

    assert_eq!(follower(&app, ward), Some(3.0));
    assert!(app.world().get::<Rallied>(ward).is_none());
    assert_eq!(actions(&app, ward), ["fetch", "guard"]);
}

#[test]
fn despawning_the_master_revokes_grants() {
    let mut app = grant_app();
    let player = player(&mut app);
    let ward = ogre(&mut app, player);
    assert_eq!(follower(&app, ward), Some(3.0));

    app.world_mut().entity_mut(player).despawn();

    assert!(app.world().get_entity(ward).is_ok());
    assert_eq!(follower(&app, ward), None);
    assert!(actions(&app, ward).is_empty());
}

#[test]
fn despawning_a_ward_is_harmless() {
    let mut app = grant_app();
    let player = player(&mut app);
    let ward = ogre(&mut app, player);

    app.world_mut().entity_mut(ward).despawn();

    assert!(app.world().get::<Wards>(player).is_none());
}

#[test]
fn unregistered_component_types_are_skipped_without_panicking() {
    let mut app = test_app(); // Follower is not registered here.
    let player = player(&mut app);
    let ward = ogre(&mut app, player);

    assert_eq!(follower(&app, ward), None);
    assert_eq!(actions(&app, ward), ["fetch", "guard"]);
}
