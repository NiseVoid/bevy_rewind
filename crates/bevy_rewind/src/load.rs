use crate::{LoadFrom, ResourceHistory, TickData};

use std::fmt::Debug;

use bevy::prelude::*;

pub(super) fn load_and_clear_resource_prediction<T: Resource + Clone + Debug + PartialEq>(
    mut commands: Commands,
    t: Option<ResMut<T>>,
    mut hist: ResMut<ResourceHistory<T>>,
    previous_tick: Res<LoadFrom>,
) {
    match hist.get(**previous_tick) {
        TickData::Value(value) => {
            if let Some(mut t) = t {
                if *t != *value {
                    *t = value.clone();
                }
            } else {
                commands.insert_resource(value.clone());
            }
        }
        TickData::Removed => {
            commands.remove_resource::<T>();
        }
        TickData::Missing => {
            commands.remove_resource::<T>();
            hist.keep_one();
            return;
        }
    }
    hist.clean(**previous_tick);
}

pub(super) fn reinsert_predicted_resource<T: Resource + Clone>(
    mut commands: Commands,
    t: Option<Res<T>>,
    history: ResMut<ResourceHistory<T>>,
    previous_tick: Res<LoadFrom>,
) {
    if t.is_some() {
        return;
    }

    if let TickData::Value(v) = history.get(**previous_tick) {
        commands.insert_resource(v.clone());
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;

    use bevy::ecs::system::RunSystemOnce;
    use bevy_replicon::core::replicon_tick::RepliconTick;

    #[derive(Resource, Clone, Copy, Deref, DerefMut, PartialEq, Eq, Debug)]
    struct A(u8);
    fn a(v: u8) -> TickData<A> {
        TickData::Value(A(v))
    }

    #[test]
    fn load_value() {
        let mut world = World::new();
        let predicted = ResourceHistory::<A>::from_list(1, [a(1), a(2), a(3)]);
        world.insert_resource(A(0));
        world.insert_resource(predicted);

        world.insert_resource(LoadFrom(RepliconTick::new(2)));
        world
            .run_system_once(load_and_clear_resource_prediction::<A>)
            .unwrap();

        assert_eq!(&A(2), world.resource::<A>());
    }

    #[test]
    fn remove_and_insert() {
        let mut world = World::new();
        let predicted = ResourceHistory::<A>::from_list(1, [a(1), TickData::Removed, a(3)]);
        world.insert_resource(A(0));
        world.insert_resource(predicted.clone());

        world.insert_resource(LoadFrom(RepliconTick::new(2)));
        world
            .run_system_once(load_and_clear_resource_prediction::<A>)
            .unwrap();

        // The data for tick 2 marked it as removed, so it should not be in the world
        assert_eq!(None, world.get_resource::<A>());

        world.insert_resource(LoadFrom(RepliconTick::new(3)));
        world.insert_resource(predicted); // Reset history since it got cleared
        world
            .run_system_once(load_and_clear_resource_prediction::<A>)
            .unwrap();

        // It's back for tick 3, so it should be inserted again
        assert_eq!(Some(&A(3)), world.get_resource::<A>());
    }

    #[test]
    fn remove_before_history_and_reinsert() {
        let mut world = World::new();
        let predicted = ResourceHistory::<A>::from_list(2, [a(1), a(2)]);
        world.insert_resource(A(0));
        world.insert_resource(predicted);

        world.insert_resource(LoadFrom(RepliconTick::new(0)));
        world
            .run_system_once(load_and_clear_resource_prediction::<A>)
            .unwrap();

        // We are loading data from before the history, so the component should be removed
        assert_eq!(None, world.get_resource::<A>());

        let hist = world.resource::<ResourceHistory<A>>();
        assert_eq!(1, hist.len());

        // The value isn't reinserted on a next frame that is outside of history
        world.insert_resource(LoadFrom(RepliconTick::new(1)));
        world
            .run_system_once(reinsert_predicted_resource::<A>)
            .unwrap();
        assert_eq!(None, world.get_resource::<A>());

        // When we get to the first value, we reinsert the component
        world.insert_resource(LoadFrom(RepliconTick::new(2)));
        world
            .run_system_once(reinsert_predicted_resource::<A>)
            .unwrap();
        assert_eq!(Some(&A(1)), world.get_resource::<A>());
    }
}
