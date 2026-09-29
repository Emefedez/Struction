//! Death, removal and unload are distinct. This is the shared pattern: a system invokes `die`
//! when `Health <= 0`, `die` starts a `Dying` state, and the state's completion invokes another
//! action. Nothing else kills an entity, so no path can skip `die`.

mod common;

use std::time::Duration;

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use common::*;
use struction_core::*;

#[derive(Component)]
struct Health(i32);

#[derive(Component)]
struct Dying {
    timer: Timer,
}

#[derive(Component)]
struct Dead;

#[derive(Resource, Default)]
struct DieCount(u32);

type Alive = (Without<Dying>, Without<Dead>);

const TICK: Duration = Duration::from_millis(500);

fn invoke_die_at_zero_health(
    mut commands: Commands,
    living: Query<(Entity, &Health, &Definition), Alive>,
) {
    for (entity, health, definition) in &living {
        if health.0 <= 0 {
            commands.invoke_action(
                format!("{}/die", definition.path),
                entity,
                ActionArgs::new(),
            );
        }
    }
}

fn finish_dying(mut commands: Commands, mut dying: Query<(Entity, &mut Dying)>) {
    for (entity, mut dying) in &mut dying {
        if dying.timer.tick(TICK).is_finished() {
            commands.invoke_action("finish_dying", entity, ActionArgs::new());
        }
    }
}

fn death_app() -> App {
    let mut app = test_app();
    app.init_resource::<DieCount>()
        .add_systems(
            FixedUpdate,
            (invoke_die_at_zero_health, finish_dying).in_set(CoreSet::Invoke),
        )
        .register_action(
            ActionMeta::new("finish_dying").requires::<Dying>(),
            |In(call): In<ActionCall>, mut commands: Commands, mut log: ResMut<Log>| {
                log.0.push(format!("finished {}", call.target));
                commands.entity(call.target).remove::<Dying>().insert(Dead);
            },
        );
    for path in ["bosses/ogre_lord/die", "minions/ogre/die"] {
        app.register_action(
            ActionMeta::new(path),
            |In(call): In<ActionCall>, mut commands: Commands, mut count: ResMut<DieCount>| {
                count.0 += 1;
                commands.entity(call.target).insert(Dying {
                    timer: Timer::new(Duration::from_secs(1), TimerMode::Once),
                });
            },
        );
    }
    app
}

fn spawn_boss(app: &mut App, health: i32) -> Entity {
    app.world_mut()
        .spawn((
            Definition::new("bosses/ogre_lord", vec![DefinitionPath::new("Actor")]),
            Health(health),
        ))
        .id()
}

fn spawn_minion(app: &mut App, master: Entity) -> Entity {
    app.world_mut()
        .spawn((
            Definition::new("minions/ogre", vec![DefinitionPath::new("Actor")]),
            Health(10),
            MasterIs(master),
            Reactions(vec![Reaction::after(
                ReactionSource::Master,
                "bosses/ogre_lord/die",
                "minions/ogre/die",
            )]),
        ))
        .id()
}

fn dies(app: &App) -> u32 {
    app.world().resource::<DieCount>().0
}

fn has<C: Component>(app: &App, e: Entity) -> bool {
    app.world().get::<C>(e).is_some()
}

#[test]
fn zero_health_invokes_die_once_and_dying_completes_into_another_action() {
    let mut app = death_app();
    let ogre = spawn_boss(&mut app, 5);
    step(&mut app);
    assert_eq!(dies(&app), 0);

    app.world_mut().get_mut::<Health>(ogre).unwrap().0 = 0;
    step(&mut app);
    assert_eq!(dies(&app), 1);
    assert!(has::<Dying>(&app, ogre));

    // The timer needs two ticks; die is not invoked again meanwhile.
    step(&mut app);
    assert!(has::<Dying>(&app, ogre));
    step(&mut app);
    assert!(has::<Dead>(&app, ogre) && !has::<Dying>(&app, ogre));
    step(&mut app);
    assert_eq!(dies(&app), 1);
    assert!(errors(&mut app).is_empty());
}

#[test]
fn a_masters_death_triggers_its_wards_reactions() {
    let mut app = death_app();
    let boss = spawn_boss(&mut app, 100);
    let minion = spawn_minion(&mut app, boss);

    app.world_mut().get_mut::<Health>(boss).unwrap().0 = 0;
    step(&mut app);

    assert_eq!(dies(&app), 2);
    assert!(has::<Dying>(&app, boss));
    assert!(has::<Dying>(&app, minion));
    // The minion died through its reaction, not through its own health.
    assert_eq!(app.world().get::<Health>(minion).unwrap().0, 10);
}

#[test]
fn despawning_the_master_is_not_death() {
    let mut app = death_app();
    let boss = spawn_boss(&mut app, 100);
    let minion = spawn_minion(&mut app, boss);

    app.world_mut().entity_mut(boss).despawn();
    step(&mut app);
    step(&mut app);

    assert_eq!(dies(&app), 0);
    assert!(has::<Health>(&app, minion) && !has::<Dying>(&app, minion));
    assert!(!has::<MasterIs>(&app, minion));
}

#[test]
fn unloading_the_master_is_not_death_and_keeps_the_relation() {
    let mut app = death_app();
    let boss = spawn_boss(&mut app, 100);
    let minion = spawn_minion(&mut app, boss);

    app.world_mut().entity_mut(boss).insert(Disabled);
    step(&mut app);
    step(&mut app);

    assert_eq!(dies(&app), 0);
    assert!(!has::<Dying>(&app, minion));
    assert_eq!(app.world().get::<MasterIs>(minion).unwrap().0, boss);

    // Streaming back in restores the same boss, still alive and still the minion's master.
    app.world_mut().entity_mut(boss).remove::<Disabled>();
    step(&mut app);
    assert_eq!(dies(&app), 0);
    assert_eq!(
        app.world().get::<Wards>(boss).unwrap().to_vec(),
        vec![minion]
    );
}

#[test]
fn removing_the_relation_is_not_death_and_stops_the_reaction() {
    let mut app = death_app();
    let boss = spawn_boss(&mut app, 100);
    let minion = spawn_minion(&mut app, boss);

    app.world_mut().entity_mut(minion).remove::<MasterIs>();
    app.world_mut().get_mut::<Health>(boss).unwrap().0 = 0;
    step(&mut app);

    assert_eq!(dies(&app), 1);
    assert!(has::<Dying>(&app, boss));
    assert!(!has::<Dying>(&app, minion));
}
