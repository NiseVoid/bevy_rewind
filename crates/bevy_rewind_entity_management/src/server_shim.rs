use crate::{
    EntityManagementCommands, EntityManagementDeferredWorld, EntityManagementWorld, SpawnPlugin,
    SpawnReason, Spawned, SpawnedEntities, ToRemove,
};

use std::marker::PhantomData;

use bevy::{ecs::world::DeferredWorld, prelude::*};
use bevy_replicon::prelude::Signature;

/// A plugin adding rollback-friendly entity management to the app.
pub struct EntityManagementPlugin<Tick: Sync + Send + 'static>(PhantomData<Tick>);

impl<Tick: Sync + Send + 'static> EntityManagementPlugin<Tick> {
    /// Construct the EntityManagementPlugin
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<Tick: Sync + Send + 'static> Plugin for EntityManagementPlugin<Tick> {
    fn build(&self, app: &mut App) {
        app.init_resource::<ToRemove>();
    }
}

impl<Reason: SpawnReason> Plugin for SpawnPlugin<Reason> {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpawnedEntities<Reason>>();
    }
}

impl EntityManagementCommands for Commands<'_, '_> {
    fn reuse_spawn<Reason: SpawnReason>(
        &mut self,
        _: &Spawned<Reason>,
        reason: Reason,
        bundle: impl Bundle,
    ) -> Entity {
        self.spawn((bundle, Signature::from(&reason))).id()
    }

    fn register_reuse<Reason: SpawnReason>(&mut self, _: &Spawned<Reason>, _: Reason, _: Entity) {}

    fn disable_or_despawn(&mut self, entity: Entity) {
        self.entity(entity).despawn();
    }
}

impl EntityManagementWorld for World {
    fn reuse_spawn<'a, Reason: SpawnReason>(
        &'a mut self,
        reason: Reason,
        bundle: impl Bundle,
    ) -> EntityWorldMut<'a> {
        self.spawn((bundle, Signature::from(&reason)))
    }

    fn register_reuse<Reason: SpawnReason>(&mut self, _: Reason, _: Entity) {}

    fn disable_or_despawn(&mut self, entity: Entity) {
        self.despawn(entity);
    }
}

impl EntityManagementDeferredWorld for DeferredWorld<'_> {
    fn register_reuse<Reason: SpawnReason>(&mut self, _: Reason, _: Entity) {}
}
