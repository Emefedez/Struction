mod common;

use bevy::prelude::*;
use common::*;
use struction_core::*;

fn reacts(app: &mut App, entity: Entity, reactions: Vec<Reaction>) {
    app.world_mut()
        .entity_mut(entity)
        .insert(Reactions(reactions));
}

fn invoke(app: &mut App, action: &str, target: Entity) {
    invoke_action(app.world_mut(), action, target, ActionArgs::new());
    step(app);
}

#[test]
fn before_and_after_hooks_wrap_the_action() {
    let mut app = test_app();
    for action in ["die", "prepare", "mourn"] {
        log_action(&mut app, action);
    }
    let ogre = named(&mut app, "ogre");
    reacts(
        &mut app,
        ogre,
        vec![
            Reaction::after(ReactionSource::This, "die", "mourn"),
            Reaction::before(ReactionSource::This, "die", "prepare"),
        ],
    );

    invoke(&mut app, "die", ogre);

    assert_eq!(log(&app), ["prepare@ogre", "die@ogre", "mourn@ogre"]);
}

#[test]
fn wards_react_to_their_masters_actions() {
    let mut app = test_app();
    for action in ["bosses/ogre_lord/die", "minions/ogre/die"] {
        log_action(&mut app, action);
    }
    let boss = named(&mut app, "boss");
    app.world_mut().spawn((
        Name::new("ward"),
        MasterIs(boss),
        Reactions(vec![Reaction::after(
            ReactionSource::Master,
            "bosses/ogre_lord/die",
            "minions/ogre/die",
        )]),
    ));
    app.world_mut().spawn((
        Name::new("stranger"),
        Reactions(vec![Reaction::after(
            ReactionSource::Master,
            "bosses/ogre_lord/die",
            "minions/ogre/die",
        )]),
    ));

    invoke(&mut app, "bosses/ogre_lord/die", boss);

    // Only the boss's ward reacts, and it runs its own action on itself.
    assert_eq!(
        log(&app),
        ["bosses/ogre_lord/die@boss", "minions/ogre/die@ward"]
    );
}

#[test]
fn a_master_reacts_to_its_wards_actions() {
    let mut app = test_app();
    for action in ["minions/ogre/die", "rally"] {
        log_action(&mut app, action);
    }
    let boss = named(&mut app, "boss");
    reacts(
        &mut app,
        boss,
        vec![Reaction::after(
            ReactionSource::Wards,
            "minions/ogre/die",
            "rally",
        )],
    );
    let ward = app
        .world_mut()
        .spawn((Name::new("ward"), MasterIs(boss)))
        .id();

    invoke(&mut app, "minions/ogre/die", ward);

    assert_eq!(log(&app), ["minions/ogre/die@ward", "rally@boss"]);
}

#[test]
fn reactions_only_fire_for_the_action_they_name() {
    let mut app = test_app();
    for action in ["die", "hit", "mourn"] {
        log_action(&mut app, action);
    }
    let ogre = named(&mut app, "ogre");
    reacts(
        &mut app,
        ogre,
        vec![Reaction::after(ReactionSource::This, "die", "mourn")],
    );

    invoke(&mut app, "hit", ogre);

    assert_eq!(log(&app), ["hit@ogre"]);
}

#[test]
fn state_changes_do_not_trigger_reactions() {
    #[derive(Component)]
    struct Health(i32);

    let mut app = test_app();
    for action in ["die", "mourn"] {
        log_action(&mut app, action);
    }
    let ogre = named(&mut app, "ogre");
    app.world_mut().entity_mut(ogre).insert(Health(10));
    reacts(
        &mut app,
        ogre,
        vec![Reaction::after(ReactionSource::This, "die", "mourn")],
    );

    app.world_mut().get_mut::<Health>(ogre).unwrap().0 = 0;
    step(&mut app);
    step(&mut app);

    assert!(log(&app).is_empty());
}

#[test]
fn reactions_chain_through_the_relation_tree() {
    let mut app = test_app();
    log_action(&mut app, "die");
    let boss = named(&mut app, "boss");
    let mut previous = boss;
    for name in ["captain", "soldier"] {
        previous = app
            .world_mut()
            .spawn((
                Name::new(name),
                MasterIs(previous),
                Reactions(vec![Reaction::after(ReactionSource::Master, "die", "die")]),
            ))
            .id();
    }

    invoke(&mut app, "die", boss);

    assert_eq!(log(&app), ["die@boss", "die@captain", "die@soldier"]);
    assert!(errors(&mut app).is_empty());
}

#[test]
fn reactions_still_fire_when_the_action_orphans_or_despawns_the_target() {
    let mut app = test_app();
    log_action(&mut app, "minions/ogre/die");
    app.register_action(
        ActionMeta::new("bosses/ogre_lord/die"),
        |In(call): In<ActionCall>, mut commands: Commands| {
            commands.entity(call.target).despawn();
        },
    );
    let boss = named(&mut app, "boss");
    app.world_mut().spawn((
        Name::new("ward"),
        MasterIs(boss),
        Reactions(vec![Reaction::after(
            ReactionSource::Master,
            "bosses/ogre_lord/die",
            "minions/ogre/die",
        )]),
    ));

    invoke(&mut app, "bosses/ogre_lord/die", boss);

    assert_eq!(log(&app), ["minions/ogre/die@ward"]);
}

#[test]
fn orphaned_wards_stop_reacting_to_the_former_master() {
    let mut app = test_app();
    for action in ["die", "mourn"] {
        log_action(&mut app, action);
    }
    let boss = named(&mut app, "boss");
    let ward = app
        .world_mut()
        .spawn((
            Name::new("ward"),
            MasterIs(boss),
            Reactions(vec![Reaction::after(
                ReactionSource::Master,
                "die",
                "mourn",
            )]),
        ))
        .id();
    app.world_mut().entity_mut(ward).remove::<MasterIs>();

    invoke(&mut app, "die", boss);

    assert_eq!(log(&app), ["die@boss"]);
}

#[test]
fn reaction_loops_are_cut_with_a_clear_error() {
    let mut app = test_app();
    log_action(&mut app, "ping");
    log_action(&mut app, "pong");
    app.world_mut().insert_resource(ReactionDepthLimit(4));
    let a = named(&mut app, "a");
    reacts(
        &mut app,
        a,
        vec![
            Reaction::after(ReactionSource::This, "ping", "pong"),
            Reaction::after(ReactionSource::This, "pong", "ping"),
        ],
    );

    invoke(&mut app, "ping", a);

    // Depths 0..=4 ran; the sixth is refused.
    assert_eq!(log(&app).len(), 5);
    let errors = errors(&mut app);
    assert_eq!(errors.len(), 1);
    assert!(matches!(
        &errors[0],
        ActionError::DepthExceeded { limit: 4, .. }
    ));
    assert!(errors[0].to_string().contains("depth limit of 4"));
}

#[test]
fn actions_invoking_themselves_are_bounded_too() {
    let mut app = test_app();
    app.world_mut().insert_resource(ReactionDepthLimit(3));
    app.register_action(
        ActionMeta::new("again"),
        |In(call): In<ActionCall>, mut commands: Commands, mut log: ResMut<Log>| {
            log.0.push("again".into());
            commands.invoke_action("again", call.target, ActionArgs::new());
        },
    );
    let a = named(&mut app, "a");

    invoke(&mut app, "again", a);

    assert_eq!(log(&app).len(), 4);
    assert!(matches!(
        errors(&mut app).as_slice(),
        [ActionError::DepthExceeded { limit: 3, .. }]
    ));
    // The next tick starts clean.
    assert!(app.world().resource::<ActionQueue>().is_empty());
}

#[test]
fn reaction_validation_names_missing_actions_and_checks_signatures() {
    let mut app = test_app();
    app.register_action(
        ActionMeta::new("guard").param("radius", ParamType::Float),
        |_: In<ActionCall>| {},
    );
    log_action(&mut app, "die");
    let registry = app.world().resource::<ActionRegistry>();

    let ok = Reaction::after(ReactionSource::Master, "die", "guard")
        .with_args(ActionArgs::new().with("radius", 2.0));
    assert!(ok.validate(registry).is_ok());

    let missing_call = Reaction::after(ReactionSource::Master, "die", "minions/ogre/die");
    let error = missing_call.validate(registry).unwrap_err();
    assert!(error.to_string().contains("minions/ogre/die"));

    let missing_source = Reaction::after(ReactionSource::Master, "bosses/x/die", "guard");
    let error = missing_source.validate(registry).unwrap_err();
    assert!(error.to_string().contains("bosses/x/die"));

    let bad_args = Reaction::after(ReactionSource::Master, "die", "guard");
    assert!(matches!(
        bad_args.validate(registry),
        Err(ActionError::MissingArg { .. })
    ));
}
