use super::component_history::ComponentHistory;
use crate::{Predicted, RollbackFrames};

use std::{fmt::Debug, mem::ManuallyDrop, num::NonZero};

use bevy::{
    ecs::component::{ComponentId, Mutable},
    platform_support::collections::HashMap,
    prelude::*,
};
use bevy_replicon::{
    bytes::Bytes,
    core::{
        replication::{
            deferred_entity::DeferredEntity,
            replication_registry::{
                ctx::{RemoveCtx, WriteCtx},
                rule_fns::RuleFns,
            },
        },
        replicon_tick::RepliconTick,
    },
};

pub struct AuthoriativeCleanupPlugin;

impl Plugin for AuthoriativeCleanupPlugin {
    fn build(&self, app: &mut App) {
        _ = app;
        // TODO: Implement cleanup to remove component histories that would entirely evaluate to Missing/Removed
        // TODO: Implement system to resize histories when RollbackFrames changes
    }
}

/// A component holding a history of authoritative (from the server) values
#[derive(Component, Deref, DerefMut, Default)]
pub struct AuthoritativeHistory {
    #[deref]
    components: HashMap<ComponentId, ComponentHistory>,
}

pub(crate) fn write_authoritative_history<
    T: Component<Mutability = Mutable> + Clone + PartialEq + Debug,
>(
    ctx: &mut WriteCtx,
    rule_fns: &RuleFns<T>,
    entity: &mut DeferredEntity,
    cursor: &mut Bytes,
) -> bevy_replicon::postcard::Result<()> {
    let value = rule_fns.deserialize(ctx, cursor)?;
    let frames = entity
        .world()
        .get_resource::<RollbackFrames>()
        .copied()
        .unwrap_or_default();

    write_history_internal(ctx.component_id, entity, ctx.message_tick, value, frames);

    Ok(())
}

fn write_history_internal<T: Component + Clone + PartialEq + Debug>(
    component_id: ComponentId,
    entity: &mut EntityMut,
    received_tick: RepliconTick,
    value: T,
    frames: RollbackFrames,
) {
    let Some(mut history) = entity.get_mut::<AuthoritativeHistory>() else {
        if !entity.contains::<Predicted>() {
            warn!(
                "Trying to write history to unpredicted entity {}",
                entity.id()
            );
            return;
        }
        warn!(
            "Predicted entity {} is missing AuthoritativeHistory",
            entity.id()
        );
        return;
    };

    let comp_hist = history.entry(component_id).or_insert_with(|| {
        ComponentHistory::from_type::<T>(NonZero::new(frames.history_size() as u8).unwrap())
    });

    // TODO: Figure out deduplication of values
    // SAFETY: We are writing to a history matching our ComponentId
    unsafe {
        comp_hist.write(received_tick.get(), |dst| {
            let value = ManuallyDrop::new(value);
            std::ptr::copy_nonoverlapping(
                (&value as *const ManuallyDrop<T>).cast(),
                dst.as_ptr(),
                size_of::<T>(),
            );
        });
    }
}

// TODO: Tests
pub fn remove_authoritative_history<T: Component + Debug>(
    ctx: &mut RemoveCtx,
    entity: &mut DeferredEntity,
) {
    remove_history_internal(ctx.component_id, ctx.message_tick, entity);
}

fn remove_history_internal(component_id: ComponentId, tick: RepliconTick, entity: &mut EntityMut) {
    let Some(mut history) = entity.get_mut::<AuthoritativeHistory>() else {
        warn!(
            "Trying to remove history for {:?} from entity without AuthoritativeHistory",
            component_id,
        );
        return;
    };
    let Some(comp_hist) = history.get_mut(&component_id) else {
        warn!(
            "Trying to remove history for {:?} from entity without a history for it",
            component_id,
        );
        return;
    };

    comp_hist.mark_removed(tick.get());
}

#[cfg(test)]
mod tests {
    use super::{
        super::{component_history::TickData, test_utils::*},
        write_history_internal, AuthoritativeHistory,
    };
    use crate::history::RollbackRegistry;
    use crate::RollbackFrames;
    use TickData::*;

    use bevy::prelude::*;

    #[test]
    fn write_changes() {
        let mut world = World::new();
        world.init_resource::<RollbackFrames>();
        let frames = world.resource::<RollbackFrames>().clone();

        let mut registry = RollbackRegistry::default();
        registry.register::<A>(&mut world);
        world.insert_resource(registry);
        let comp_a = world.register_component::<A>();

        let e1 = world.spawn(AuthoritativeHistory::default()).id();
        let e2 = world.spawn(AuthoritativeHistory::default()).id();

        // Write A(1) to e1 for tick 0
        let mut entity_mut = EntityMut::from(world.entity_mut(e1));
        write_history_internal::<A>(comp_a, &mut entity_mut, r_tick(0), A(1), frames);

        // Write A(5) to e2 for tick 1
        let mut entity_mut = EntityMut::from(world.entity_mut(e2));
        write_history_internal::<A>(comp_a, &mut entity_mut, r_tick(1), A(5), frames);

        // Write A(2) and A(3) to e1 for tick 1 and 3 respectively
        let mut entity_mut = EntityMut::from(world.entity_mut(e1));
        write_history_internal::<A>(comp_a, &mut entity_mut, r_tick(1), A(2), frames);
        write_history_internal::<A>(comp_a, &mut entity_mut, r_tick(3), A(3), frames);

        // Write A(7) to e2 for tick 2
        let mut entity_mut = EntityMut::from(world.entity_mut(e2));
        write_history_internal::<A>(comp_a, &mut entity_mut, r_tick(2), A(7), frames);

        use Missing as M;

        let e = world.entity(e1);
        let hist = e.get::<AuthoritativeHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        for (i, v) in [a(1), a(2), M, a(3), M].iter_enumerate() {
            assert_eq!(v, hist.get(&comp_a).unwrap().get(i as u32).deref().cloned());
        }

        let e = world.entity(e2);
        let hist = e.get::<AuthoritativeHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        for (i, v) in [M, a(5), a(7), M].iter_enumerate() {
            assert_eq!(v, hist.get(&comp_a).unwrap().get(i as u32).deref().cloned());
        }
    }

    // TODO: Figure out deduplication of values
    // #[test]
    // fn write_duplicate() {
    //     let mut world = World::new();
    //     world.init_resource::<RollbackFrames>();
    //     let mut registry = RollbackRegistry::default();
    //     registry.register::<A>(&mut world);
    //     world.insert_resource(registry);
    //     let e1 = world.spawn(AuthoritativeHistory::default()).id();

    //     // Write A(1) to e1 for tick 0
    //     let (mut commands, mut entity_mut) = commands_and_entity(&mut world, &mut queue, e1);
    //     write_history_internal::<A>(&mut commands, &mut entity_mut, r_tick(0), A(1));

    //     // Write A(1) to e1 for tick 2 and 4
    //     let (mut commands, mut entity_mut) = commands_and_entity(&mut world, &mut queue, e1);
    //     write_history_internal::<A>(&mut commands, &mut entity_mut, r_tick(2), A(1));
    //     write_history_internal::<A>(&mut commands, &mut entity_mut, r_tick(4), A(1));

    //     // Write A(1) to e1 for tick 3
    //     let (mut commands, mut entity_mut) = commands_and_entity(&mut world, &mut queue, e1);
    //     write_history_internal::<A>(&mut commands, &mut entity_mut, r_tick(3), A(1));

    //     use Missing as M;

    //     let e = world.entity(e1);
    //     let hist = e.get::<AuthoritativeHistory>().unwrap();
    //     assert!(hist.contains_key(&comp_a));
    //     for (i, v) in [a(1), M, M, M, M].iter_enumerate() {
    //         assert_eq!(v, hist.get(&comp_a).unwrap().get(i as u32).deref().cloned());
    //     }
    // }

    #[test]
    fn write_out_of_order() {
        let mut world = World::new();
        world.init_resource::<RollbackFrames>();
        let frames = world.resource::<RollbackFrames>().clone();

        let mut registry = RollbackRegistry::default();
        registry.register::<A>(&mut world);
        world.insert_resource(registry);
        let comp_a = world.register_component::<A>();

        let e1 = world.spawn(AuthoritativeHistory::default()).id();

        // Write A(2) to e1 for tick 1
        let mut entity_mut = EntityMut::from(world.entity_mut(e1));
        write_history_internal::<A>(comp_a, &mut entity_mut, r_tick(1), A(2), frames);

        // Write A(4) and A(1) to e1 for tick 3 and 0 respectively
        write_history_internal::<A>(comp_a, &mut entity_mut, r_tick(3), A(4), frames);
        write_history_internal::<A>(comp_a, &mut entity_mut, r_tick(0), A(1), frames);

        // Write A(3) to e1 for tick 2
        write_history_internal::<A>(comp_a, &mut entity_mut, r_tick(2), A(3), frames);

        use Missing as M;

        let e = world.entity(e1);
        let hist = e.get::<AuthoritativeHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        for (i, v) in [a(1), a(2), a(3), a(4), M].iter_enumerate() {
            assert_eq!(v, hist.get(&comp_a).unwrap().get(i as u32).deref().cloned());
        }
    }

    #[test]
    fn multiple_adds() {
        let mut world = World::new();
        world.init_resource::<RollbackFrames>();
        let frames = world.resource::<RollbackFrames>().clone();

        let mut registry = RollbackRegistry::default();
        registry.register::<A>(&mut world);
        world.insert_resource(registry);
        let comp_a = world.register_component::<A>();

        let e1 = world.spawn(AuthoritativeHistory::default()).id();

        // Write A(1), A(2), and A(3) to e1 for tick 0, 1 and 3 respectively
        let mut entity_mut = EntityMut::from(world.entity_mut(e1));
        write_history_internal::<A>(comp_a, &mut entity_mut, r_tick(0), A(1), frames);
        write_history_internal::<A>(comp_a, &mut entity_mut, r_tick(1), A(2), frames);
        write_history_internal::<A>(comp_a, &mut entity_mut, r_tick(2), A(3), frames);

        use Missing as M;

        let e = world.entity(e1);
        let hist = e.get::<AuthoritativeHistory>().unwrap();
        assert!(hist.contains_key(&comp_a));
        for (i, v) in [a(1), a(2), a(3), M].iter_enumerate() {
            assert_eq!(v, hist.get(&comp_a).unwrap().get(i as u32).deref().cloned());
        }
    }

    #[test]
    fn drop_once_on_success() {
        let mut world = World::new();
        world.init_resource::<RollbackFrames>();
        let frames = world.resource::<RollbackFrames>().clone();

        let mut registry = RollbackRegistry::default();
        registry.register::<D>(&mut world);
        world.insert_resource(registry);
        let comp_d = world.register_component::<D>();

        let e1 = world.spawn(AuthoritativeHistory::default()).id();

        let drops = DropList::default();

        // Write D(1) to e1 for tick 0
        let mut entity = EntityMut::from(world.entity_mut(e1));
        write_history_internal(comp_d, &mut entity, r_tick(0), D::new(1, &drops), frames);
        write_history_internal(comp_d, &mut entity, r_tick(1), D::new(2, &drops), frames);
        write_history_internal(comp_d, &mut entity, r_tick(2), D::new(3, &drops), frames);
        write_history_internal(comp_d, &mut entity, r_tick(3), D::new(4, &drops), frames);
        write_history_internal(comp_d, &mut entity, r_tick(4), D::new(5, &drops), frames);

        assert_drops(&drops, []);

        world.despawn(e1);

        assert_drops(&drops, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn drop_once_on_fail() {
        let mut world = World::new();
        world.init_resource::<RollbackFrames>();
        let frames = world.resource::<RollbackFrames>().clone();

        let mut registry = RollbackRegistry::default();
        registry.register::<D>(&mut world);
        world.insert_resource(registry);
        let comp_d = world.register_component::<D>();

        let e1 = world.spawn(AuthoritativeHistory::default()).id();

        let drops = DropList::default();

        // Write D(1) to e1 for tick 0
        let mut entity = EntityMut::from(world.entity_mut(e1));
        write_history_internal(comp_d, &mut entity, r_tick(10), D::new(1, &drops), frames);
        write_history_internal(comp_d, &mut entity, r_tick(10), D::new(2, &drops), frames);

        assert_drops(&drops, [1]);

        let mut entity = EntityMut::from(world.entity_mut(e1));
        write_history_internal(comp_d, &mut entity, r_tick(1), D::new(3, &drops), frames);
        write_history_internal(comp_d, &mut entity, r_tick(2), D::new(4, &drops), frames);
        write_history_internal(comp_d, &mut entity, r_tick(3), D::new(5, &drops), frames);

        assert_drops(&drops, [1, 3, 4, 5]);

        world.despawn(e1);

        assert_drops(&drops, [1, 3, 4, 5, 2]);
    }
}
