use std::collections::BTreeMap;

use crate::{
    backend,
    input::{self, InputPress, PressStage},
    output::{
        Down, Move, Out, Over, PointerDown, PointerInteraction, PointerMove, PointerOut,
        PointerOver, PointerUp, Up,
    },
    PointerId,
};
use bevy::{
    prelude::*,
    ui::FocusPolicy,
    utils::{FloatOrd, HashMap, HashSet},
};

type DepthMap = BTreeMap<FloatOrd, Entity>;
type LayerMap = BTreeMap<PickLayer, DepthMap>;

/// Assigns an entity to a picking layer. When computing picking focus, entities
/// are sorted in order from the highest to lowest layer, and by depth within each layer.
#[derive(Debug, Clone, Copy, Component, PartialEq, Eq, PartialOrd, Ord)]
pub enum PickLayer {
    Top = 0,
    AboveUi = 1,
    UI = 2,
    BelowUi = 3,
    AboveWorld = 4,
    World = 5,
    BelowWorld = 6,
    Bottom = 7,
}
impl Default for PickLayer {
    fn default() -> Self {
        PickLayer::World
    }
}

pub fn update_focus(
    // Inputs
    focus: Query<&FocusPolicy>,
    pick_layers: Query<&PickLayer>,
    pointers: Query<(&PointerId, &PointerInteraction)>, // <- what happened last frame
    mut input_presses: EventReader<input::InputPress>,
    mut under_pointer: EventReader<backend::EntitiesUnderPointer>,
    mut pointer_move_in: EventReader<input::InputMove>,
    // Locals
    mut pointer_map: Local<HashMap<PointerId, LayerMap>>,
    mut hover_map: Local<HashMap<PointerId, HashSet<Entity>>>,
    // Output
    mut pointer_move: EventWriter<PointerMove>,
    mut pointer_over: EventWriter<PointerOver>,
    mut pointer_out: EventWriter<PointerOut>,
    mut pointer_up: EventWriter<PointerUp>,
    mut pointer_down: EventWriter<PointerDown>,
) {
    reset_local_maps(&mut hover_map, &mut pointer_map);
    build_pointer_map(pick_layers, &mut under_pointer, &mut pointer_map);
    build_hover_map(&pointers, focus, pointer_map, &mut hover_map);

    let input_presses: Vec<&InputPress> = input_presses.iter().collect();

    for event in pointer_move_in.iter() {
        for hover_entity in hover_map.get(&event.id).iter().flat_map(|h| h.iter()) {
            pointer_move.send(PointerMove::new(&event.id, hover_entity, Move))
        }
    }

    for (pointer_id, pointer_interaction) in pointers.iter() {
        let just_pressed = input_presses
            .iter()
            .filter_map(|click| (&click.id == pointer_id).then_some(click.press))
            .last();

        // If the entity is hovered...
        for hover_entity in hover_map.get(pointer_id).iter().flat_map(|h| h.iter()) {
            // ...but was not hovered last frame...
            if matches!(
                pointer_interaction.get(hover_entity),
                Some(Interaction::None) | None
            ) {
                pointer_over.send(PointerOver::new(pointer_id, hover_entity, Over));
            }

            match just_pressed {
                Some(PressStage::Down) => {
                    pointer_down.send(PointerDown::new(pointer_id, hover_entity, Down));
                }
                Some(PressStage::Up) => {
                    pointer_up.send(PointerUp::new(pointer_id, hover_entity, Up));
                }
                None => (),
            }
        }

        if let Some(hover_entities) = hover_map.get(pointer_id) {
            // If the entity was hovered last frame...
            for entity in pointer_interaction
                .iter()
                .filter_map(|(entity, interaction)| {
                    matches!(interaction, Interaction::Hovered | Interaction::Clicked)
                        .then_some(entity)
                })
            {
                // ...but is now not being hovered...
                if !hover_entities.contains(entity) {
                    if matches!(just_pressed, Some(PressStage::Up)) {
                        // ...the pointer is considered just up on this entity even though it was
                        // not hovering the entity this frame
                        pointer_up.send(PointerUp::new(pointer_id, entity, Up));
                    }
                    pointer_out.send(PointerOut::new(pointer_id, entity, Out));
                }
            }
        }
    }
}

/// Clear local maps, reusing allocated memory.
fn reset_local_maps(
    hover_map: &mut Local<
        bevy::utils::hashbrown::HashMap<PointerId, bevy::utils::hashbrown::HashSet<Entity>>,
    >,
    pointer_map: &mut Local<
        bevy::utils::hashbrown::HashMap<PointerId, BTreeMap<PickLayer, BTreeMap<FloatOrd, Entity>>>,
    >,
) {
    for entity_set in hover_map.values_mut() {
        if !entity_set.is_empty() {
            entity_set.clear()
        }
    }
    for layer_map in pointer_map.values_mut() {
        for depth_map in layer_map.values_mut() {
            if !depth_map.is_empty() {
                depth_map.clear()
            }
        }
    }
}

/// Build an ordered map of entities that are under each pointer
fn build_pointer_map(
    pick_layers: Query<&PickLayer>,
    over_events: &mut EventReader<backend::EntitiesUnderPointer>,
    pointer_map: &mut Local<HashMap<PointerId, BTreeMap<PickLayer, BTreeMap<FloatOrd, Entity>>>>,
) {
    for event in over_events.iter() {
        let layer_map = match pointer_map.get_mut(&event.id) {
            Some(map) => map,
            None => pointer_map.try_insert(event.id, BTreeMap::new()).unwrap(),
        };
        for over in event.over_list.iter() {
            let layer = pick_layers
                .get(over.entity)
                .map(|layer| *layer)
                .unwrap_or_else(|_error| {
                    error!(
                        "Pickable entity {:?} doesn't have a `PickLayer` component",
                        over.entity
                    );
                    PickLayer::default()
                });

            layer_map.entry(layer).or_insert_with(BTreeMap::new);

            let depth_map = layer_map.get_mut(&layer).unwrap();
            depth_map.insert(FloatOrd(over.depth), over.entity);
        }
    }
}

// Build an unsorted set of hovered entities, accounting for depth, layer, and focus policy.
fn build_hover_map(
    pointers: &Query<(&PointerId, &PointerInteraction)>,
    focus: Query<&FocusPolicy>,
    pointer_map: Local<HashMap<PointerId, BTreeMap<PickLayer, BTreeMap<FloatOrd, Entity>>>>,
    hover_map: &mut Local<HashMap<PointerId, HashSet<Entity>>>,
) {
    for (id, _) in pointers.iter() {
        let pointer_entity_set = match hover_map.contains_key(id) {
            true => hover_map.get_mut(&id).unwrap(),
            false => hover_map.try_insert(*id, HashSet::new()).unwrap(),
        };
        if let Some(layer_map) = pointer_map.get(id) {
            for depth_map in layer_map.values() {
                for entity in depth_map.values() {
                    pointer_entity_set.insert(*entity);
                    if let Ok(_policy @ FocusPolicy::Block) = focus.get(*entity) {
                        break;
                    }
                }
            }
        }
    }
}
