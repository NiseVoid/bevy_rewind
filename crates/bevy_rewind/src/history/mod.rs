// Data types
mod blob_deque;
mod sparse_blob_deque;

// Shared history types
mod component;
pub use component::{ExistingOrUninit, LoadFn};
mod component_history;

// Specific history types
mod authoritative;
pub use authoritative::AuthoritativeHistory;
mod predicted;
pub use predicted::PredictedHistory;

mod batch;
mod load;

#[cfg(test)]
mod test_utils;

use bevy::{ecs::component::ComponentId, platform::collections::HashMap, prelude::*};
use component::HistoryComponent;

// TODO: Add some extra safeguards to check types and reduce places to duplicate them

pub struct HistoryPlugin;

impl Plugin for HistoryPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            load::HistoryLoadPlugin,
            predicted::PredictionStorePlugin,
            authoritative::AuthoriativeCleanupPlugin,
        ));
    }
}

#[allow(unused)]
pub(crate) use authoritative::{remove_authoritative_history, write_authoritative_history};

#[derive(Resource, Default)]
pub struct RollbackRegistry {
    pub ids: HashMap<ComponentId, usize>,
    pub components: Vec<HistoryComponent>,
}

impl RollbackRegistry {
    pub fn register<T: Component + Clone + PartialEq>(&mut self, world: &mut World) {
        let id = world.register_component::<T>();
        self.ids.insert(id, self.components.len());
        self.components.push(HistoryComponent::new::<T>());
    }

    pub fn register_with_load<T: Component + Clone + PartialEq>(
        &mut self,
        world: &mut World,
        load_fn: LoadFn<T>,
    ) {
        let id = world.register_component::<T>();
        self.ids.insert(id, self.components.len());
        self.components
            .push(HistoryComponent::with_load::<T>(load_fn));
    }
}
