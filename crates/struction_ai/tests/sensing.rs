mod common;

use bevy::prelude::*;
use common::*;
use struction_ai::*;

/// Blocks sightlines crossing it, as a physics raycast would.
#[derive(Component)]
struct Wall;

fn small_ogre_sensing() -> Sensing {
    Sensing {
        sees: vec!["player".into()],
        flocks_with: vec!["minions/ogre".into()],
        range: 10.0,
        field_of_view: 120.0,
    }
}

fn spawn(app: &mut App, path: &str, lineage: &[&str], at: Vec3) -> Entity {
    app.world_mut()
        .spawn((actor(path, lineage), Transform::from_translation(at)))
        .id()
}

#[test]
fn sees_by_lineage_range_and_field_of_view() {
    let mut app = test_app();
    // Facing -Z from the origin.
    let ogre = app
        .world_mut()
        .spawn((
            actor("minions/small_ogre", &["minions/ogre", "Actor"]),
            small_ogre_sensing(),
            Transform::default(),
        ))
        .id();
    let player = spawn(&mut app, "player", &["Actor"], Vec3::new(0.0, 0.0, -5.0));
    let knight = spawn(
        &mut app,
        "heroes/knight",
        &["player", "Actor"],
        Vec3::new(1.0, 0.0, -2.0),
    );
    let _goblin = spawn(
        &mut app,
        "minions/goblin",
        &["Actor"],
        Vec3::new(0.0, 0.0, -1.0),
    );
    let behind = spawn(&mut app, "player", &["Actor"], Vec3::new(0.0, 0.0, 5.0));
    let _far = spawn(&mut app, "player", &["Actor"], Vec3::new(0.0, 0.0, -50.0));
    let buddy = spawn(
        &mut app,
        "minions/ogre",
        &["Actor"],
        Vec3::new(0.0, 0.0, 3.0),
    );
    let big = spawn(
        &mut app,
        "minions/big_ogre",
        &["minions/ogre", "Actor"],
        Vec3::new(-2.0, 0.0, 0.0),
    );

    step(&mut app);
    let sensed = app.world().get::<Sensed>(ogre).unwrap();
    assert_eq!(sensed.visible, [knight, player]);
    // Flockmates are felt all around, nearest first; the ogre is not its own mate.
    assert_eq!(sensed.flockmates, [big, buddy]);

    // Sensing is rebuilt every tick: turned around, only the player behind is in view.
    app.world_mut()
        .get_mut::<Transform>(ogre)
        .unwrap()
        .rotate_y(std::f32::consts::PI);
    step(&mut app);
    assert_eq!(app.world().get::<Sensed>(ogre).unwrap().visible, [behind]);
}

#[test]
fn line_of_sight_is_pluggable() {
    let mut app = test_app();
    // A wall along x = 1 between the ogre and anything to its right.
    app.set_line_of_sight(
        |In(lines): In<Vec<Sightline>>, walls: Query<&Transform, With<Wall>>| {
            lines
                .iter()
                .map(|line| {
                    walls.iter().all(|wall| {
                        let x = wall.translation.x;
                        (line.from.x - x) * (line.to.x - x) >= 0.0
                    })
                })
                .collect::<Vec<bool>>()
        },
    );
    app.world_mut()
        .spawn((Wall, Transform::from_xyz(1.0, 0.0, 0.0)));
    let ogre = app
        .world_mut()
        .spawn((
            actor("minions/small_ogre", &["minions/ogre", "Actor"]),
            Sensing {
                field_of_view: 360.0,
                ..small_ogre_sensing()
            },
            Transform::default(),
        ))
        .id();
    let hidden = spawn(&mut app, "player", &["Actor"], Vec3::new(3.0, 0.0, -1.0));
    let seen = spawn(&mut app, "player", &["Actor"], Vec3::new(-3.0, 0.0, -1.0));
    let mate = spawn(
        &mut app,
        "minions/ogre",
        &["Actor"],
        Vec3::new(3.0, 0.0, 0.0),
    );

    step(&mut app);
    let sensed = app.world().get::<Sensed>(ogre).unwrap();
    assert_eq!(sensed.visible, [seen]);
    assert!(!sensed.visible.contains(&hidden));
    assert_eq!(sensed.flockmates, [mate]);
}

#[test]
fn trees_react_to_what_is_seen_in_the_same_tick() {
    let mut app = test_app();
    log_action(&mut app, "chase");
    let sees_player = NodeDef::Condition(
        LeafDef::new("sensing/sees").arg("definition", ArgDef::Str("player".into())),
    );
    app.add_behavior_tree(
        "ai/simple_ogre",
        &tree(seq(vec![sees_player, act("chase")])),
    );
    let ogre = app
        .world_mut()
        .spawn((
            actor("minions/small_ogre", &["minions/ogre", "Actor"]),
            small_ogre_sensing(),
            Brain::new("ai/simple_ogre"),
            Transform::default(),
        ))
        .id();
    step(&mut app);
    assert!(take_log(&mut app).is_empty());

    spawn(
        &mut app,
        "heroes/knight",
        &["player", "Actor"],
        Vec3::new(0.0, 0.0, -4.0),
    );
    step(&mut app);
    assert_eq!(take_log(&mut app), [format!("chase@{ogre}")]);
}

#[test]
fn sensing_reads_its_data_section() {
    let sensing: Sensing = serde_json::from_str(
        r#"{ "sees": ["player"], "flocksWith": ["minions/ogre"], "range": 12 }"#,
    )
    .unwrap();
    assert_eq!(sensing.sees, ["player"]);
    assert_eq!(sensing.flocks_with, ["minions/ogre"]);
    assert_eq!(sensing.range, 12.0);
    assert_eq!(sensing.field_of_view, Sensing::default().field_of_view);
}
