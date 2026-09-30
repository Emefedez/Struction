use bevy::prelude::*;
use struction_physics::{avian3d::prelude::*, camera_obstructions, testing::*};

#[derive(Resource)]
struct Probe {
    focus: Vec3,
    camera: Vec3,
    exclude: Entity,
    hits: Vec<Entity>,
}
fn query(spatial: SpatialQuery, mut probe: ResMut<Probe>) {
    probe.hits = camera_obstructions(
        &spatial,
        probe.focus,
        probe.camera,
        0.25,
        &SpatialQueryFilter::default().with_excluded_entities([probe.exclude]),
    );
}

#[test]
fn query_finds_multiple_blockers_and_a_camera_inside_the_planet() {
    let mut app = headless_app();
    let player = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Collider::sphere(0.3),
            Transform::from_xyz(0.0, 0.0, 6.0),
        ))
        .id();
    let planet = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Collider::sphere(4.0),
            Transform::default(),
        ))
        .id();
    let wall = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Collider::cuboid(10.0, 10.0, 0.2),
            Transform::from_xyz(0.0, 0.0, -5.0),
        ))
        .id();
    let behind_camera = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Collider::sphere(1.0),
            Transform::from_xyz(0.0, 0.0, -20.0),
        ))
        .id();
    app.insert_resource(Probe {
        focus: Vec3::Z * 6.0,
        camera: Vec3::NEG_Z * 10.0,
        exclude: player,
        hits: vec![],
    })
    .add_systems(Update, query);
    step(&mut app, 3);
    let hits = &app.world().resource::<Probe>().hits;
    assert!(hits.contains(&planet));
    assert!(hits.contains(&wall));
    assert!(!hits.contains(&player));
    assert!(!hits.contains(&behind_camera));
    app.world_mut().resource_mut::<Probe>().camera = Vec3::ZERO;
    step(&mut app, 1);
    assert_eq!(app.world().resource::<Probe>().hits, vec![planet]);
    app.world_mut().resource_mut::<Probe>().camera = Vec3::Z * 10.0;
    step(&mut app, 1);
    assert!(app.world().resource::<Probe>().hits.is_empty());
}
