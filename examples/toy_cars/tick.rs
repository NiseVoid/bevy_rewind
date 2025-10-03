use crate::connect::ConnectionState;

use bevy::prelude::*;
use bevy_replicon::shared::replicon_tick::RepliconTick;
use serde::{Deserialize, Serialize};

pub fn tick_plugin(app: &mut App) {
    app.init_resource::<GameTick>().add_systems(
        FixedPreUpdate,
        increment_tick.run_if(not(in_state(ConnectionState::Menu))),
    );
}

#[derive(Resource, Clone, Copy, Serialize, Deserialize, Default, Deref, DerefMut)]
pub struct GameTick(u32);

impl From<RepliconTick> for GameTick {
    fn from(value: RepliconTick) -> Self {
        Self(value.get())
    }
}

impl From<GameTick> for RepliconTick {
    fn from(value: GameTick) -> Self {
        Self::new(value.0)
    }
}

fn increment_tick(mut tick: ResMut<GameTick>) {
    **tick += 1;
}
