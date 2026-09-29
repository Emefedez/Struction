use super::*;

const G: f32 = 9.81;

fn scene() -> GravityField {
    GravityField::scene(Vec3::NEG_Y * G)
}

fn at(field: &GravityField, translation: Vec3, rotation: Quat, point: Vec3) -> Vec3 {
    field_acceleration(field, translation, rotation, point)
}

#[test]
fn scene_gravity_is_the_same_everywhere() {
    let field = scene();
    for point in [Vec3::ZERO, Vec3::splat(1e6), Vec3::new(-3.0, 8.0, 2.0)] {
        assert_eq!(
            at(&field, Vec3::ZERO, Quat::IDENTITY, point),
            Vec3::NEG_Y * G
        );
    }
}

#[test]
fn directional_field_rotates_with_its_transform() {
    let field = scene();
    let rotation = Quat::from_rotation_z(core::f32::consts::FRAC_PI_2);
    let a = at(&field, Vec3::ZERO, rotation, Vec3::ZERO);
    assert!(a.abs_diff_eq(Vec3::X * G, 1e-5), "{a}");
}

#[test]
fn radial_field_points_at_the_center_inside_the_sphere() {
    let field = GravityField::planet(G, 10.0);
    let center = Vec3::new(5.0, 0.0, 0.0);
    let a = at(
        &field,
        center,
        Quat::IDENTITY,
        center + Vec3::new(0.0, 4.0, 0.0),
    );
    assert!(a.abs_diff_eq(Vec3::NEG_Y * G, 1e-5));
    let a = at(
        &field,
        center,
        Quat::IDENTITY,
        center + Vec3::new(3.0, 4.0, 0.0),
    );
    assert!((a.length() - G).abs() < 1e-5);
    assert!(
        a.normalize()
            .abs_diff_eq(-Vec3::new(3.0, 4.0, 0.0) / 5.0, 1e-5)
    );
}

#[test]
fn radial_field_is_zero_outside_the_volume_and_at_the_center() {
    let field = GravityField::planet(G, 10.0);
    assert_eq!(
        at(
            &field,
            Vec3::ZERO,
            Quat::IDENTITY,
            Vec3::new(10.5, 0.0, 0.0)
        ),
        Vec3::ZERO
    );
    assert_eq!(
        at(&field, Vec3::ZERO, Quat::IDENTITY, Vec3::ZERO),
        Vec3::ZERO
    );
}

#[test]
fn negative_radial_strength_repels() {
    let field = GravityField {
        volume: GravityVolume::Sphere { radius: 10.0 },
        kind: GravityKind::Radial {
            strength: -2.0,
            falloff: Falloff::Constant,
        },
    };
    let a = at(&field, Vec3::ZERO, Quat::IDENTITY, Vec3::X * 3.0);
    assert!(a.abs_diff_eq(Vec3::X * 2.0, 1e-6));
}

#[test]
fn inverse_square_falls_off_beyond_the_reference_distance() {
    let field = GravityField {
        volume: GravityVolume::Infinite,
        kind: GravityKind::Radial {
            strength: 8.0,
            falloff: Falloff::InverseSquare {
                reference_distance: 2.0,
            },
        },
    };
    let magnitude = |d: f32| at(&field, Vec3::ZERO, Quat::IDENTITY, Vec3::X * d).length();
    assert!((magnitude(1.0) - 8.0).abs() < 1e-5, "full strength inside");
    assert!((magnitude(2.0) - 8.0).abs() < 1e-5);
    assert!((magnitude(4.0) - 2.0).abs() < 1e-5);
    assert!((magnitude(20.0) - 0.08).abs() < 1e-5);
}

#[test]
fn box_volume_is_oriented_with_the_field() {
    let field = GravityField {
        volume: GravityVolume::Box {
            half_extents: Vec3::new(5.0, 1.0, 1.0),
        },
        kind: GravityKind::Directional {
            acceleration: Vec3::NEG_Y,
        },
    };
    let inside = Vec3::new(4.0, 0.0, 0.0);
    assert_ne!(at(&field, Vec3::ZERO, Quat::IDENTITY, inside), Vec3::ZERO);
    // Rotated a quarter turn about Z, the long axis now runs along world Y.
    let rotation = Quat::from_rotation_z(core::f32::consts::FRAC_PI_2);
    assert_eq!(at(&field, Vec3::ZERO, rotation, inside), Vec3::ZERO);
    assert_ne!(
        at(&field, Vec3::ZERO, rotation, Vec3::new(0.0, 4.0, 0.0)),
        Vec3::ZERO
    );
}

#[test]
fn overlapping_fields_sum_without_priority() {
    let scene = scene();
    let planet = GravityField::planet(G, 10.0);
    // Standing on top of a planet at the origin while scene gravity still points down.
    let point = Vec3::new(0.0, 5.0, 0.0);
    let sum = sum_fields(
        [
            (&scene, Vec3::ZERO, Quat::IDENTITY),
            (&planet, Vec3::ZERO, Quat::IDENTITY),
        ],
        point,
    );
    assert!(sum.abs_diff_eq(Vec3::NEG_Y * 2.0 * G, 1e-5));
    // Opposing fields cancel.
    let sum = sum_fields(
        [
            (&scene, Vec3::ZERO, Quat::IDENTITY),
            (
                &GravityField::scene(Vec3::Y * G),
                Vec3::ZERO,
                Quat::IDENTITY,
            ),
        ],
        point,
    );
    assert!(sum.length() < 1e-5);
}

#[test]
fn up_is_opposite_gravity_with_a_fallback_when_zero() {
    assert_eq!(up_from_gravity(Vec3::NEG_Y * G, Dir3::X), Dir3::Y);
    let up = up_from_gravity(Vec3::new(3.0, 0.0, 4.0) * -1.0, Dir3::Y);
    assert!(up.abs_diff_eq(Vec3::new(0.6, 0.0, 0.8), 1e-6));
    assert_eq!(up_from_gravity(Vec3::ZERO, Dir3::Z), Dir3::Z);
    assert_eq!(up_from_gravity(Vec3::splat(1e-6), Dir3::Z), Dir3::Z);
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, GravityPlugin::new(Update)));
    app
}

#[test]
fn system_writes_local_gravity_and_up_from_all_fields() {
    let mut app = app();
    app.world_mut().spawn(scene());
    app.world_mut().spawn((
        GravityField::planet(G, 10.0),
        Transform::from_xyz(100.0, 0.0, 0.0),
    ));
    let near_planet = app
        .world_mut()
        .spawn((
            LocalGravity::default(),
            Transform::from_xyz(100.0, 6.0, 0.0),
        ))
        .id();
    let far = app
        .world_mut()
        .spawn((LocalGravity::default(), Transform::from_xyz(0.0, 50.0, 0.0)))
        .id();
    // The first update propagates transforms, the second reads them.
    app.update();
    app.update();

    let gravity = app.world().get::<LocalGravity>(far).unwrap();
    assert_eq!(gravity.0, Vec3::NEG_Y * G);
    assert_eq!(app.world().get::<LocalUp>(far).unwrap().0, Dir3::Y);

    let gravity = app.world().get::<LocalGravity>(near_planet).unwrap();
    assert!(gravity.0.abs_diff_eq(Vec3::NEG_Y * 2.0 * G, 1e-5));
}

#[test]
fn up_persists_when_fields_cancel() {
    let mut app = app();
    app.world_mut().spawn(GravityField::scene(Vec3::X * -G));
    let body = app
        .world_mut()
        .spawn((LocalGravity::default(), Transform::default()))
        .id();
    app.update();
    app.update();
    assert_eq!(app.world().get::<LocalUp>(body).unwrap().0, Dir3::X);

    app.world_mut().spawn(GravityField::scene(Vec3::X * G));
    app.update();
    app.update();
    assert_eq!(app.world().get::<LocalGravity>(body).unwrap().0, Vec3::ZERO);
    assert_eq!(app.world().get::<LocalUp>(body).unwrap().0, Dir3::X);
}
