use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use serde_json::json;
use struction_core::StableIdGenerator;
use struction_debug::*;

#[derive(Component, Reflect, Clone, PartialEq, Debug)]
struct SimPosition(Vec3);

#[derive(Component, Reflect)]
struct Status {
    health: f32,
    swimming: bool,
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(DebugTracePlugin)
        .trace_component::<SimPosition>()
        .trace_component::<Status>();
    app
}

fn tick(app: &mut App) -> Vec<TraceEvent> {
    app.world_mut().run_schedule(FixedLast);
    app.world_mut().resource_mut::<TraceLog>().drain().collect()
}

#[test]
fn reports_only_changes_with_tick_identity_and_before_after_values() {
    let mut app = app();
    let id = StableIdGenerator::new(17).next_id();
    let entity = app
        .world_mut()
        .spawn((
            id,
            Name::new("guard"),
            SimPosition(Vec3::ZERO),
            Status {
                health: 100.0,
                swimming: false,
            },
        ))
        .id();
    let initial = tick(&mut app);
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].tick, 1);
    assert_eq!(
        initial[0].identity.stable_id.as_deref(),
        Some(id.to_string().as_str())
    );
    assert_eq!(initial[0].identity.name.as_deref(), Some("guard"));
    assert!(matches!(initial[0].change, TraceChange::Spawned { .. }));
    assert!(tick(&mut app).is_empty());
    app.world_mut().get_mut::<SimPosition>(entity).unwrap().0 = Vec3::X;
    app.world_mut().get_mut::<Status>(entity).unwrap().swimming = true;
    let events = tick(&mut app);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].tick, 3);
    let TraceChange::Changed { components, active } = &events[0].change else {
        panic!("expected changes")
    };
    assert_eq!(*active, None);
    assert_eq!(
        components[Status::type_path()].before,
        Some(json!({"health":100.0,"swimming":false}))
    );
    assert_eq!(
        components[Status::type_path()].after,
        Some(json!({"health":100.0,"swimming":true}))
    );
    assert!(components.contains_key(SimPosition::type_path()));
    app.world_mut().entity_mut(entity).remove::<Status>();
    let events = tick(&mut app);
    let TraceChange::Changed { components, .. } = &events[0].change else {
        panic!("expected removal")
    };
    assert_eq!(components[Status::type_path()].after, None);
    app.world_mut().entity_mut(entity).insert(Status {
        health: 25.0,
        swimming: false,
    });
    let events = tick(&mut app);
    let TraceChange::Changed { components, .. } = &events[0].change else {
        panic!("expected addition")
    };
    assert_eq!(components[Status::type_path()].before, None);
    for event in initial.into_iter().chain(events) {
        assert_eq!(
            serde_json::from_str::<TraceEvent>(&serde_json::to_string(&event).unwrap()).unwrap(),
            event
        );
    }
}

#[test]
fn unloading_untracking_and_despawning_are_distinct() {
    let mut app = app();
    let entity = app
        .world_mut()
        .spawn((TraceEntity, SimPosition(Vec3::ZERO)))
        .id();
    tick(&mut app);
    app.world_mut().entity_mut(entity).insert(Disabled);
    let events = tick(&mut app);
    assert!(matches!(
        events[0].change,
        TraceChange::Changed {
            active: Some(false),
            ..
        }
    ));
    app.world_mut().entity_mut(entity).remove::<Disabled>();
    assert!(matches!(
        tick(&mut app)[0].change,
        TraceChange::Changed {
            active: Some(true),
            ..
        }
    ));
    app.world_mut().entity_mut(entity).remove::<TraceEntity>();
    assert_eq!(tick(&mut app)[0].change, TraceChange::Untracked);
    app.world_mut().entity_mut(entity).insert(TraceEntity);
    assert!(matches!(
        tick(&mut app)[0].change,
        TraceChange::Spawned { .. }
    ));
    app.world_mut().entity_mut(entity).despawn();
    assert_eq!(tick(&mut app)[0].change, TraceChange::Removed);
}

#[test]
fn tracing_is_opt_in_bounded_and_does_not_mutate_components() {
    let mut app = app();
    let ignored = app.world_mut().spawn(SimPosition(Vec3::ZERO)).id();
    assert!(tick(&mut app).is_empty());
    app.world_mut().entity_mut(ignored).insert(TraceEntity);
    app.world_mut().resource_mut::<TraceSettings>().enabled = false;
    assert!(tick(&mut app).is_empty());
    app.world_mut().resource_mut::<TraceSettings>().enabled = true;
    app.world_mut().resource_mut::<TraceSettings>().capacity = 2;
    for x in 0..5 {
        app.world_mut().get_mut::<SimPosition>(ignored).unwrap().0.x = x as f32;
        app.world_mut().run_schedule(FixedLast);
    }
    let log = app.world().resource::<TraceLog>();
    assert_eq!(log.iter().count(), 2);
    assert_eq!(log.iter().last().unwrap().tick, 7);
    assert_eq!(
        app.world().get::<SimPosition>(ignored).unwrap(),
        &SimPosition(Vec3::X * 4.0)
    );
}

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);
impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn streams_every_event_as_json_lines_even_with_a_zero_capacity_log() {
    let buffer = Buffer::default();
    let mut app = app();
    app.insert_resource(TraceWriter::new(buffer.clone()));
    app.world_mut().resource_mut::<TraceSettings>().capacity = 0;
    let entity = app
        .world_mut()
        .spawn((TraceEntity, SimPosition(Vec3::ZERO)))
        .id();
    assert!(tick(&mut app).is_empty());
    app.world_mut().entity_mut(entity).despawn();
    tick(&mut app);
    let bytes = buffer.0.lock().unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    let events: Vec<TraceEvent> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 2);
    assert!(matches!(events[0].change, TraceChange::Spawned { .. }));
    assert_eq!(events[1].change, TraceChange::Removed);
    assert!(text.ends_with('\n'));
}

struct Broken;
impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("disk full"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn output_failure_is_reported_while_memory_capture_continues() {
    let mut app = app();
    app.insert_resource(TraceWriter::new(Broken));
    let entity = app
        .world_mut()
        .spawn((TraceEntity, SimPosition(Vec3::ZERO)))
        .id();
    assert_eq!(tick(&mut app).len(), 1);
    assert!(
        app.world()
            .resource::<TraceWriter>()
            .error()
            .unwrap()
            .contains("disk full")
    );
    app.world_mut().entity_mut(entity).despawn();
    assert_eq!(tick(&mut app)[0].change, TraceChange::Removed);
}
