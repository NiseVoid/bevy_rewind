use bevy::{ecs::schedule::ScheduleLabel, prelude::*};

pub fn simulation_plugin(app: &mut App) {
    app.init_schedule(SimulationMain)
        .init_schedule(SimulationPreUpdate)
        .init_schedule(SimulationUpdate)
        .init_schedule(SimulationPostUpdate)
        .init_schedule(SimulationLast)
        .add_systems(SimulationMain, run_simulation)
        .add_systems(FixedUpdate, run_simulation_main);
}

/// A schedule that runs the game's simulation
#[derive(ScheduleLabel, Clone, PartialEq, Eq, Debug, Hash)]
pub struct SimulationMain;

/// A schedule that runs immediately before the game's simulation
#[derive(ScheduleLabel, Clone, PartialEq, Eq, Debug, Hash)]
pub struct SimulationPreUpdate;

/// The schedule for the game's simulation
#[derive(ScheduleLabel, Clone, PartialEq, Eq, Debug, Hash)]
pub struct SimulationUpdate;

/// A schedule that runs immediately after that game's simulation
#[derive(ScheduleLabel, Clone, PartialEq, Eq, Debug, Hash)]
pub struct SimulationPostUpdate;

/// A schedule that runs at the end of the simulation, after [`SimulationPostUpdate`]
#[derive(ScheduleLabel, Clone, PartialEq, Eq, Debug, Hash)]
pub struct SimulationLast;

fn run_simulation(world: &mut World) {
    world.run_schedule(SimulationPreUpdate);
    world.run_schedule(SimulationUpdate);
    world.run_schedule(SimulationPostUpdate);
    world.run_schedule(SimulationLast);
}

fn run_simulation_main(world: &mut World) {
    world.run_schedule(SimulationMain);
}
