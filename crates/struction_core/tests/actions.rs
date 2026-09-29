mod common;

use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use common::*;
use struction_core::*;

#[derive(Component)]
struct Health(i32);

#[test]
fn resolve_error_names_the_missing_action() {
    let mut app = test_app();
    log_action(&mut app, "minions/ogre/hit");
    let registry = app.world().resource::<ActionRegistry>();

    assert!(registry.resolve(&"minions/ogre/hit".into()).is_ok());
    let error = registry.resolve(&"minions/ogre/die".into()).unwrap_err();
    assert_eq!(
        error,
        ActionError::Unknown {
            name: "minions/ogre/die".into()
        }
    );
    assert!(error.to_string().contains("minions/ogre/die"));
}

#[test]
fn duplicate_registration_is_an_error() {
    let mut app = test_app();
    log_action(&mut app, "a");
    let result = ActionRegistry::register(
        app.world_mut(),
        ActionMeta::new("a"),
        |_: In<ActionCall>| {},
    );
    assert!(matches!(result, Err(ActionError::Duplicate { .. })));
}

#[test]
fn descriptors_expose_metadata_sorted_by_name() {
    let mut app = test_app();
    app.register_action(
        ActionMeta::new("minions/ogre/hit")
            .doc("Deals damage")
            .param("damage", ParamType::Float)
            .requires::<Health>(),
        |_: In<ActionCall>| {},
    );
    log_action(&mut app, "bosses/ogre_lord/die");

    let descriptors = app.world().resource::<ActionRegistry>().descriptors();

    let names: Vec<_> = descriptors.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["bosses/ogre_lord/die", "minions/ogre/hit"]);
    let hit = &descriptors[1];
    assert_eq!(hit.doc, "Deals damage");
    assert_eq!(hit.params[0].name, "damage");
    assert_eq!(hit.params[0].ty, ParamType::Float);
    assert!(hit.requires[0].ends_with("Health"));
}

#[test]
fn actions_run_with_validated_args_at_dispatch() {
    let mut app = test_app();
    app.register_action(
        ActionMeta::new("hit")
            .param("damage", ParamType::Float)
            .param_or("times", ParamType::Int, 2)
            .requires::<Health>(),
        |In(call): In<ActionCall>, mut health: Query<&mut Health>| {
            let times = call.args.int("times").unwrap();
            health.get_mut(call.target).unwrap().0 -=
                (call.args.float("damage").unwrap() as i32) * times as i32;
        },
    );
    let ogre = app.world_mut().spawn(Health(100)).id();

    invoke_action(
        app.world_mut(),
        "hit",
        ogre,
        ActionArgs::new().with("damage", 10),
    );
    // Queued, not executed, until the dispatch phase.
    assert_eq!(app.world().get::<Health>(ogre).unwrap().0, 100);
    step(&mut app);

    assert_eq!(app.world().get::<Health>(ogre).unwrap().0, 80);
    assert!(errors(&mut app).is_empty());
}

#[test]
fn invalid_invocations_are_reported_and_skipped() {
    let mut app = test_app();
    app.register_action(
        ActionMeta::new("hit")
            .param("damage", ParamType::Float)
            .requires::<Health>(),
        |_: In<ActionCall>, mut log: ResMut<Log>| log.0.push("ran".into()),
    );
    let ogre = app.world_mut().spawn(Health(1)).id();
    let rock = app.world_mut().spawn_empty().id();
    let gone = app.world_mut().spawn_empty().id();
    app.world_mut().entity_mut(gone).despawn();

    let world = app.world_mut();
    invoke_action(world, "nope", ogre, ActionArgs::new());
    invoke_action(world, "hit", ogre, ActionArgs::new());
    invoke_action(
        world,
        "hit",
        ogre,
        ActionArgs::new().with("damage", "a lot"),
    );
    invoke_action(world, "hit", rock, ActionArgs::new().with("damage", 1.0));
    invoke_action(world, "hit", gone, ActionArgs::new().with("damage", 1.0));
    step(&mut app);

    assert!(log(&app).is_empty());
    let errors = errors(&mut app);
    assert!(matches!(&errors[0], ActionError::Unknown { name } if name.as_str() == "nope"));
    assert!(matches!(&errors[1], ActionError::MissingArg { param, .. } if param == "damage"));
    assert!(matches!(&errors[2], ActionError::ArgType { .. }));
    assert!(matches!(&errors[3], ActionError::MissingComponent { .. }));
    assert!(matches!(&errors[4], ActionError::TargetMissing { .. }));
    assert_eq!(errors.len(), 5);
}

#[test]
fn actions_can_invoke_other_actions_which_run_in_the_same_dispatch() {
    let mut app = test_app();
    log_action(&mut app, "second");
    app.register_action(
        ActionMeta::new("first"),
        |In(call): In<ActionCall>, mut commands: Commands, mut log: ResMut<Log>| {
            log.0.push("first".into());
            commands.invoke_action("second", call.target, ActionArgs::new());
        },
    );
    let ogre = named(&mut app, "ogre");

    invoke_action(app.world_mut(), "first", ogre, ActionArgs::new());
    step(&mut app);

    assert_eq!(log(&app), ["first", "second@ogre"]);
}

#[test]
fn phases_run_in_declared_order_regardless_of_registration_order() {
    let mut app = test_app();
    log_action(&mut app, "die");
    // Registered in the "wrong" order on purpose.
    app.add_systems(
        FixedUpdate,
        (|mut log: ResMut<Log>| log.0.push("post".into())).in_set(CoreSet::Post),
    );
    app.add_systems(
        FixedUpdate,
        (|mut commands: Commands, q: Query<Entity, With<Health>>, mut log: ResMut<Log>| {
            log.0.push("invoke".into());
            for e in &q {
                commands.invoke_action("die", e, ActionArgs::new());
            }
        })
        .in_set(CoreSet::Invoke),
    );
    app.world_mut().spawn((Name::new("ogre"), Health(0)));

    step(&mut app);

    assert_eq!(log(&app), ["invoke", "die@ogre", "post"]);
}

#[test]
fn plugin_runs_in_the_fixed_timestep_under_the_real_runner_setup() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, CorePlugin::default()))
        .init_resource::<Log>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            100,
        )))
        .insert_resource(Time::<Fixed>::from_duration(Duration::from_millis(100)));
    log_action(&mut app, "ping");
    let ogre = named(&mut app, "ogre");
    invoke_action(app.world_mut(), "ping", ogre, ActionArgs::new());

    for _ in 0..3 {
        app.update();
    }

    assert_eq!(log(&app), ["ping@ogre"]);
}
