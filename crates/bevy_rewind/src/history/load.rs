use super::{
    authoritative::AuthoritativeHistory,
    batch::{InsertBatch, RemoveBatch},
    component_history::TickData,
    predicted::PredictedHistory,
    RollbackRegistry,
};
use crate::{LoadFrom, Predicted, RollbackLoadSet, RollbackSchedule};

use bevy::{
    ecs::{
        archetype::Archetype,
        entity::Entities,
        world::{CommandQueue, EntityMutExcept},
    },
    prelude::*,
};
use bevy_replicon::{
    client::{confirm_history::ConfirmHistory, server_mutate_ticks::ServerMutateTicks},
    shared::replicon_tick::RepliconTick,
};

pub struct HistoryLoadPlugin;

impl Plugin for HistoryLoadPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            RollbackSchedule::PreResimulation,
            (load_confirmed_authoritative, reinsert_predicted)
                .chain()
                .in_set(RollbackLoadSet),
        )
        .add_systems(RollbackSchedule::Rollback, load_and_clear_prediction);
    }
}

fn load_and_clear_prediction(
    mut commands: Commands,
    mut q: Query<
        (
            Entity,
            &mut PredictedHistory,
            Option<(&AuthoritativeHistory, &ConfirmHistory)>,
        ),
        With<Predicted>,
    >,
    registry: Res<RollbackRegistry>,
    previous_tick: Res<LoadFrom>,
    global_confirm: Res<ServerMutateTicks>,
    entities: &Entities,
) {
    let mut inserts = InsertBatch::new();
    let mut load_queue = CommandQueue::default();
    let mut removes = RemoveBatch::new();

    // TODO: Can we par_iter this?
    for (entity, mut predicted, maybe_authoritative) in q.iter_mut() {
        let mut load_commands = Commands::new_from_entities(&mut load_queue, entities);
        for (&comp_id, pred_hist) in predicted.iter_mut() {
            let &reg_idx = registry.ids.get(&comp_id).unwrap();
            let component = registry.components.get(reg_idx).unwrap();

            let auth = maybe_authoritative
                .map(|(authoritative, confirmed)| {
                    if let Some(auth_hist) = authoritative.get(&comp_id) {
                        let check_range = auth_hist.empty_after(previous_tick.get());
                        let end_tick = RepliconTick::new(previous_tick.get() + check_range);
                        if confirmed.contains_any(**previous_tick, end_tick)
                            || global_confirm.contains_any(**previous_tick, end_tick)
                        {
                            return auth_hist.get_latest(previous_tick.get());
                        }
                    }
                    TickData::Missing
                })
                .unwrap_or(TickData::Missing);

            let pred = pred_hist.get_latest(previous_tick.get());

            match (auth, pred) {
                (TickData::Removed, _) | (TickData::Missing, TickData::Removed) => {
                    removes.push(comp_id);
                }
                (TickData::Missing, TickData::Missing) => {
                    // We are loading a value from before the history
                    // remove the component until the history starts
                    removes.push(comp_id);
                    pred_hist.keep_first_item();
                    continue;
                }
                (auth, pred) => {
                    inserts.push(comp_id, component, |dst| unsafe {
                        component.load_to_uninit(
                            auth.value(),
                            pred.value(),
                            dst,
                            load_commands.reborrow(),
                            entity,
                        );
                    });
                }
            }

            pred_hist.clean(previous_tick.get());
        }

        if !inserts.is_empty() {
            commands.entity(entity).queue(inserts.clone());
            inserts.clear();
        }

        if !removes.is_empty() {
            commands.entity(entity).queue(removes.clone());
            removes.clear();
        }

        if !load_queue.is_empty() {
            let mut queue = std::mem::take(&mut load_queue);
            commands.queue(move |world: &mut World| queue.apply(world));
        }
    }
}

fn load_confirmed_authoritative(
    mut commands: Commands,
    mut q: Query<
        (
            EntityMutExcept<(AuthoritativeHistory, ConfirmHistory)>,
            &AuthoritativeHistory,
            &ConfirmHistory,
        ),
        With<Predicted>,
    >,
    registry: Res<RollbackRegistry>,
    previous_tick: Res<LoadFrom>,
    global_confirm: Res<ServerMutateTicks>,
    entities: &Entities,
) {
    let mut inserts = InsertBatch::new();
    let mut load_queue = CommandQueue::default();
    let mut removes = RemoveBatch::new();

    // TODO: Can we par_iter this?
    for (entity, authoritative, confirmed) in q.iter_mut() {
        let mut load_commands = Commands::new_from_entities(&mut load_queue, entities);
        for (&comp_id, auth_hist) in authoritative.iter() {
            let &reg_idx = registry.ids.get(&comp_id).unwrap();
            let component = registry.components.get(reg_idx).unwrap();

            let check_range = auth_hist.empty_after(previous_tick.get());
            let end_tick = RepliconTick::new(previous_tick.get() + check_range);
            if !confirmed.contains_any(**previous_tick, end_tick)
                && !global_confirm.contains_any(**previous_tick, end_tick)
            {
                continue;
            }

            match auth_hist.get_latest(previous_tick.get()) {
                TickData::Value(value) => {
                    inserts.push(comp_id, component, |dst| unsafe {
                        component.load_to_uninit(
                            Some(value),
                            entity.get_by_id(comp_id),
                            dst,
                            load_commands.reborrow(),
                            entity.id(),
                        );
                    });
                    continue;
                }
                TickData::Removed => {
                    removes.push(comp_id);
                    continue;
                }
                TickData::Missing => {}
            }
        }

        if !inserts.is_empty() {
            commands.entity(entity.id()).queue(inserts.clone());
            inserts.clear();
        }

        if !removes.is_empty() {
            commands.entity(entity.id()).queue(removes.clone());
            removes.clear();
        }

        if !load_queue.is_empty() {
            let mut queue = std::mem::take(&mut load_queue);
            commands.queue(move |world: &mut World| queue.apply(world));
        }
    }
}

fn reinsert_predicted(
    mut commands: Commands,
    mut q: Query<(Entity, &Archetype, &PredictedHistory, &AuthoritativeHistory), With<Predicted>>,
    registry: Res<RollbackRegistry>,
    previous_tick: Res<LoadFrom>,
    entities: &Entities,
) {
    let mut inserts = InsertBatch::new();
    let mut load_queue = CommandQueue::default();

    // TODO: Can we par_iter this?
    for (entity, archetype, predicted, authoritative) in q.iter_mut() {
        let mut load_commands = Commands::new_from_entities(&mut load_queue, entities);
        for (&comp_id, pred_hist) in predicted.iter() {
            if archetype.contains(comp_id) {
                continue;
            }

            let TickData::Value(value) = pred_hist.get(previous_tick.get()) else {
                continue;
            };

            // TODO: only insert if authoritative is not known yet
            _ = authoritative;

            let &reg_idx = registry.ids.get(&comp_id).unwrap();
            let component = registry.components.get(reg_idx).unwrap();

            inserts.push(comp_id, component, |dst| unsafe {
                component.load_to_uninit(None, Some(value), dst, load_commands.reborrow(), entity);
            });
        }

        if !inserts.is_empty() {
            commands.entity(entity).queue(inserts.clone());
            inserts.clear();
        }

        if !load_queue.is_empty() {
            let mut queue = std::mem::take(&mut load_queue);
            commands.queue(move |world: &mut World| queue.apply(world));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{LoadFrom, Predicted};

    use super::{
        super::{
            component_history::TickData, load::load_confirmed_authoritative,
            predicted::PredictedHistory, test_utils::*,
        },
        load_and_clear_prediction, RollbackRegistry,
    };
    use bevy::{
        ecs::{component::ComponentId, system::ScheduleSystem},
        prelude::*,
    };
    use bevy_replicon::{
        client::server_mutate_ticks::ServerMutateTicks, shared::replicon_tick::RepliconTick,
    };

    fn init_app<C: Component + Clone + PartialEq, M>(
        load_from: u32,
        system: impl IntoScheduleConfigs<ScheduleSystem, M>,
    ) -> (App, ComponentId) {
        let mut app = App::new();
        app.add_systems(Update, system)
            .init_resource::<ServerMutateTicks>()
            .insert_resource(LoadFrom(RepliconTick::new(load_from)));

        let mut registry = RollbackRegistry::default();
        registry.register::<C>(app.world_mut());
        app.insert_resource(registry);

        let comp_id = app.world_mut().register_component::<C>();

        (app, comp_id)
    }

    #[test]
    fn load_predicted_no_authoritative() {
        let (mut app, comp_a) = init_app::<A, _>(0, load_and_clear_prediction);

        let pred_hist = pred_history(0, comp_a, [a(5)]);
        let e1 = app.world_mut().spawn((Predicted, pred_hist, A(1))).id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
    }

    #[test]
    fn load_predicted_missing_authoritative() {
        let (mut app, comp_a) = init_app::<A, _>(1, load_and_clear_prediction);

        let pred_hist = pred_history(1, comp_a, [a(5)]);
        let auth_hist = auth_history::<A>(0, comp_a, []);
        let confirm = confirm_history([0, 1, 2]);
        let e1 = app
            .world_mut()
            .spawn((Predicted, pred_hist, auth_hist, confirm, A(0)))
            .id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
    }

    #[test]
    fn load_predicted_unconfirmed_authoritative() {
        let (mut app, comp_a) = init_app::<A, _>(1, load_and_clear_prediction);

        let pred_hist = pred_history(1, comp_a, [a(5)]);
        let auth_hist = auth_history(1, comp_a, [a(10), a(15)]);
        let confirm = confirm_history([0, 2]); // Only the previous and next tick are confirmed
        let e1 = app
            .world_mut()
            .spawn((Predicted, pred_hist, auth_hist, confirm, A(0)))
            .id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
    }

    #[test]
    fn load_authoritative_direct_confirm() {
        let (mut app, comp_a) = init_app::<A, _>(0, load_and_clear_prediction);

        let pred_hist = pred_history::<A>(0, comp_a, []);
        let auth_hist = auth_history(0, comp_a, [a(5)]);
        let confirm = confirm_history([0]); // The target tick is confirmed
        let e1 = app
            .world_mut()
            .spawn((Predicted, pred_hist, auth_hist, confirm, A(1)))
            .id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
    }

    #[test]
    fn load_authoritative_direct_global_confirm() {
        let (mut app, comp_a) = init_app::<A, _>(0, load_and_clear_prediction);

        let pred_hist = pred_history::<A>(0, comp_a, []);
        let auth_hist = auth_history(0, comp_a, [a(5)]);
        let confirm = confirm_history([]); // The tick is unconfirmed on the entity
        let e1 = app
            .world_mut()
            .spawn((Predicted, pred_hist, auth_hist, confirm, A(1)))
            .id();

        // The tick is confirmed globally
        app.world_mut()
            .resource_mut::<ServerMutateTicks>()
            .confirm(r_tick(0), 1);

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
    }

    #[test]
    fn load_authoritative_future_empty_confirm() {
        let (mut app, comp_a) = init_app::<A, _>(0, load_and_clear_prediction);

        let pred_hist = pred_history::<A>(0, comp_a, []);
        let auth_hist = auth_history(0, comp_a, [a(5)]);
        let confirm = confirm_history([1]); // A future empty tick is confirmed
        let e1 = app
            .world_mut()
            .spawn((Predicted, pred_hist, auth_hist, confirm, A(1)))
            .id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
    }

    #[test]
    fn load_authoritative_future_empty_global_confirm() {
        let (mut app, comp_a) = init_app::<A, _>(0, load_and_clear_prediction);

        let pred_hist = pred_history::<A>(0, comp_a, []);
        let auth_hist = auth_history(0, comp_a, [a(5)]);
        let confirm = confirm_history([]); // No ticks are confirmed on the entity
        let e1 = app
            .world_mut()
            .spawn((Predicted, pred_hist, auth_hist, confirm, A(1)))
            .id();

        // A future tick is confirmed globally
        app.world_mut()
            .resource_mut::<ServerMutateTicks>()
            .confirm(r_tick(1), 1);

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
    }

    #[test]
    fn remove_predicted() {
        let (mut app, comp_a) = init_app::<A, _>(0, load_and_clear_prediction);

        let pred_hist = pred_history::<A>(0, comp_a, [TickData::Removed]);
        let e1 = app.world_mut().spawn((Predicted, pred_hist, A(1))).id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(None, e.get::<A>());
    }

    #[test]
    fn remove_authoritative() {
        let (mut app, comp_a) = init_app::<A, _>(0, load_and_clear_prediction);

        let pred_hist = pred_history(0, comp_a, [a(2)]);
        let auth_hist = auth_history::<A>(0, comp_a, [TickData::Removed]);
        let confirm = confirm_history([0]); // The target tick is confirmed
        let e1 = app
            .world_mut()
            .spawn((Predicted, pred_hist, auth_hist, confirm, A(1)))
            .id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(None, e.get::<A>());
    }

    #[test]
    fn insert_predicted() {
        let (mut app, comp_a) = init_app::<A, _>(0, load_and_clear_prediction);

        let pred_hist = pred_history(0, comp_a, [a(5)]);
        let e1 = app.world_mut().spawn((Predicted, pred_hist)).id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
    }

    #[test]
    fn insert_authoritative() {
        let (mut app, comp_a) = init_app::<A, _>(0, load_and_clear_prediction);

        let pred_hist = pred_history::<A>(0, comp_a, []);
        let auth_hist = auth_history(0, comp_a, [a(5)]);
        let confirm = confirm_history([0]); // The target tick is confirmed
        let e1 = app
            .world_mut()
            .spawn((Predicted, pred_hist, auth_hist, confirm))
            .id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
    }

    #[test]
    fn change_detection() {
        // TODO
    }

    #[test]
    fn clears_predicted() {
        let (mut app, comp_a) = init_app::<A, _>(1, load_and_clear_prediction);

        let pred_hist = pred_history::<A>(0, comp_a, [a(4), a(5), a(6)]);
        let e1 = app.world_mut().spawn((Predicted, pred_hist, A(1))).id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
        assert_eq!(
            2,
            e.get::<PredictedHistory>()
                .unwrap()
                .get(&comp_a)
                .unwrap()
                .len()
        );

        app.insert_resource(LoadFrom(RepliconTick::new(0)));
        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(4)), e.get::<A>());
        assert_eq!(
            1,
            e.get::<PredictedHistory>()
                .unwrap()
                .get(&comp_a)
                .unwrap()
                .len()
        );
    }

    #[test]
    fn retains_predicted_for_reinsert() {
        let (mut app, comp_a) = init_app::<A, _>(0, load_and_clear_prediction);

        let pred_hist = pred_history::<A>(2, comp_a, [a(4), a(5), a(6)]);
        let e1 = app.world_mut().spawn((Predicted, pred_hist, A(1))).id();

        app.update();

        let e = app.world().entity(e1);
        // The value should've been removed, but the first item should be retained
        assert_eq!(None, e.get::<A>());
        assert_eq!(
            1,
            e.get::<PredictedHistory>()
                .unwrap()
                .get(&comp_a)
                .unwrap()
                .len()
        );

        // We should be able to load the item again when we get back to that tick
        app.insert_resource(LoadFrom(RepliconTick::new(2)));
        app.update();
        let e = app.world().entity(e1);
        assert_eq!(Some(&A(4)), e.get::<A>());
    }

    #[test]
    fn skip_unpredicted() {
        let (mut app, comp_a) = init_app::<A, _>(0, load_and_clear_prediction);

        // Spawn an entity with the history but no Predicted, it should stay untouched
        let pred_hist = pred_history::<A>(0, comp_a, [a(5)]);
        let e1 = app.world_mut().spawn((pred_hist, A(1))).id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(1)), e.get::<A>());
    }

    ///////////////////////////////////////////////////
    /////   load_confirmed_authoritative tests   //////
    ///////////////////////////////////////////////////

    #[test]
    fn load_confirmed_authoritative_value() {
        let (mut app, comp_a) = init_app::<A, _>(1, load_confirmed_authoritative);

        let auth_hist = auth_history(1, comp_a, [a(5)]);
        let confirm = confirm_history([1]); // The target tick is confirmed
        let e1 = app
            .world_mut()
            .spawn((Predicted, auth_hist, confirm, A(1)))
            .id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
    }

    #[test]
    fn load_confirmed_confirmed_gap() {
        let (mut app, comp_a) = init_app::<A, _>(1, load_confirmed_authoritative);

        let auth_hist = auth_history(0, comp_a, [a(5)]);
        let confirm = confirm_history([1]); // The target tick is confirmed
        let e1 = app
            .world_mut()
            .spawn((Predicted, auth_hist, confirm, A(1)))
            .id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
    }

    #[test]
    fn load_globally_confirmed_confirmed_gap() {
        let (mut app, comp_a) = init_app::<A, _>(1, load_confirmed_authoritative);

        let auth_hist = auth_history(0, comp_a, [a(5)]);
        let confirm = confirm_history([]); // No ticks are confirmed on the entity
        let e1 = app
            .world_mut()
            .spawn((Predicted, auth_hist, confirm, A(1)))
            .id();

        // The gap is confirmed globally
        app.world_mut()
            .resource_mut::<ServerMutateTicks>()
            .confirm(r_tick(1), 1);

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
    }

    #[test]
    fn load_confirmed_skips_unconfirmed() {
        let (mut app, comp_a) = init_app::<A, _>(1, load_confirmed_authoritative);

        let auth_hist = auth_history(0, comp_a, [a(5)]);
        let confirm = confirm_history([]); // Nothing is confirmed
        let e1 = app
            .world_mut()
            .spawn((Predicted, auth_hist, confirm, A(1)))
            .id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(1)), e.get::<A>());
    }

    #[test]
    fn reinsert_predicted() {
        let (mut app, comp_a) = init_app::<A, _>(0, super::reinsert_predicted);

        let pred_hist = pred_history(0, comp_a, [a(5)]);
        let e1 = app.world_mut().spawn((Predicted, pred_hist)).id();

        app.update();

        let e = app.world().entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
    }

    // TODO: This behavior is temporarily disabled, we need a better version of it
    //       that isn't as incompatible with required components
    // #[test]
    // fn reinsert_predicted_skips_authoritative_components() {
    //     let (mut app, comp_a) = init_app::<A, _>(0, super::reinsert_predicted);

    //     let comp_b = app.world_mut().register_component::<B>();

    //     app.world_mut()
    //         .resource_scope::<RollbackRegistry, _>(|world, mut registry| {
    //             registry.register::<B>(world)
    //         });

    //     let mut pred_hist = pred_history(0, comp_a, [a(5)]);
    //     pred_hist.insert(comp_b, comp_history(0, [b()]));

    //     let auth_hist = auth_history::<A>(0, comp_a, []);

    //     let e1 = app
    //         .world_mut()
    //         .spawn((Predicted, pred_hist, auth_hist))
    //         .id();

    //     app.update();

    //     let e = app.world().entity(e1);
    //     assert_eq!(None, e.get::<A>());
    //     assert_eq!(Some(&B), e.get::<B>());
    // }

    // TODO: Test command order, commands from loading should apply AFTER inserts/removes
}
