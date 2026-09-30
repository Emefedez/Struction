use bevy::prelude::*;
use struction_character::{
    CharacterController, CharacterControllerPlugin, CharacterIntent, CharacterState,
};
use struction_gravity::{GravityField, GravityHysteresis, GravityInfluences, LocalGravity};
use struction_physics::{Surface, avian3d::prelude::*, testing::*};

#[test]
fn a_normal_jump_crosses_the_planets_exit_boundary() {
    for radial in [Vec3::Y, Vec3::Z, Vec3::X] {
        let mut app = headless_app_with(CharacterControllerPlugin);
        let center = Vec3::ZERO;
        app.world_mut()
            .spawn(GravityField::scene(Vec3::NEG_Y * 9.81));
        let planet = app
            .world_mut()
            .spawn((
                RigidBody::Static,
                Collider::sphere(4.0),
                GravityField::planet(24.0, 5.0),
                GravityHysteresis { exit_margin: 0.5 },
                Surface::default(),
                Transform::from_translation(center),
            ))
            .id();
        let up = (radial * 24.0 + Vec3::Y * 9.81).normalize();
        let body = app
            .world_mut()
            .spawn((
                CharacterController::default(),
                Transform::from_translation(center + radial * 4.8)
                    .with_rotation(Quat::from_rotation_arc(Vec3::Y, up)),
            ))
            .id();
        step(&mut app, 60);
        assert!(
            app.world().get::<CharacterState>(body).unwrap().grounded,
            "radial {radial}"
        );
        assert!(
            app.world()
                .get::<GravityInfluences>(body)
                .unwrap()
                .0
                .contains(&planet)
        );
        app.world_mut()
            .get_mut::<CharacterIntent>(body)
            .unwrap()
            .jump_requested = true;
        let mut escaped = false;
        for _ in 0..100 {
            step(&mut app, 1);
            if !app
                .world()
                .get::<GravityInfluences>(body)
                .unwrap()
                .0
                .contains(&planet)
            {
                let distance = app
                    .world()
                    .get::<Position>(body)
                    .unwrap()
                    .0
                    .distance(center);
                assert!(distance > 5.5, "released too early at {distance}");
                assert!(
                    app.world()
                        .get::<LocalGravity>(body)
                        .unwrap()
                        .0
                        .abs_diff_eq(Vec3::NEG_Y * 9.81, 1e-4)
                );
                escaped = true;
                break;
            }
        }
        assert!(
            escaped,
            "jump must leave the planet from {radial}; position {:?}",
            app.world().get::<Position>(body)
        );
    }
}

#[test]
fn walking_around_the_planet_under_scene_gravity_stays_grounded() {
    let mut app = headless_app_with(CharacterControllerPlugin);
    app.world_mut()
        .spawn(GravityField::scene(Vec3::NEG_Y * 9.81));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::sphere(4.0),
        GravityField::planet(24.0, 5.0),
        GravityHysteresis { exit_margin: 0.5 },
        Surface::default(),
        Transform::default(),
    ));
    // On the equator the scene field tilts local up about 22 degrees from the surface normal.
    let up = (Vec3::X * 24.0 + Vec3::Y * 9.81).normalize();
    let body = app
        .world_mut()
        .spawn((
            CharacterController::default(),
            Transform::from_translation(Vec3::X * 4.8)
                .with_rotation(Quat::from_rotation_arc(Vec3::Y, up)),
        ))
        .id();
    step(&mut app, 60);
    // Toward the pole, along the tilt: the body moves along up without leaving the surface.
    let mut intent = app.world_mut().get_mut::<CharacterIntent>(body).unwrap();
    intent.movement = Vec2::Y;
    intent.movement_forward = Some(Vec3::Y);
    for tick in 0..60 {
        step(&mut app, 1);
        assert!(
            app.world().get::<CharacterState>(body).unwrap().grounded,
            "airborne at tick {tick}"
        );
    }
    let position = app.world().get::<Position>(body).unwrap().0;
    assert!(
        (position.length() - 4.8).abs() < 0.1,
        "left the surface: {position}"
    );
    assert!(position.y > 2.5, "walked toward the pole: {position}");
}
