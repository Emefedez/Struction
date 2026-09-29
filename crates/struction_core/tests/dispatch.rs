mod common;

use bevy::prelude::*;
use common::*;
use struction_core::*;

fn boss_with_wards(app: &mut App, names: &[&'static str]) -> (Entity, Vec<Entity>) {
    let boss = named(app, "boss");
    let wards = names
        .iter()
        .map(|name| {
            app.world_mut()
                .spawn((Name::new(*name), MasterIs(boss)))
                .id()
        })
        .collect();
    (boss, wards)
}

#[test]
fn notify_invokes_the_action_on_every_ward() {
    let mut app = test_app();
    log_action(&mut app, "scatter");
    let (boss, _) = boss_with_wards(&mut app, &["a", "b"]);
    named(&mut app, "bystander");

    notify_wards(app.world_mut(), boss, "scatter", ActionArgs::new());
    step(&mut app);

    assert_eq!(log(&app), ["scatter@a", "scatter@b"]);
}

#[test]
fn notified_wards_trigger_their_own_reactions() {
    let mut app = test_app();
    log_action(&mut app, "scatter");
    log_action(&mut app, "hide");
    let (boss, wards) = boss_with_wards(&mut app, &["a"]);
    app.world_mut()
        .entity_mut(wards[0])
        .insert(Reactions(vec![Reaction::after(
            ReactionSource::This,
            "scatter",
            "hide",
        )]));

    notify_wards(app.world_mut(), boss, "scatter", ActionArgs::new());
    step(&mut app);

    assert_eq!(log(&app), ["scatter@a", "hide@a"]);
}

#[test]
fn notify_through_commands_from_a_system() {
    let mut app = test_app();
    log_action(&mut app, "scatter");
    let (boss, _) = boss_with_wards(&mut app, &["a", "b"]);
    app.add_systems(
        FixedUpdate,
        (move |mut commands: Commands, mut done: Local<bool>| {
            if !*done {
                commands.notify_wards(boss, "scatter", ActionArgs::new());
                *done = true;
            }
        })
        .in_set(CoreSet::Invoke),
    );

    step(&mut app);
    step(&mut app);

    assert_eq!(log(&app), ["scatter@a", "scatter@b"]);
}

#[test]
fn orders_are_queued_on_wards_for_their_decision_tree() {
    let mut app = test_app();
    log_action(&mut app, "guard");
    let (boss, wards) = boss_with_wards(&mut app, &["a", "b"]);
    let bystander = named(&mut app, "bystander");

    order_wards(
        app.world_mut(),
        boss,
        "guard",
        ActionArgs::new().with("radius", 4.0),
    );
    step(&mut app);

    // Ordering does not execute anything.
    assert!(log(&app).is_empty());
    for ward in wards {
        let orders = app.world().get::<Orders>(ward).unwrap();
        let order = orders.current().unwrap();
        assert_eq!(order.action, ActionName::new("guard"));
        assert_eq!(order.args.float("radius"), Some(4.0));
        assert_eq!(order.issuer, boss);
    }
    assert!(app.world().get::<Orders>(bystander).is_none());
}

#[test]
fn a_ward_may_override_its_orders() {
    let mut app = test_app();
    let (boss, wards) = boss_with_wards(&mut app, &["a"]);
    order_wards(app.world_mut(), boss, "guard", ActionArgs::new());
    order_wards(app.world_mut(), boss, "fetch", ActionArgs::new());

    let mut ward = app.world_mut().entity_mut(wards[0]);
    let mut orders = ward.get_mut::<Orders>().unwrap();
    assert_eq!(orders.len(), 2);
    assert_eq!(orders.pop().unwrap().action, ActionName::new("guard"));
    orders.clear();

    assert!(app.world().get::<Orders>(wards[0]).unwrap().is_empty());
}

#[test]
fn orders_end_with_the_relation() {
    let mut app = test_app();
    let (boss, wards) = boss_with_wards(&mut app, &["a", "b"]);
    let other = named(&mut app, "other");
    order_wards(app.world_mut(), boss, "guard", ActionArgs::new());

    app.world_mut().entity_mut(wards[0]).insert(MasterIs(other));
    app.world_mut().entity_mut(wards[1]).remove::<MasterIs>();

    assert!(app.world().get::<Orders>(wards[0]).is_none());
    assert!(app.world().get::<Orders>(wards[1]).is_none());
}
