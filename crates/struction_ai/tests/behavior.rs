mod common;

use bevy::prelude::*;
use common::*;
use struction_ai::*;

fn spawn_brain(app: &mut App, tree: &str) -> Entity {
    app.world_mut()
        .spawn((Brain::new(tree), Transform::default()))
        .id()
}

#[test]
fn sequence_resumes_at_the_running_child() {
    let mut app = test_app();
    log_action(&mut app, "greet");
    log_action(&mut app, "sit");
    app.add_behavior_tree(
        "ai/walker",
        &tree(seq(vec![act("greet"), walk_to(0.0, -3.0), act("sit")])),
    );
    let e = spawn_brain(&mut app, "ai/walker");

    step(&mut app);
    assert_eq!(status(&app, e), Some(Status::Running));
    assert_eq!(
        take_log(&mut app),
        [format!("greet@{e}"), format!("move_to@{e}")]
    );

    // Walking 3 m at 1 m per tick: running, and `greet` is not repeated while resuming.
    steps(&mut app, 3);
    assert_eq!(status(&app, e), Some(Status::Running));
    assert!(take_log(&mut app).is_empty());

    step(&mut app);
    assert_eq!(status(&app, e), Some(Status::Success));
    assert_eq!(take_log(&mut app), [format!("sit@{e}")]);
    let position = app.world().get::<Transform>(e).unwrap().translation;
    assert_eq!(position, Vec3::new(0.0, 0.0, -3.0));

    // Finished: the next tick starts over.
    step(&mut app);
    assert_eq!(
        take_log(&mut app),
        [format!("greet@{e}"), format!("move_to@{e}")]
    );
}

#[test]
fn sequence_fails_at_the_first_failure() {
    let mut app = test_app();
    log_action(&mut app, "a");
    log_action(&mut app, "b");
    app.add_behavior_tree("t", &tree(seq(vec![act("a"), flag("go"), act("b")])));
    let e = spawn_brain(&mut app, "t");

    step(&mut app);
    assert_eq!(status(&app, e), Some(Status::Failure));
    assert_eq!(take_log(&mut app), [format!("a@{e}")]);

    set_flag(&mut app, e, "go");
    step(&mut app);
    assert_eq!(status(&app, e), Some(Status::Success));
    assert_eq!(take_log(&mut app), [format!("a@{e}"), format!("b@{e}")]);
}

#[test]
fn selector_takes_the_first_child_that_does_not_fail() {
    let mut app = test_app();
    log_action(&mut app, "a");
    log_action(&mut app, "b");
    app.add_behavior_tree(
        "t",
        &tree(sel(vec![seq(vec![flag("x"), act("a")]), act("b")])),
    );
    let e = spawn_brain(&mut app, "t");

    step(&mut app);
    assert_eq!(take_log(&mut app), [format!("b@{e}")]);
    set_flag(&mut app, e, "x");
    step(&mut app);
    assert_eq!(status(&app, e), Some(Status::Success));
    assert_eq!(take_log(&mut app), [format!("a@{e}")]);

    app.add_behavior_tree("none", &tree(sel(vec![flag("y"), flag("z")])));
    let f = spawn_brain(&mut app, "none");
    step(&mut app);
    assert_eq!(status(&app, f), Some(Status::Failure));
}

#[test]
fn higher_priority_branch_interrupts_and_cancels_a_running_task() {
    let mut app = test_app();
    log_action(&mut app, "shout");
    app.add_behavior_tree(
        "t",
        &tree(sel(vec![
            seq(vec![flag("alarm"), act("shout")]),
            walk_to(0.0, -10.0),
        ])),
    );
    let e = spawn_brain(&mut app, "t");

    steps(&mut app, 3);
    assert_eq!(status(&app, e), Some(Status::Running));
    assert_eq!(take_log(&mut app), [format!("move_to@{e}")]);
    assert!(app.world().get::<common::MoveTo>(e).is_some());

    set_flag(&mut app, e, "alarm");
    step(&mut app);
    assert_eq!(status(&app, e), Some(Status::Success));
    assert_eq!(
        take_log(&mut app),
        [format!("shout@{e}"), format!("stop@{e}")]
    );
    assert!(app.world().get::<common::MoveTo>(e).is_none());

    // The interrupted task starts over once the alarm is gone.
    clear_flag(&mut app, e, "alarm");
    step(&mut app);
    assert_eq!(take_log(&mut app), [format!("move_to@{e}")]);
}

#[test]
fn inverter_swaps_success_and_failure() {
    let mut app = test_app();
    app.add_behavior_tree(
        "t",
        &tree(NodeDef::Inverter {
            children: vec![flag("x")],
        }),
    );
    let e = spawn_brain(&mut app, "t");
    step(&mut app);
    assert_eq!(status(&app, e), Some(Status::Success));
    set_flag(&mut app, e, "x");
    step(&mut app);
    assert_eq!(status(&app, e), Some(Status::Failure));
}

#[test]
fn parallel_guard_rechecks_its_condition_and_cancels_on_failure() {
    let mut app = test_app();
    app.add_behavior_tree(
        "t",
        &tree(NodeDef::Parallel {
            policy: ParallelPolicy::RequireAll,
            children: vec![flag("allowed"), walk_to(0.0, -2.0)],
        }),
    );
    let guarded = spawn_brain(&mut app, "t");
    set_flag(&mut app, guarded, "allowed");
    let other = spawn_brain(&mut app, "t");
    set_flag(&mut app, other, "allowed");

    step(&mut app);
    assert_eq!(status(&app, guarded), Some(Status::Running));
    take_log(&mut app);

    // Revoking the guard interrupts the walk.
    clear_flag(&mut app, other, "allowed");
    step(&mut app);
    assert_eq!(status(&app, other), Some(Status::Failure));
    assert_eq!(take_log(&mut app), [format!("stop@{other}")]);

    steps(&mut app, 2);
    assert_eq!(status(&app, guarded), Some(Status::Success));
}

#[test]
fn parallel_require_one_succeeds_with_the_first_success() {
    let mut app = test_app();
    app.add_behavior_tree(
        "t",
        &tree(NodeDef::Parallel {
            policy: ParallelPolicy::RequireOne,
            children: vec![flag("bored"), walk_to(0.0, -10.0)],
        }),
    );
    let e = spawn_brain(&mut app, "t");
    steps(&mut app, 2);
    assert_eq!(status(&app, e), Some(Status::Running));
    set_flag(&mut app, e, "bored");
    take_log(&mut app);
    step(&mut app);
    assert_eq!(status(&app, e), Some(Status::Success));
    assert_eq!(take_log(&mut app), [format!("stop@{e}")]);
}

#[test]
fn a_shared_tree_keeps_independent_state_per_entity() {
    let mut app = test_app();
    log_action(&mut app, "arrive");
    app.add_behavior_tree(
        "ai/simple_ogre",
        &tree(seq(vec![walk_to(0.0, -3.0), act("arrive")])),
    );
    let near = app
        .world_mut()
        .spawn((
            Brain::new("ai/simple_ogre"),
            Transform::from_xyz(0.0, 0.0, -2.0),
        ))
        .id();
    let far = app
        .world_mut()
        .spawn((
            Brain::new("ai/simple_ogre"),
            Transform::from_xyz(0.0, 0.0, 10.0),
        ))
        .id();
    assert_eq!(app.world().resource::<BehaviorTrees>().len(), 1);

    let mut arrivals = Vec::new();
    for tick in 1..=20 {
        step(&mut app);
        for line in take_log(&mut app) {
            if line.starts_with("arrive") {
                arrivals.push((tick, line));
            }
        }
    }
    // Each ogre finishes on its own schedule and then starts again.
    assert_eq!(arrivals[0], (3, format!("arrive@{near}")));
    assert!(arrivals.contains(&(15, format!("arrive@{far}"))));
    assert!(
        !arrivals
            .iter()
            .any(|(t, l)| *t < 15 && *l == format!("arrive@{far}"))
    );
}

#[test]
fn hot_reload_replaces_the_tree_and_resets_state() {
    let mut app = test_app();
    log_action(&mut app, "a");
    log_action(&mut app, "b");
    app.add_behavior_tree("t", &tree(seq(vec![act("a"), walk_to(0.0, -9.0)])));
    let e = spawn_brain(&mut app, "t");
    step(&mut app);
    take_log(&mut app);

    app.add_behavior_tree("t", &tree(act("b")));
    step(&mut app);
    assert_eq!(status(&app, e), Some(Status::Success));
    assert_eq!(take_log(&mut app), [format!("b@{e}")]);
}

#[test]
fn missing_references_name_what_is_missing() {
    let mut app = test_app();
    let def = tree(sel(vec![
        NodeDef::Condition(LeafDef::new("sensing/smells")),
        act("minions/ogre/roar"),
        NodeDef::Task(TaskDef {
            start: LeafDef::new("move_to").arg("x", ArgDef::Str("far".into())),
            until: LeafDef::new("move_to/arrived"),
            cancel: None,
        }),
        NodeDef::Inverter { children: vec![] },
    ]));
    let errors = BehaviorTrees::register(app.world_mut(), "ai/broken", &def).unwrap_err();
    let messages: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(messages.len(), 4, "{messages:#?}");
    assert!(messages[0].contains("unknown condition `sensing/smells`"));
    assert!(messages[0].contains("ai/broken"));
    assert!(messages[1].contains("unknown action `minions/ogre/roar`"));
    assert!(messages[2].contains("root:Selector/2:Task.start"));
    assert!(messages[2].contains("parameter `x`"));
    assert!(messages[3].contains("exactly one child"));
    assert!(
        app.world()
            .resource::<BehaviorTrees>()
            .resolve("ai/broken")
            .is_err()
    );
}

#[test]
#[should_panic(expected = "unknown action `nope`")]
fn adding_an_invalid_tree_to_the_app_panics() {
    let mut app = test_app();
    app.add_behavior_tree("t", &tree(act("nope")));
}

#[test]
fn a_brain_naming_an_unregistered_tree_is_reported_once() {
    let mut app = test_app();
    let e = spawn_brain(&mut app, "ai/ghost");
    steps(&mut app, 3);
    let errors = app.world_mut().resource_mut::<BrainErrors>().drain();
    assert_eq!(
        errors,
        [BrainError::MissingTree {
            entity: e,
            name: "ai/ghost".into()
        }]
    );
    assert_eq!(status(&app, e), None);
}

#[test]
fn the_documented_format_loads_through_serde() {
    let json = r#"{
      "root": { "Selector": { "children": [
        { "Sequence": { "children": [
          { "Condition": { "name": "flag", "args": { "name": { "Str": "alarm" } } } },
          { "Action": { "name": "move_to/stop" } }
        ] } },
        { "Sequence": { "children": [ { "Condition": { "name": "orders/any" } }, "ExecuteOrder" ] } },
        { "Parallel": { "policy": "RequireAll", "children": [
          { "Inverter": { "children": [ { "Condition": { "name": "flag", "args": { "name": { "Str": "tired" } } } } ] } },
          { "Task": {
            "start": { "name": "move_to", "args": { "x": { "Float": 1.5 }, "z": { "Int": 2 } } },
            "until": { "name": "move_to/arrived" },
            "cancel": { "name": "move_to/stop" }
          } }
        ] } }
      ] } }
    }"#;
    let def: BehaviorTreeDef = serde_json::from_str(json).unwrap();
    let back: BehaviorTreeDef =
        serde_json::from_str(&serde_json::to_string(&def).unwrap()).unwrap();
    assert_eq!(def, back);

    let mut app = test_app();
    BehaviorTrees::register(app.world_mut(), "ai/doc", &def).unwrap();
    assert_eq!(
        app.world()
            .resource::<BehaviorTrees>()
            .resolve("ai/doc")
            .unwrap()
            .len(),
        11
    );
}

#[test]
fn tree_definitions_are_registered_for_reflection() {
    let app = test_app();
    let registry = app.world().resource::<AppTypeRegistry>().read();
    for ty in [
        std::any::TypeId::of::<BehaviorTreeDef>(),
        std::any::TypeId::of::<NodeDef>(),
        std::any::TypeId::of::<LeafDef>(),
        std::any::TypeId::of::<ArgDef>(),
    ] {
        assert!(registry.get(ty).is_some(), "{ty:?}");
    }
}
