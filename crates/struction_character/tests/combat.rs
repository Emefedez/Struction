use avian3d::prelude::*;
use bevy::prelude::*;
use struction_character::prelude::*;
use struction_core::{ActionArgs, ActionInvocation, ActionQueue, CorePlugin};
use struction_physics::{prelude::*, testing::*};

fn scene() -> (App, Entity) {
    let mut app = headless_app_with((
        CharacterPlugins,
        DodgePlugin,
        CombatPlugin,
        CorePlugin::default(),
    ));
    app.world_mut()
        .spawn(GravityField::scene(Vec3::NEG_Y * 9.81));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 1.0, 100.0),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    let body = app
        .world_mut()
        .spawn((
            CharacterController::default(),
            Attack::default(),
            Roll::default(),
            Transform::from_xyz(0.0, 0.8, 0.0),
        ))
        .id();
    step(&mut app, 30);
    (app, body)
}

/// A light cube resting on the floor `distance` ahead (-Z) of the character.
fn cube(app: &mut App, distance: f32) -> Entity {
    app.world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(0.6, 0.6, 0.6),
            ColliderDensity(80.0),
            Transform::from_xyz(0.0, 0.3, -distance),
        ))
        .id()
}

fn intent(app: &mut App, body: Entity) -> Mut<'_, CharacterIntent> {
    app.world_mut().get_mut::<CharacterIntent>(body).unwrap()
}

fn position(app: &App, entity: Entity) -> Vec3 {
    app.world().get::<Position>(entity).unwrap().0
}

#[test]
fn a_swing_knocks_back_what_is_in_front_once() {
    let (mut app, body) = scene();
    let ahead = cube(&mut app, 1.0);
    let behind = cube(&mut app, -1.5);
    step(&mut app, 20);
    let start = position(&app, ahead);
    intent(&mut app, body).attack_requested = true;
    step(&mut app, 1);
    let swing = app.world().get::<Attacking>(body).unwrap().clone();
    assert!(!swing.struck);
    assert!(swing.direction.abs_diff_eq(Vec3::NEG_Z, 1e-4));
    let mut ticks = 1;
    while app.world().get::<Attacking>(body).is_some() {
        step(&mut app, 1);
        ticks += 1;
        assert!(ticks < 40);
    }
    assert!(app.world().get::<CharacterMove>(body).unwrap().recovery > 0.0);
    step(&mut app, 10);
    let pushed = position(&app, ahead) - start;
    assert!(pushed.z < -0.5, "{pushed}");
    assert!(
        position(&app, behind).abs_diff_eq(Vec3::new(0.0, 0.3, 1.5), 0.05),
        "only what is in front is hit"
    );
}

#[test]
fn one_move_at_a_time_and_jumps_wait() {
    let (mut app, body) = scene();
    {
        let mut intent = intent(&mut app, body);
        intent.roll_requested = true;
        intent.attack_requested = true;
    }
    step(&mut app, 1);
    assert!(app.world().get::<Rolling>(body).is_some());
    assert!(
        app.world().get::<Attacking>(body).is_none(),
        "a roll goes first"
    );
    assert_eq!(
        app.world().get::<CharacterMove>(body).unwrap().active,
        [CharacterCondition::Rolling]
    );

    while app.world().get::<Rolling>(body).is_some() {
        step(&mut app, 1);
    }
    intent(&mut app, body).attack_requested = true;
    step(&mut app, 1);
    assert!(
        app.world().get::<Attacking>(body).is_none(),
        "recovery refuses the next move"
    );
    step(&mut app, 15);
    {
        let mut intent = intent(&mut app, body);
        intent.attack_requested = true;
        intent.jump_requested = true;
    }
    step(&mut app, 2);
    assert!(app.world().get::<Attacking>(body).is_some());
    assert!(
        app.world().get::<LinearVelocity>(body).unwrap().y < 0.5,
        "no jump while attacking"
    );
}

#[test]
fn blocked_while_refuses_and_cancels_moves() {
    let (mut app, body) = scene();
    // In the air: a roll is blocked by default, an attack is not.
    intent(&mut app, body).jump_requested = true;
    step(&mut app, 5);
    assert!(!app.world().get::<CharacterState>(body).unwrap().grounded);
    {
        let mut intent = intent(&mut app, body);
        intent.roll_requested = true;
        intent.attack_requested = true;
    }
    step(&mut app, 1);
    assert!(app.world().get::<Rolling>(body).is_none());
    assert!(app.world().get::<Attacking>(body).is_some());

    // Authoring `Airborne` into the attack's list cancels the swing.
    app.world_mut()
        .get_mut::<Attack>(body)
        .unwrap()
        .blocked_while
        .push(CharacterCondition::Airborne);
    step(&mut app, 1);
    assert!(app.world().get::<Attacking>(body).is_none());
    assert!(
        app.world()
            .get::<CharacterMove>(body)
            .unwrap()
            .active
            .is_empty()
    );
}

#[test]
fn a_roll_without_airborne_in_its_list_carries_on_as_a_dash() {
    let (mut app, body) = scene();
    app.world_mut().get_mut::<Roll>(body).unwrap().blocked_while = vec![];
    intent(&mut app, body).jump_requested = true;
    step(&mut app, 5);
    intent(&mut app, body).roll_requested = true;
    step(&mut app, 10);
    assert!(app.world().get::<Rolling>(body).is_some());
    let velocity = app.world().get::<LinearVelocity>(body).unwrap().0;
    assert!(velocity.z < -5.0, "{velocity}");
}

#[test]
fn the_registered_action_requests_a_swing() {
    let (mut app, body) = scene();
    app.world_mut()
        .resource_mut::<ActionQueue>()
        .invoke(ActionInvocation::new(
            "combat/attack",
            body,
            ActionArgs::new(),
        ));
    step(&mut app, 1);
    assert!(app.world().get::<Attacking>(body).is_some());
}

#[test]
fn without_the_capability_nothing_happens() {
    let (mut app, body) = scene();
    app.world_mut().entity_mut(body).remove::<Attack>();
    intent(&mut app, body).attack_requested = true;
    step(&mut app, 1);
    assert!(app.world().get::<Attacking>(body).is_none());
    assert!(
        !intent(&mut app, body).attack_requested,
        "the press is consumed"
    );
}

#[test]
fn authored_conditions_decide_which_moves_overlap() {
    let (mut app, body) = scene();
    // This character may swing mid-roll, and jump mid-swing.
    {
        let mut attack = app.world_mut().get_mut::<Attack>(body).unwrap();
        attack
            .blocked_while
            .retain(|&c| c != CharacterCondition::Rolling);
        attack.duration = 1.5;
    }
    app.world_mut()
        .get_mut::<CharacterController>(body)
        .unwrap()
        .jump_blocked_while = vec![CharacterCondition::Rolling];
    intent(&mut app, body).roll_requested = true;
    step(&mut app, 3);
    intent(&mut app, body).attack_requested = true;
    step(&mut app, 1);
    assert!(app.world().get::<Rolling>(body).is_some());
    assert!(app.world().get::<Attacking>(body).is_some());
    let mut active = app
        .world()
        .get::<CharacterMove>(body)
        .unwrap()
        .active
        .clone();
    active.sort_by_key(|c| format!("{c:?}"));
    assert_eq!(
        active,
        [CharacterCondition::Attacking, CharacterCondition::Rolling]
    );

    // Once the roll ends, a jump is allowed while the swing goes on.
    while app.world().get::<Rolling>(body).is_some() {
        step(&mut app, 1);
    }
    step(&mut app, 5);
    assert!(app.world().get::<Attacking>(body).is_some());
    intent(&mut app, body).jump_requested = true;
    step(&mut app, 2);
    assert!(app.world().get::<LinearVelocity>(body).unwrap().y > 1.0);
}

#[test]
fn cancel_windows_let_actions_cut_moves_short() {
    let (mut app, body) = scene();
    app.world_mut().get_mut::<Roll>(body).unwrap().cancel_into = vec![CancelInto {
        action: CharacterAction::Jump,
        after: 0.3,
    }];
    app.world_mut().get_mut::<Attack>(body).unwrap().cancel_into = vec![CancelInto {
        action: CharacterAction::Roll,
        after: 0.1,
    }];

    // Too early: the jump is refused and the roll goes on.
    intent(&mut app, body).roll_requested = true;
    step(&mut app, 6);
    intent(&mut app, body).jump_requested = true;
    step(&mut app, 1);
    assert!(app.world().get::<Rolling>(body).is_some());
    // Inside the window: the jump ends the roll at once, without recovery.
    step(&mut app, 12);
    intent(&mut app, body).jump_requested = true;
    step(&mut app, 1);
    assert!(app.world().get::<Rolling>(body).is_none());
    assert_eq!(
        app.world().get::<CharacterMove>(body).unwrap().recovery,
        0.0
    );
    assert!(app.world().get::<LinearVelocity>(body).unwrap().y > 1.0);

    // Landed and recovered: a roll cancels a swing into a roll.
    step(&mut app, 90);
    intent(&mut app, body).attack_requested = true;
    step(&mut app, 8);
    intent(&mut app, body).roll_requested = true;
    step(&mut app, 1);
    assert!(app.world().get::<Attacking>(body).is_none());
    assert!(app.world().get::<Rolling>(body).is_some());
}

#[test]
fn states_switch_components_while_they_hold() {
    use std::sync::Arc;
    use struction_core::{StateRule, StateRules};

    let (mut app, body) = scene();
    let field = GravityField::planet(9.81, 3.0);
    app.world_mut()
        .entity_mut(body)
        .insert(StateRules(Arc::from(vec![StateRule {
            state: "Rolling".into(),
            enable: vec![(std::any::TypeId::of::<GravityField>(), Box::new(field) as _)],
            disable: vec![std::any::TypeId::of::<Attack>()],
        }])));
    step(&mut app, 1);
    assert!(app.world().get::<GravityField>(body).is_none());

    intent(&mut app, body).roll_requested = true;
    step(&mut app, 2);
    assert_eq!(app.world().get::<GravityField>(body), Some(&field));
    assert!(app.world().get::<Attack>(body).is_none());

    while app.world().get::<Rolling>(body).is_some() {
        step(&mut app, 1);
    }
    step(&mut app, 1);
    assert!(app.world().get::<GravityField>(body).is_none());
    assert_eq!(app.world().get::<Attack>(body), Some(&Attack::default()));
}
