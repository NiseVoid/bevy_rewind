use crate::{
    EntityManagementCommands, EntityManagementEntityWorldMut, EntityManagementWorld, SpawnPlugin,
    SpawnReason, Spawned, SpawnedEntities, SpawnedEntity,
};

use std::marker::PhantomData;

use bevy::{
    ecs::{component::HookContext, entity_disabling::Disabled, world::DeferredWorld},
    prelude::*,
};
use bevy_replicon::{
    prelude::RepliconClient,
    shared::{
        replication::replication_registry::{ctx::DespawnCtx, ReplicationRegistry},
        replicon_tick::RepliconTick,
    },
};
use bevy_rewind::{
    Predicted, RollbackFrames, RollbackSchedule, RollbackStoreSet, StoreScheduleLabel, TickSource,
};

/// A plugin adding rollback-friendly entity management to the app.
pub struct EntityManagementPlugin<Tick: TickSource>(PhantomData<Tick>);

impl<Tick: TickSource> Default for EntityManagementPlugin<Tick> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Tick: TickSource> EntityManagementPlugin<Tick> {
    /// Construct the `EntityManagementPlugin`
    pub fn new() -> Self {
        Self(PhantomData)
    }
}

impl<Tick: TickSource> Plugin for EntityManagementPlugin<Tick> {
    fn build(&self, app: &mut App) {
        app.world_mut()
            .resource_mut::<ReplicationRegistry>()
            .despawn = replicon_despawn::<Tick>;

        let store_schedule = **app.world().resource::<StoreScheduleLabel>();
        app.add_systems(
            store_schedule,
            (
                disable_server_despawned_entities::<Tick>,
                despawn_unused_entities::<Tick>,
            )
                .before(RollbackStoreSet),
        )
        .insert_resource(HasEntityManagement)
        .insert_resource(GetTick(|world| (*world.resource::<Tick>()).into()))
        .insert_resource(GetTickDeferred(|world| (*world.resource::<Tick>()).into()))
        .add_systems(RollbackSchedule::BackToPresent, despawn_unspawned_entities);
    }
}

fn replicon_despawn<Tick: TickSource>(ctx: &DespawnCtx, mut entity: EntityWorldMut) {
    if entity.contains::<Predicted>() {
        if ctx.message_tick >= (*entity.world().resource::<Tick>()).into() {
            entity.insert((RemovedByServerAt(ctx.message_tick), Despawned));
        } else {
            entity.insert(RemovedByServerAt(ctx.message_tick));
        }
        return;
    }

    entity.despawn();
}

#[derive(Component, Deref)]
struct RemovedByServerAt(RepliconTick);

fn disable_server_despawned_entities<Tick: TickSource>(
    mut commands: Commands,
    query: Query<(Entity, &RemovedByServerAt)>,
    tick: Res<Tick>,
) {
    let tick = (*tick).into();
    for (entity, at) in query.iter() {
        if **at > tick {
            commands.entity(entity).insert(Despawned);
        }
    }
}

#[derive(Resource)]
struct HasEntityManagement;

#[derive(Resource)]
struct GetTick(fn(&World) -> RepliconTick);

#[derive(Resource)]
struct GetTickDeferred(fn(&DeferredWorld) -> RepliconTick);

impl<Reason: SpawnReason> Plugin for SpawnPlugin<Reason> {
    fn build(&self, app: &mut App) {
        app.init_resource::<SpawnedEntities<Reason>>().add_systems(
            RollbackSchedule::BackToPresent,
            |world: &World| -> RepliconTick { world.resource::<GetTick>().0(world) }
                .pipe(clean_spawned_entities_system::<Reason>),
        );
    }
}

/// A marker for entities that should be despawned once they fall out of history
#[derive(Component)]
#[component(on_add=track_unused)]
#[component(on_remove=untrack_unused)]
#[require(Disabled, UnusedAt)]
pub struct Despawned;

fn track_unused(mut world: DeferredWorld, ctx: HookContext) {
    let get_tick = world.resource::<GetTickDeferred>();
    let tick = get_tick.0(&world);
    world.commands().entity(ctx.entity).insert(UnusedAt(tick));
}

fn untrack_unused(mut world: DeferredWorld, ctx: HookContext) {
    world.commands().entity(ctx.entity).remove::<UnusedAt>();
    reenable(world, ctx)
}

fn reenable(mut world: DeferredWorld, ctx: HookContext) {
    let despawned_id = world.component_id::<Despawned>().unwrap();
    let unspawned_id = world.component_id::<Unspawned>().unwrap();
    let entity = world.entity(ctx.entity);
    let removed_or_missing = |comp_id| ctx.component_id == comp_id || !entity.contains_id(comp_id);

    if !removed_or_missing(despawned_id) || !removed_or_missing(unspawned_id) {
        return;
    }

    world.commands().entity(ctx.entity).remove::<Disabled>();
}

/// A marker for entities that have been rolled back to before they spawned.
/// These entities are despawned if they aren't re-enabled before [`RollbackSchedule::BackToPresent`]
#[derive(Component)]
#[component(on_remove = reenable)]
#[require(Disabled)]
pub struct Unspawned;

/// A component tracking when an entity became unused, it will be despawned once this tick is
/// outside of the history range.
#[derive(Component, Deref, Default)]
struct UnusedAt(RepliconTick);

fn despawn_unused_entities<Tick: TickSource>(
    mut commands: Commands,
    query: Query<(Entity, &UnusedAt), (With<Disabled>, With<Despawned>)>,
    tick: Res<Tick>,
    frames: Res<RollbackFrames>,
) {
    for (entity, unused_at) in query.iter() {
        if **unused_at + (frames.history_size() as u32) < (*tick).into() {
            commands.entity(entity).despawn();
        }
    }
}

fn despawn_unspawned_entities(
    mut commands: Commands,
    query: Query<Entity, (With<Disabled>, With<Unspawned>)>,
) {
    for entity in query.iter() {
        commands.entity(entity).despawn();
    }
}

impl<Reason: SpawnReason> SpawnedEntities<Reason> {
    fn get(&self, reason: &Reason) -> Option<Entity> {
        self.0.get(reason).map(|e| e.id)
    }

    fn get_and_update(&mut self, reason: &Reason, tick: RepliconTick) -> Option<Entity> {
        self.0.get_mut(reason).map(|e| {
            e.last_spawned = RepliconTick::new(e.last_spawned.get().max(tick.get()));
            e.id
        })
    }

    fn insert(&mut self, reason: Reason, tick: RepliconTick, id: Entity) {
        self.0.insert(
            reason,
            SpawnedEntity {
                id,
                last_spawned: tick,
            },
        );
    }

    fn update(&mut self, reason: &Reason, tick: RepliconTick) {
        if let Some(e) = self.0.get_mut(reason) {
            e.last_spawned = RepliconTick::new(e.last_spawned.get().max(tick.get()));
        }
    }
}

#[derive(Component)]
struct Reuse;

fn clean_spawned_entities_system<Reason: SpawnReason>(
    In(tick): In<RepliconTick>,
    mut entities: ResMut<SpawnedEntities<Reason>>,
    frames: Res<bevy_rewind::RollbackFrames>,
    mut removed: RemovedComponents<Reuse>, // TODO: Switch to hook
) {
    let max_ticks = frames.history_size() as u32;

    let removed = removed
        .read()
        .collect::<bevy::platform::collections::HashSet<Entity>>();

    entities.0.retain(|_key, entity| {
        !removed.contains(&entity.id) && tick < entity.last_spawned + max_ticks
    });
}

impl EntityManagementCommands for Commands<'_, '_> {
    fn reuse_spawn<Reason: SpawnReason>(
        &mut self,
        spawned: &Spawned<Reason>,
        reason: Reason,
        bundle: impl Bundle,
    ) -> Entity {
        if spawned.client.as_ref().map(|c| c.is_connected()) == Some(true) {
            if let Some(entities) = spawned.entities.as_ref() {
                if let Some(entity) = entities.get(&reason) {
                    if let Ok(mut entity_cmd) = self.get_entity(entity) {
                        entity_cmd.commands().queue(UpdateSpawnedEntity(reason));
                        entity_cmd.insert(bundle).remove::<(Despawned, Unspawned)>();
                        return entity;
                    }
                    warn!("Failed to reuse {}, creating new entity", entity);
                }

                let new_entity = self.spawn((Reuse, bundle)).id();
                self.queue(InsertSpawnedEntity(reason, new_entity));
                return new_entity;
            }
        }
        self.spawn(bundle).id()
    }

    fn disable_or_despawn(&mut self, entity: Entity) {
        let Ok(mut ec) = self.get_entity(entity) else {
            return;
        };
        ec.queue(|entity: EntityWorldMut| entity.disable_or_despawn());
    }
}

impl EntityManagementEntityWorldMut for EntityWorldMut<'_> {
    fn disable_or_despawn(mut self) {
        if self
            .get_resource::<RepliconClient>()
            .map(|c| c.is_connected())
            == Some(true)
            && self.world().contains_resource::<HasEntityManagement>()
        {
            self.insert(Despawned);
            self.flush();
            return;
        }
        self.despawn();
    }
}

impl EntityManagementWorld for World {
    fn reuse_spawn<Reason: SpawnReason>(
        &mut self,
        reason: Reason,
        bundle: impl Bundle,
    ) -> EntityWorldMut {
        if self
            .get_resource::<RepliconClient>()
            .map(|c| c.is_connected())
            == Some(true)
        {
            let get_tick = self.resource::<GetTick>();
            let tick = get_tick.0(self);

            if let Some(mut entities) = self.get_resource_mut::<SpawnedEntities<Reason>>() {
                if let Some(entity) = entities.get_and_update(&reason, tick) {
                    if self.entities().contains(entity) {
                        let mut entity_mut = self.entity_mut(entity);
                        entity_mut.insert(bundle).remove::<(Despawned, Unspawned)>();
                        return entity_mut;
                    }
                    warn!("Failed to reuse {}, creating new entity", entity);
                }

                let new_entity = self.spawn((Reuse, bundle)).id();
                self.resource_mut::<SpawnedEntities<Reason>>()
                    .insert(reason, tick, new_entity);
                return self.entity_mut(new_entity);
            }
        }
        self.spawn(bundle)
    }

    fn disable_or_despawn(&mut self, entity: Entity) {
        if !self.entities().contains(entity) {
            return;
        }
        if self
            .get_resource::<RepliconClient>()
            .map(|c| c.is_connected())
            == Some(true)
            && self.contains_resource::<HasEntityManagement>()
        {
            let mut entity_mut = self.entity_mut(entity);
            entity_mut.insert(Despawned);
            self.flush();
            return;
        }
        self.despawn(entity);
    }
}

struct InsertSpawnedEntity<Reason: SpawnReason>(pub Reason, pub Entity);

impl<Reason: SpawnReason> Command for InsertSpawnedEntity<Reason> {
    fn apply(self, world: &mut World) {
        let get_tick = world.resource::<GetTick>();
        let tick = get_tick.0(world);

        let mut entities = world.resource_mut::<SpawnedEntities<Reason>>();
        #[cfg(debug_assertions)]
        if entities.0.contains_key(&self.0) {
            warn!("Duplicate insert for key: {:?}", self.0);
        }
        entities.insert(self.0, tick, self.1);
    }
}

struct UpdateSpawnedEntity<Reason: SpawnReason>(pub Reason);

impl<Reason: SpawnReason> Command for UpdateSpawnedEntity<Reason> {
    fn apply(self, world: &mut World) {
        let get_tick = world.resource::<GetTick>();
        let tick = get_tick.0(world);

        let mut entities = world.resource_mut::<SpawnedEntities<Reason>>();
        entities.update(&self.0, tick);
    }
}
