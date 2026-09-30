//! Orders from the master feed the tree, which may still override them.

mod common;

use bevy::prelude::*;
use common::*;
use struction_ai::*;
use struction_core::*;

#[derive(Component)]
struct Health {
    current: f32,
    max: f32,
}

fn ogre_app() -> App {
    let mut app = test_app();
    log_action(&mut app, "minions/ogre/flee");
    log_action(&mut app, "minions/ogre/idle");
    log_action(&mut app, "guard");
    app.register_condition(
        ConditionMeta::new("health/below").param("fraction", ParamType::Float),
        |In(call): In<ConditionCall>, health: Query<&Health>| {
            let fraction = call.args.float("fraction").unwrap() as f32;
            health
                .get(call.entity)
                .is_ok_and(|h| h.current < h.max * fraction)
        },
    );
    let below =
        NodeDef::Condition(LeafDef::new("health/below").arg("fraction", ArgDef::Float(0.25)));
    let has_order = NodeDef::Condition(LeafDef::new("orders/any"));
    app.add_behavior_tree(
        "ai/simple_ogre",
        &tree(sel(vec![
            seq(vec![below, act("minions/ogre/flee")]),
            seq(vec![has_order, NodeDef::ExecuteOrder]),
            act("minions/ogre/idle"),
        ])),
    );
    app
}

fn spawn_ogre(app: &mut App, boss: Entity, hp: f32) -> Entity {
    app.world_mut()
        .spawn((
            Brain::new("ai/simple_ogre"),
            Health {
                current: hp,
                max: 100.0,
            },
            MasterIs(boss),
        ))
        .id()
}

#[test]
fn a_survival_branch_overrides_the_masters_order() {
    let mut app = ogre_app();
    let boss = app.world_mut().spawn_empty().id();
    let healthy = spawn_ogre(&mut app, boss, 90.0);
    let hurt = spawn_ogre(&mut app, boss, 10.0);

    step(&mut app);
    assert_eq!(
        take_log(&mut app),
        [
            format!("minions/ogre/idle@{healthy}"),
            format!("minions/ogre/flee@{hurt}")
        ]
    );

    order_wards(app.world_mut(), boss, "guard", ActionArgs::new());
    step(&mut app);
    // The healthy ogre obeys; the hurt one flees and keeps the order for later.
    assert_eq!(
        take_log(&mut app),
        [
            format!("guard@{healthy}"),
            format!("minions/ogre/flee@{hurt}")
        ]
    );
    assert!(app.world().get::<Orders>(healthy).unwrap().is_empty());
    assert_eq!(app.world().get::<Orders>(hurt).unwrap().len(), 1);

    app.world_mut().get_mut::<Health>(hurt).unwrap().current = 80.0;
    step(&mut app);
    assert_eq!(
        take_log(&mut app),
        [
            format!("minions/ogre/idle@{healthy}"),
            format!("guard@{hurt}")
        ]
    );
    assert!(errors_are_empty(&mut app));
}

#[test]
fn orders_outside_the_action_set_are_refused() {
    let mut app = ogre_app();
    let boss = app.world_mut().spawn_empty().id();
    let ogre = spawn_ogre(&mut app, boss, 90.0);
    app.world_mut()
        .entity_mut(ogre)
        .insert(ActionSet::from_iter([ActionName::new("fetch")]));

    order_wards(app.world_mut(), boss, "guard", ActionArgs::new());
    step(&mut app);
    // The order is consumed and refused, so the selector falls through to idle.
    assert_eq!(take_log(&mut app), [format!("minions/ogre/idle@{ogre}")]);
    let errors = app.world_mut().resource_mut::<BrainErrors>().drain();
    assert_eq!(
        errors,
        [BrainError::OrderRefused {
            entity: ogre,
            action: "guard".into()
        }]
    );
}

#[test]
fn orders_current_matches_the_next_order() {
    let mut app = test_app();
    log_action(&mut app, "guard");
    log_action(&mut app, "ignore");
    let is_guard = NodeDef::Condition(
        LeafDef::new("orders/current").arg("action", ArgDef::Str("guard".into())),
    );
    app.add_behavior_tree(
        "t",
        &tree(sel(vec![
            seq(vec![is_guard, NodeDef::ExecuteOrder]),
            act("ignore"),
        ])),
    );
    let boss = app.world_mut().spawn_empty().id();
    let ward = app
        .world_mut()
        .spawn((Brain::new("t"), MasterIs(boss)))
        .id();
    order_wards(app.world_mut(), boss, "dance", ActionArgs::new());
    order_wards(app.world_mut(), boss, "guard", ActionArgs::new());
    step(&mut app);
    assert_eq!(take_log(&mut app), [format!("ignore@{ward}")]);
    app.world_mut().get_mut::<Orders>(ward).unwrap().pop();
    step(&mut app);
    assert_eq!(take_log(&mut app), [format!("guard@{ward}")]);
}

fn errors_are_empty(app: &mut App) -> bool {
    app.world().resource::<BrainErrors>().is_empty()
        && app
            .world_mut()
            .resource_mut::<ActionErrors>()
            .drain()
            .is_empty()
}
