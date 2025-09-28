//! A crate handling entity management in a way that plays nice with rollback.
//! Based on features can also serve as a server shim.

// TODO: Tests

#[cfg(feature = "client")]
mod client;

#[cfg(feature = "client")]
pub use client::{Despawned, EntityManagementPlugin, Unspawned};

#[cfg(not(feature = "client"))]
mod server_shim;
#[cfg(not(feature = "client"))]
pub use server_shim::EntityManagementPlugin;

use std::marker::PhantomData;

use bevy::{
    ecs::system::SystemParam,
    platform::collections::{HashMap, HashSet},
    prelude::*,
};
use bevy_replicon::shared::replicon_tick::RepliconTick;

/// A plugin adding handling of entity reuse for a specific [`SpawnReason`]
pub struct SpawnPlugin<Reason: SpawnReason>(PhantomData<Reason>);

impl<Reason: SpawnReason> Default for SpawnPlugin<Reason> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Reason: SpawnReason> SpawnPlugin<Reason> {
    /// Construct a `SpawnPlugin` for the specified [`SpawnReason`]
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

#[derive(Resource, Deref, DerefMut, Default)]
struct ToRemove(HashSet<Entity>);

/// A system param used to track spawned entities
#[derive(SystemParam)]
#[cfg_attr(not(feature = "client"), allow(unused))]
pub struct Spawned<'w, Reason: SpawnReason> {
    entities: ResMut<'w, SpawnedEntities<Reason>>,
    to_remove: Res<'w, ToRemove>,
    #[cfg(feature = "client")]
    authority: Option<Res<'w, client::HasAuthority>>,
}

#[derive(Debug)]
#[cfg_attr(not(feature = "client"), allow(unused))]
struct SpawnedEntity {
    id: Entity,
    last_spawned: RepliconTick,
}

#[derive(Resource, Debug)]
#[cfg_attr(not(feature = "client"), allow(unused))]
struct SpawnedEntities<Reason: SpawnReason>(HashMap<Reason, SpawnedEntity>);

impl<Reason: SpawnReason> Default for SpawnedEntities<Reason> {
    fn default() -> Self {
        Self(HashMap::default())
    }
}

/// A trait for spawn reasons, which are used to reuse entities during rollback
pub trait SpawnReason:
    PartialEq + Eq + std::hash::Hash + std::fmt::Debug + Sync + Send + 'static
{
    /// Get the tick for this spawn reason
    fn tick(&self) -> RepliconTick;
}

/// An extension trait for [`Commands`] for rollback-friendly entity management
pub trait EntityManagementCommands {
    /// Spawn an entity, reusing entities on client if matching
    fn reuse_spawn<Reason: SpawnReason>(
        &mut self,
        spawned: &Spawned<Reason>,
        reason: Reason,
        bundle: impl Bundle,
    ) -> Entity;

    /// Register an entity, causing later spawns to reuse this entity
    fn register_reuse<Reason: SpawnReason>(
        &mut self,
        spawned: &Spawned<Reason>,
        reason: Reason,
        entity: Entity,
    );

    /// Disable an entity if doing rollback, otherwise despawn it
    fn disable_or_despawn(&mut self, entity: Entity);
}

/// An extension trait for [`EntityWorldMut`] for rollback-friendly entity management
pub trait EntityManagementEntityWorldMut {
    /// Disable an entity if doing rollback, otherwise despawn it
    fn disable_or_despawn(self);
}

/// An extension trait for [`World`] for rollback-friendly entity management
pub trait EntityManagementWorld {
    /// Spawn an entity, reusing entities on client if matching
    fn reuse_spawn<'a, Reason: SpawnReason>(
        &'a mut self,
        spawn: Reason,
        bundle: impl Bundle,
    ) -> EntityWorldMut<'a>;

    /// Register an entity, causing later spawns to reuse this entity
    fn register_reuse<Reason: SpawnReason>(&mut self, reason: Reason, entity: Entity);

    /// Disable an entity if doing rollback, otherwise despawn it
    fn disable_or_despawn(&mut self, entity: Entity);
}

/// An extension trait for [`DeferredWorld`](bevy::ecs::world::DeferredWorld) for rollback-friendly
/// entity management
pub trait EntityManagementDeferredWorld {
    /// Register an entity, causing later spawns to reuse this entity
    fn register_reuse<Reason: SpawnReason>(&mut self, reason: Reason, entity: Entity);
}
