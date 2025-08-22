use super::{
    component_history::{ComponentHistory, EntityHistory, TickData},
    RollbackRegistry,
};
use crate::{RollbackFrames, RollbackSchedule, RollbackStoreSet, StoreFor, StoreScheduleLabel};

use std::num::NonZero;

use bevy::{
    ecs::{
        archetype::{ArchetypeGeneration, ArchetypeId},
        component::ComponentId,
    },
    prelude::*,
};

pub struct PredictionStorePlugin;

impl Plugin for PredictionStorePlugin {
    fn build(&self, app: &mut App) {
        let schedule = **app.world().resource::<StoreScheduleLabel>();
        app.init_resource::<ArchetypeCache>()
            .add_systems(schedule, run_store.in_set(RollbackStoreSet))
            .add_systems(
                RollbackSchedule::PreRollback,
                save_initial.in_set(RollbackStoreSet),
            );
    }
}

// TODO: Implement cleanup to remove component histories that would entirely evaluate to Missing/Removed

#[derive(Component, Deref, DerefMut, Default, Debug)]
pub struct PredictedHistory {
    #[deref]
    history: EntityHistory,
    last_archetype: Option<ArchetypeId>,
}

fn run_store(world: &mut World) {
    // TODO: Check rollback frames, if it changed and went up, grow histories first

    world.resource_scope::<ArchetypeCache, _>(|world, mut cache| {
        world.resource_scope::<RollbackRegistry, _>(|world, registry| {
            update_archetype_cache(world, &mut cache, &registry);

            world.resource_scope::<StoreFor, _>(|world, tick| {
                store_components(world, &cache, &registry, *tick);
            });
        });
    });

    // TODO: If rollback frames went down, shrink histories afterwards
}

fn save_initial(world: &mut World) {
    world.resource_scope::<ArchetypeCache, _>(|world, mut cache| {
        world.resource_scope::<RollbackRegistry, _>(|world, registry| {
            update_archetype_cache(world, &mut cache, &registry);

            world.resource_scope::<StoreFor, _>(|world, tick| {
                store_initial(world, &cache, &registry, *tick);
            });
        });
    });
}

#[derive(Resource, Deref, DerefMut)]
struct ArchetypeCache {
    generation: ArchetypeGeneration,
    #[deref]
    list: Vec<ArchetypeEntry>,
    no_components: Vec<ArchetypeId>,
}

impl Default for ArchetypeCache {
    fn default() -> Self {
        Self {
            generation: ArchetypeGeneration::initial(),
            list: default(),
            no_components: default(),
        }
    }
}

struct ArchetypeEntry {
    id: ArchetypeId,
    predicted: Vec<(ComponentId, usize)>,
}

fn update_archetype_cache(
    world: &mut World,
    cache: &mut ArchetypeCache,
    registry: &RollbackRegistry,
) {
    let predicted_id = world.register_component::<crate::Predicted>();
    let history_id = world.register_component::<PredictedHistory>();

    for archetype in &world.archetypes()[cache.generation..] {
        if !archetype.contains(predicted_id) || !archetype.contains(history_id) {
            continue;
        }

        let mut predicted = Vec::new();

        for component_id in archetype.components() {
            if let Some(&index) = registry.ids.get(&component_id) {
                predicted.push((component_id, index));
            }
        }

        predicted.sort_by_key(|&(id, _)| id);

        if !predicted.is_empty() {
            cache.list.push(ArchetypeEntry {
                id: archetype.id(),
                predicted,
            });
        } else {
            cache.no_components.push(archetype.id());
        }
    }

    cache.generation = world.archetypes().generation();
}

fn store_components(
    world: &mut World,
    cache: &ArchetypeCache,
    registry: &RollbackRegistry,
    tick: StoreFor,
) {
    let tick = tick.get();
    let hist_size = NonZero::new(
        world
            .get_resource::<RollbackFrames>()
            .copied()
            .unwrap()
            .history_size() as u8,
    )
    .unwrap();

    let world = world.as_unsafe_world_cell();
    let archetypes = world.archetypes();

    for &id in cache.no_components.iter() {
        for entity in archetypes
            .get(id)
            .unwrap()
            .entities()
            .iter()
            .map(|e| e.id())
        {
            let entity_mut = world.get_entity(entity).unwrap();
            // SAFETY: We don't do structural changes in this system
            let Some(mut history) = (unsafe { entity_mut.get_mut::<PredictedHistory>() }) else {
                continue;
            };

            if history.last_archetype.is_some() {
                for comp_hist in history.values_mut() {
                    if comp_hist.first_tick() >= tick {
                        // Don't write Removed histories that haven't started yet
                        continue;
                    }
                    comp_hist.mark_removed(tick);
                }
                history.last_archetype = None;
            }
        }
    }

    for entry in cache.iter() {
        for entity in archetypes
            .get(entry.id)
            .unwrap()
            .entities()
            .iter()
            .map(|e| e.id())
        {
            let entity = world.get_entity(entity).unwrap();
            // SAFETY: We don't do structural changes in this system
            let Some(mut history) = (unsafe { entity.get_mut::<PredictedHistory>() }) else {
                continue;
            };

            if let Some(last_archetype) = history.last_archetype {
                if last_archetype != entry.id {
                    // Archetype changed, check for components that should be marked removed
                    for (component_id, comp_hist) in history.iter_mut() {
                        if comp_hist.first_tick() >= tick {
                            // Don't write Removed histories that haven't started yet
                            continue;
                        }
                        if !entry.predicted.iter().any(|(id, _)| id == component_id) {
                            comp_hist.mark_removed(tick);
                        }
                    }
                }
            }
            history.last_archetype = Some(entry.id);

            // Store current values to histories, or create them
            for &(component_id, registry_index) in entry.predicted.iter() {
                let component = &registry.components[registry_index];

                let history = history
                    .entry(component_id)
                    .or_insert_with(|| ComponentHistory::from_component(component, hist_size));
                // SAFETY: We don't do structural changes in this system
                let ptr = unsafe { entity.get_mut_by_id(component_id) }.unwrap();
                if !ptr.is_changed() {
                    continue;
                }
                if let TickData::Value(prev_ptr) = history.get_latest(tick.saturating_sub(1)) {
                    // SAFETY: Both the history and component were fetched using the same ComponentId
                    let equal = unsafe { component.equal(prev_ptr, ptr.as_ref()) };
                    if equal {
                        continue;
                    }
                }
                // SAFETY: Both the history and component were fetched using the same ComponentId
                unsafe { history.write(tick, |dst| component.store(ptr.as_ref(), dst)) };
            }
        }
    }
}

fn store_initial(
    world: &mut World,
    cache: &ArchetypeCache,
    registry: &RollbackRegistry,
    tick: StoreFor,
) {
    let tick = tick.get();
    let hist_size = NonZero::new(
        world
            .get_resource::<RollbackFrames>()
            .copied()
            .unwrap()
            .history_size() as u8,
    )
    .unwrap();

    let world = world.as_unsafe_world_cell();
    let archetypes = world.archetypes();
    // SAFETY: We don't do structural changes in this system
    let world = unsafe { world.world_mut() };

    for entry in cache.iter() {
        for entity in archetypes
            .get(entry.id)
            .unwrap()
            .entities()
            .iter()
            .map(|e| e.id())
        {
            let entity = world.as_unsafe_world_cell().get_entity(entity).unwrap();
            // SAFETY: We don't do structural changes in this system
            let Some(mut history) = (unsafe { entity.get_mut::<PredictedHistory>() }) else {
                continue;
            };

            if let Some(last_archetype) = history.last_archetype {
                if last_archetype == entry.id {
                    // The archetype hasn't changed so there cannot be any new components
                    continue;
                }
            }

            // Store current values to histories, or create them
            for &(component_id, registry_index) in entry.predicted.iter() {
                if history.contains_key(&component_id) {
                    continue;
                }

                let component = &registry.components[registry_index];
                let mut comp_hist = ComponentHistory::from_component(component, hist_size);

                // SAFETY: We don't do structural changes in this system
                let ptr = unsafe { entity.get_by_id(component_id) }.unwrap();
                // SAFETY: Both the history and component were fetched using the same ComponentId
                unsafe { comp_hist.write(tick, |dst| component.store(ptr, dst)) };
                history.insert(component_id, comp_hist);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::{
        super::{component_history::TickData, test_utils::*},
        PredictedHistory, RollbackRegistry,
    };
    use crate::{Predicted, RollbackFrames};
    use TickData::*;

    use bevy::prelude::*;
    use bevy_replicon::shared::replicon_tick::RepliconTick;

    fn init_app() -> App {
        let mut app = App::new();
        app.init_resource::<super::ArchetypeCache>()
            .init_resource::<RollbackFrames>()
            .add_systems(Update, super::run_store);
        app
    }

    #[test]
    fn history_stores_changes() {
        let mut app = init_app();

        let e1 = app
            .world_mut()
            .spawn((Predicted, PredictedHistory::default(), A(1)))
            .id();
        let e2 = app
            .world_mut()
            .spawn((Predicted, PredictedHistory::default(), A(12)))
            .id();

        let mut registry = RollbackRegistry::default();
        registry.register::<A>(app.world_mut());
        app.insert_resource(registry);

        for i in 0..=5 {
            app.insert_resource(super::StoreFor(RepliconTick::new(i)));
            app.update();
            **app.world_mut().entity_mut(e1).get_mut::<A>().unwrap() += 1;
            **app.world_mut().entity_mut(e2).get_mut::<A>().unwrap() -= 1;
        }

        let world = app.world_mut();
        let comp_a = world.register_component::<A>();
        use Missing as M;

        let e = world.entity(e1);
        let hist = e.get::<PredictedHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        for (i, v) in [a(1), a(2), a(3), a(4), a(5), a(6), M].iter_enumerate() {
            assert_eq!(v, hist.get(&comp_a).unwrap().get(i as u32).deref().cloned());
        }

        let e = world.entity(e2);
        let hist = e.get::<PredictedHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        for (i, v) in [a(12), a(11), a(10), a(9), a(8), a(7), M].iter_enumerate() {
            assert_eq!(v, hist.get(&comp_a).unwrap().get(i as u32).deref().cloned());
        }
    }

    #[test]
    fn stores_removed() {
        let mut app = init_app();

        let e1 = app
            .world_mut()
            .spawn((Predicted, PredictedHistory::default(), A(1)))
            .id();
        let e2 = app
            .world_mut()
            .spawn((Predicted, PredictedHistory::default(), A(12)))
            .id();

        let mut registry = RollbackRegistry::default();
        registry.register::<A>(app.world_mut());
        app.insert_resource(registry);

        for i in 0..=5 {
            if i == 1 {
                app.world_mut().entity_mut(e1).remove::<A>();
            }
            if i == 3 {
                app.world_mut().entity_mut(e2).remove::<A>();
            }

            app.insert_resource(super::StoreFor(RepliconTick::new(i)));
            app.update();
        }

        let world = app.world_mut();
        let comp_a = world.register_component::<A>();

        let e = world.entity(e1);
        let hist = e.get::<PredictedHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        for i in 0..=5 {
            let v = hist
                .get(&comp_a)
                .unwrap()
                .get(i as u32)
                .deref::<()>()
                .copied();
            if i == 1 {
                assert_eq!(Removed, v);
            } else {
                assert_ne!(Removed, v);
            }
        }

        let e = world.entity(e2);
        let hist = e.get::<PredictedHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        for i in 0..=5 {
            let v = hist
                .get(&comp_a)
                .unwrap()
                .get(i as u32)
                .deref::<()>()
                .copied();
            if i == 3 {
                assert_eq!(Removed, v);
            } else {
                assert_ne!(Removed, v);
            }
        }
    }

    #[test]
    fn history_skips_unchanged() {
        let mut app = init_app();

        let e1 = app
            .world_mut()
            .spawn((Predicted, PredictedHistory::default(), A(1), F(f32::NAN)))
            .id();
        let e2 = app
            .world_mut()
            .spawn((Predicted, PredictedHistory::default(), A(10), F(f32::NAN)))
            .id();

        let mut registry = RollbackRegistry::default();
        registry.register::<A>(app.world_mut());
        registry.register::<F>(app.world_mut());
        app.insert_resource(registry);

        for i in 0..7 {
            app.insert_resource(super::StoreFor(RepliconTick::new(i)));
            app.update();

            let change = (i % 3 == 2) as u16;
            // Always mark e1 changed, even if the value is unchanged
            **app.world_mut().entity_mut(e1).get_mut::<A>().unwrap() += change;
            **app.world_mut().entity_mut(e1).get_mut::<F>().unwrap() += change as f32;

            // Only mark e2's A and F changed if we try to change it
            if i % 3 == 0 {
                **app.world_mut().entity_mut(e2).get_mut::<A>().unwrap() += 1;
                **app.world_mut().entity_mut(e2).get_mut::<F>().unwrap() += 1.;
            }
        }

        let world = app.world_mut();
        let comp_a = world.register_component::<A>();
        let comp_f = world.register_component::<F>();
        use Missing as M;

        let e = world.entity(e1);
        let hist = e.get::<PredictedHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        assert!(hist.contains_key(&comp_f));
        for (i, v) in [a(1), M, M, a(2), M, M, a(3), M].iter_enumerate() {
            assert_eq!(v, hist.get(&comp_a).unwrap().get(i as u32).deref().cloned());
        }
        for i in 0..7 {
            let entry = hist.get(&comp_f).unwrap().get(i as u32);
            assert!(matches!(entry, Value(_)));
            let TickData::Value(f) = entry.deref::<F>() else {
                panic!();
            };
            assert!(f.is_nan());
        }

        let e = world.entity(e2);
        let hist = e.get::<PredictedHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        assert!(hist.contains_key(&comp_f));
        for (i, v) in [a(10), a(11), M, M, a(12), M, M, M].iter_enumerate() {
            assert_eq!(v, hist.get(&comp_a).unwrap().get(i as u32).deref().cloned());
        }
        let (v, m) = (true, false);
        for (i, v) in [v, v, m, m, v, m, m, m].iter_enumerate() {
            if v {
                let entry = hist.get(&comp_f).unwrap().get(i as u32);
                assert!(matches!(entry, Value(_)));
                let TickData::Value(f) = entry.deref::<F>() else {
                    panic!();
                };
                assert!(f.is_nan());
            } else {
                assert_eq!(
                    Missing,
                    hist.get(&comp_f)
                        .unwrap()
                        .get(i as u32)
                        .deref::<F>()
                        .cloned()
                );
            }
        }
    }

    #[test]
    fn stores_reinserts() {
        let mut app = init_app();

        let e1 = app
            .world_mut()
            .spawn((Predicted, PredictedHistory::default(), A(1)))
            .id();
        let e2 = app
            .world_mut()
            .spawn((Predicted, PredictedHistory::default(), A(12)))
            .id();

        let mut registry = RollbackRegistry::default();
        registry.register::<A>(app.world_mut());
        app.insert_resource(registry);

        for i in 0..=5 {
            if i == 1 {
                app.world_mut().entity_mut(e1).remove::<A>();
            }
            if i == 2 {
                app.world_mut().entity_mut(e1).insert(A(2));
                app.world_mut().entity_mut(e2).remove::<A>();
            }
            if i == 3 {
                app.world_mut().entity_mut(e2).insert(A(20));
            }

            app.insert_resource(super::StoreFor(RepliconTick::new(i)));
            app.update();
        }

        let world = app.world_mut();
        let comp_a = world.register_component::<A>();
        use Removed as R;

        let e = world.entity(e1);
        let hist = e.get::<PredictedHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        for (i, v) in [(0, a(1)), (1, R), (2, a(2))] {
            assert_eq!(v, hist.get(&comp_a).unwrap().get(i as u32).deref().cloned());
        }

        let e = world.entity(e2);
        let hist = e.get::<PredictedHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        for (i, v) in [(0, a(12)), (2, R), (3, a(20))] {
            assert_eq!(v, hist.get(&comp_a).unwrap().get(i as u32).deref().cloned());
        }
    }

    #[test]
    fn stores_inserts() {
        let mut app = init_app();

        let e1 = app
            .world_mut()
            .spawn((Predicted, PredictedHistory::default(), A(1)))
            .id();
        let e2 = app
            .world_mut()
            .spawn((Predicted, PredictedHistory::default()))
            .id();

        let mut registry = RollbackRegistry::default();
        registry.register::<A>(app.world_mut());
        registry.register::<B>(app.world_mut());
        app.insert_resource(registry);

        for i in 0..=5 {
            if i == 2 {
                app.world_mut().entity_mut(e1).insert(B);
            }
            if i == 3 {
                app.world_mut().entity_mut(e2).insert(A(2));
            }

            app.insert_resource(super::StoreFor(RepliconTick::new(i)));
            app.update();
        }

        let world = app.world_mut();
        let comp_a = world.register_component::<A>();
        let comp_b = world.register_component::<B>();

        let e = world.entity(e1);
        let hist = e.get::<PredictedHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        assert!(hist.contains_key(&comp_b));
        for (i, v) in [(1, Missing), (2, Value(B))] {
            assert_eq!(v, hist.get(&comp_b).unwrap().get(i as u32).deref().cloned());
        }

        let e = world.entity(e2);
        let hist = e.get::<PredictedHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        for (i, v) in [(2, Missing), (3, Value(A(2)))] {
            assert_eq!(v, hist.get(&comp_a).unwrap().get(i as u32).deref().cloned());
        }
    }

    #[test]
    fn drop_once_unique_values() {
        let mut app = init_app();
        let drops = DropList::default();

        let mut registry = RollbackRegistry::default();
        registry.register::<D>(app.world_mut());
        app.insert_resource(registry);

        let e1 = app
            .world_mut()
            .spawn((Predicted, PredictedHistory::default(), D::new(1, &drops)))
            .id();

        for i in 0..5 {
            app.insert_resource(super::StoreFor(RepliconTick::new(i)));
            app.update();
            app.world_mut().entity_mut(e1).get_mut::<D>().unwrap().0 += 1;
        }

        assert_drops(&drops, []);

        app.world_mut().despawn(e1);

        assert_drops(&drops, [6, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn drop_once_duplicates() {
        let mut app = init_app();
        let drops = DropList::default();

        let mut registry = RollbackRegistry::default();
        registry.register::<D>(app.world_mut());
        app.insert_resource(registry);

        let e1 = app
            .world_mut()
            .spawn((Predicted, PredictedHistory::default(), D::new(1, &drops)))
            .id();

        for i in 0..5 {
            app.insert_resource(super::StoreFor(RepliconTick::new(i)));
            app.update();
            // Mark the component changes to make sure the more complex branch is used
            app.world_mut()
                .entity_mut(e1)
                .get_mut::<D>()
                .unwrap()
                .set_changed();
        }
        // Update afterwards to prevent a double drop of D(1)
        app.world_mut().entity_mut(e1).get_mut::<D>().unwrap().0 += 1;

        assert_drops(&drops, []);

        app.world_mut().despawn(e1);

        assert_drops(&drops, [2, 1]);
    }

    #[test]
    fn drop_once_out_of_bounds() {
        let mut app = init_app();
        let drops = DropList::default();

        let mut registry = RollbackRegistry::default();
        registry.register::<D>(app.world_mut());
        app.insert_resource(registry);

        let e1 = app
            .world_mut()
            .spawn((Predicted, PredictedHistory::default(), D::new(1, &drops)))
            .id();

        app.insert_resource(super::StoreFor(RepliconTick::new(10)));
        app.update();
        app.world_mut().entity_mut(e1).get_mut::<D>().unwrap().0 += 1;

        // Write to a tick that is old enough that it won't be written
        app.insert_resource(super::StoreFor(RepliconTick::new(2)));
        app.update();

        // The value was never cloned so it should not have been dropped either
        assert_drops(&drops, []);

        app.world_mut().despawn(e1);

        assert_drops(&drops, [2, 1]);
    }

    // TODO: Test cleanup of histories
}
