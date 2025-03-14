//! A crate adding input queue and history logic to `bevy_replicon` apps.

use arrayvec::ArrayVec;

#[cfg(feature = "server")]
mod queue;
#[cfg(feature = "server")]
pub use queue::InputQueue;

mod history;
pub use history::InputHistory;

#[cfg(feature = "client")]
mod client;
#[cfg(feature = "client")]
pub use client::InputAuthority;
#[cfg(feature = "server")]
mod server;
#[cfg(feature = "server")]
pub use server::InputTarget;

use bevy::{
    ecs::{component::Mutable, entity::MapEntities, intern::Interned, schedule::ScheduleLabel},
    prelude::*,
    reflect::TypePath,
};
use bevy_replicon::{core::replicon_tick::RepliconTick, prelude::*};
use serde::{Deserialize, Serialize};

/// The source of the current simulation tick
pub trait TickSource: Resource + Copy + From<RepliconTick> + Into<RepliconTick> {}

impl<T> TickSource for T where T: Resource + Copy + From<RepliconTick> + Into<RepliconTick> {}

/// A plugin adding input queue logic to an app
pub struct InputQueuePlugin<T: InputTrait, Tick: TickSource> {
    #[cfg_attr(not(any(feature = "client", feature = "server")), allow(dead_code))]
    schedule: Interned<dyn ScheduleLabel>,
    phantom: std::marker::PhantomData<(T, Tick)>,
}

impl<T: InputTrait, Tick: TickSource> InputQueuePlugin<T, Tick> {
    /// Construct an `InputQueuePlugin` from the schedule inputs should be loaded in
    pub fn new(schedule: impl ScheduleLabel) -> Self {
        Self {
            schedule: schedule.intern(),
            phantom: std::marker::PhantomData::<(T, Tick)>,
        }
    }
}

impl<T: InputTrait, Tick: TickSource> Plugin for InputQueuePlugin<T, Tick> {
    fn build(&self, app: &mut App) {
        app.add_mapped_client_event::<InputHistory<T>>(ChannelKind::Unreliable)
            .add_mapped_server_event::<HistoryFor<T>>(ChannelKind::Unreliable);

        #[cfg(feature = "client")]
        app.add_plugins(client::InputQueueClientPlugin::<T, Tick>::new(
            self.schedule,
        ));

        #[cfg(feature = "server")]
        app.add_plugins(server::InputQueueServerPlugin::<T, Tick>::new(
            self.schedule,
        ));
    }
}

/// A collection of system sets for input queue logic
#[derive(SystemSet, Clone, PartialEq, Eq, Debug, Hash)]
pub enum InputQueueSet {
    /// A set with systems receiving/sending data.
    /// Receiving happens in `PreUpdate`, sending in `PostUpdate`
    Network,
    /// A set that loads inputs to the T registered on [`InputQueuePlugin`]
    Load,
    /// A set that stores and clears inputs
    Clean,
}

/// A trait for an Input that can be registered via [`InputQueuePlugin`]
pub trait InputTrait:
    Component<Mutability = Mutable>
    + Sync
    + Send
    + 'static
    + Clone
    + std::fmt::Debug
    + MapEntities
    + Serialize
    + for<'a> Deserialize<'a>
    + TypePath
    + Default
{
    /// Whether or not the input repeats
    fn repeats() -> bool;

    /// Get a repeated copy of this input
    fn repeated(&self, since: u32) -> Option<Self> {
        if !Self::repeats() || since > 5 {
            return None;
        }
        Some(self.clone())
    }
}

#[derive(Event, Clone, TypePath, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(bound(deserialize = "T: for<'de2> serde::Deserialize<'de2>"))]
struct HistoryFor<T: InputTrait> {
    entity: Entity,
    tick: RepliconTick,
    past: ArrayVec<(u8, T), 3>,
    future: ArrayVec<(u8, T), 7>,
}

impl<T: InputTrait> MapEntities for HistoryFor<T> {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        self.entity = mapper.get_mapped(self.entity);
        self.past
            .iter_mut()
            .for_each(|(_, t)| t.map_entities(mapper));
        self.future
            .iter_mut()
            .for_each(|(_, t)| t.map_entities(mapper));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub use crate::history::tests::hist;

    #[derive(Resource, Clone, Copy, Deref, DerefMut, PartialEq, Eq, Debug, Default)]
    pub struct Tick(pub u32);

    impl From<RepliconTick> for Tick {
        fn from(value: RepliconTick) -> Self {
            Self(value.get())
        }
    }

    impl From<Tick> for RepliconTick {
        fn from(value: Tick) -> Self {
            RepliconTick::new(value.0)
        }
    }

    #[derive(Component, Clone, Default, Serialize, Deserialize, Debug, PartialEq, TypePath)]
    pub struct A(pub u8);

    impl InputTrait for A {
        fn repeats() -> bool {
            true
        }
    }

    impl MapEntities for A {
        fn map_entities<M: EntityMapper>(&mut self, _: &mut M) {}
    }
}
