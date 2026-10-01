//! A definition's `states` section for character states: while `Rolling`, `Swimming` and the
//! rest hold, the components it lists are enabled or disabled, and leaving restores what they
//! replaced. Applied between the moves and the controller, so a state can retune either.

use std::any::TypeId;
use std::sync::Arc;

use bevy::ecs::reflect::ReflectComponent;
use bevy::prelude::*;
use struction_core::{StateRule, StateRules};

use crate::{CharacterCondition, CharacterMove, CharacterState};

impl CharacterCondition {
    pub fn named(name: &str) -> Option<Self> {
        Some(match name {
            "Grounded" => Self::Grounded,
            "Airborne" => Self::Airborne,
            "Swimming" => Self::Swimming,
            "Rolling" => Self::Rolling,
            "Attacking" => Self::Attacking,
            "Recovering" => Self::Recovering,
            _ => return None,
        })
    }
}

/// A component a rule changed: the rule, the component and its value before (`None`: absent).
type Saved = (usize, TypeId, Option<Box<dyn Reflect>>);

/// Which of an entity's state rules are applied, and what they replaced.
#[derive(Component, Default)]
pub struct HeldStates {
    rules: Option<Arc<[StateRule]>>,
    held: Vec<usize>,
    saved: Vec<Saved>,
}

type Ruled<'a> = (
    Entity,
    &'a StateRules,
    &'a CharacterState,
    &'a CharacterMove,
);

pub(crate) fn apply_state_rules(world: &mut World) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let types = registry.read();
    let mut ruled = world.query::<Ruled>();
    let wanted: Vec<(Entity, Arc<[StateRule]>, Vec<usize>)> = ruled
        .iter(world)
        .map(|(entity, rules, state, moving)| {
            let holding = rules
                .0
                .iter()
                .enumerate()
                .filter(|(_, rule)| {
                    CharacterCondition::named(&rule.state)
                        .is_some_and(|condition| moving.holds(state, condition))
                })
                .map(|(index, _)| index)
                .collect();
            (entity, rules.0.clone(), holding)
        })
        .collect();
    let mut orphans = world.query_filtered::<Entity, (With<HeldStates>, Without<StateRules>)>();
    let orphans: Vec<Entity> = orphans.iter(world).collect();
    for entity in orphans {
        if let Some(mut held) = world.entity_mut(entity).take::<HeldStates>() {
            leave_all(world, entity, &mut held, &types);
        }
    }

    for (entity, rules, holding) in wanted {
        let mut held = world
            .entity_mut(entity)
            .take::<HeldStates>()
            .unwrap_or_default();
        // Live reload replaced the rules: undo the old ones before applying the new.
        if held
            .rules
            .as_ref()
            .is_some_and(|old| !Arc::ptr_eq(old, &rules))
        {
            leave_all(world, entity, &mut held, &types);
        }
        for index in held.held.clone().into_iter().rev() {
            if !holding.contains(&index) {
                leave(world, entity, &mut held, index, &types);
            }
        }
        for &index in &holding {
            if !held.held.contains(&index) {
                enter(world, entity, &mut held, &rules, index, &types);
            }
        }
        held.rules = Some(rules);
        world.entity_mut(entity).insert(held);
    }
}

fn enter(
    world: &mut World,
    entity: Entity,
    held: &mut HeldStates,
    rules: &[StateRule],
    index: usize,
    types: &bevy::reflect::TypeRegistry,
) {
    let rule = &rules[index];
    let changed = rule
        .enable
        .iter()
        .map(|(type_id, _)| *type_id)
        .chain(rule.disable.iter().copied());
    for type_id in changed {
        let Some(reflect) = types.get_type_data::<ReflectComponent>(type_id) else {
            continue;
        };
        let before = reflect
            .reflect(world.entity(entity))
            .and_then(|value| value.reflect_clone().ok());
        held.saved.push((index, type_id, before));
    }
    let mut target = world.entity_mut(entity);
    for (type_id, value) in &rule.enable {
        if let Some(reflect) = types.get_type_data::<ReflectComponent>(*type_id) {
            reflect.insert(&mut target, value.as_partial_reflect(), types);
        }
    }
    for type_id in &rule.disable {
        if let Some(reflect) = types.get_type_data::<ReflectComponent>(*type_id) {
            reflect.remove(&mut target);
        }
    }
    held.held.push(index);
}

fn leave_all(
    world: &mut World,
    entity: Entity,
    held: &mut HeldStates,
    types: &bevy::reflect::TypeRegistry,
) {
    for index in held.held.clone().into_iter().rev() {
        leave(world, entity, held, index, types);
    }
}

/// Restores what entering the rule changed, latest first.
fn leave(
    world: &mut World,
    entity: Entity,
    held: &mut HeldStates,
    index: usize,
    types: &bevy::reflect::TypeRegistry,
) {
    let mut target = world.entity_mut(entity);
    let mut kept = Vec::new();
    for (rule, type_id, before) in held.saved.drain(..).rev() {
        if rule != index {
            kept.push((rule, type_id, before));
            continue;
        }
        let Some(reflect) = types.get_type_data::<ReflectComponent>(type_id) else {
            continue;
        };
        match before {
            Some(value) => reflect.insert(&mut target, value.as_partial_reflect(), types),
            None => reflect.remove(&mut target),
        }
    }
    kept.reverse();
    held.saved = kept;
    held.held.retain(|&held| held != index);
}
