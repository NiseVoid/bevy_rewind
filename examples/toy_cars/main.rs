use std::marker::PhantomData;

use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use bevy_replicon::prelude::*;
use bevy_replicon_example_backend::*;
use bevy_rewind::*;
use bevy_rewind_entity_management::EntityManagementPlugin;

mod simulation;

mod tick;

mod avian;
mod connect;
mod gameplay;
mod input;

fn main() {
    App::new()
        .add_plugins((
            // Regular bevy plugins
            DefaultPlugins,
            // Replicon networking
            RepliconPlugins.set(ServerPlugin {
                tick_policy: TickPolicy::Manual,
                ..default()
            }),
            RepliconExampleClientPlugin,
            RepliconExampleServerPlugin,
            // Add our custom simulation schedules and tick
            simulation::simulation_plugin,
            tick::tick_plugin,
            // Setup crates from this repo with our tick type and simulation schedules
            RollbackPlugin::<tick::GameTick> {
                rollback_schedule: simulation::SimulationMain.intern(),
                store_schedule: simulation::SimulationLast.intern(),
                phantom: PhantomData,
            },
            EntityManagementPlugin::<tick::GameTick>::new(),
            // Add avian-related logic to the app
            avian::avian_plugin,
            // Add our game-specific logic
            input::game_input_plugin,
            gameplay::gameplay_plugin,
            // A plugin to manage hosting/joining and establishing a working connection
            connect::connect_plugin,
        ))
        .run();
}
