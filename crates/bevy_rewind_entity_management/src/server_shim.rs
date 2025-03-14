use crate::{EntityManagementCommands, EntityManagementWorld, SpawnPlugin, SpawnReason, Spawned};

use std::marker::PhantomData;

use bevy::prelude::*;

/// A plugin adding rollback-friendly entity management to the app.
pub struct EntityManagementPlugin<Tick: Sync + Send + 'static>(PhantomData<Tick>);

impl<Tick: Sync + Send + 'static> EntityManagementPlugin<Tick> {
    /// Construct the EntityManagementPlugin
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<Tick: Sync + Send + 'static> Plugin for EntityManagementPlugin<Tick> {
    fn build(&self, _: &mut App) {}
}

impl<Reason: SpawnReason> Plugin for SpawnPlugin<Reason> {
    fn build(&self, _: &mut App) {}
}

impl EntityManagementCommands for Commands<'_, '_> {
    fn reuse_spawn<Reason: SpawnReason>(
        &mut self,
        _: &Spawned<Reason>,
        _: Reason,
        bundle: impl Bundle,
    ) -> Entity {
        self.spawn(bundle).id()
    }

    fn disable_or_despawn(&mut self, entity: Entity) {
        self.entity(entity).despawn();
    }
}

impl EntityManagementWorld for World {
    fn reuse_spawn<Reason: SpawnReason>(
        &mut self,
        _: Reason,
        bundle: impl Bundle,
    ) -> EntityWorldMut {
        self.spawn(bundle)
    }

    fn disable_or_despawn(&mut self, entity: Entity) {
        self.despawn(entity);
    }
}
