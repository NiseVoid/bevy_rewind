use crate::{
    connect::ConnectionState, gameplay::OurCar, simulation::SimulationPreUpdate, tick::GameTick,
};

use avian3d::prelude::Rotation;
use bevy::{ecs::entity::MapEntities, prelude::*};
use bevy_rewind_input::*;

pub fn game_input_plugin(app: &mut App) {
    app.add_plugins(InputQueuePlugin::<GameInput, GameTick>::new(
        SimulationPreUpdate,
    ))
    .add_systems(
        FixedPreUpdate,
        generate_inputs
            .ignore_param_missing()
            .run_if(in_state(ConnectionState::InGame)),
    );
}

#[derive(Component, TypePath, Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
#[require(InputHistory::<GameInput>)]
pub struct GameInput {
    pub direction: Option<Dir2>,
}

impl MapEntities for GameInput {
    fn map_entities<M: EntityMapper>(&mut self, _: &mut M) {}
}

impl InputTrait for GameInput {
    fn repeats() -> bool {
        true
    }
}

fn generate_inputs(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut car: Single<(&mut GameInput, &Rotation), With<OurCar>>,
) {
    let rot = car.1.to_euler(EulerRot::YXZ).0;
    let rot = Rot2::radians(-rot);

    car.0.direction = Dir2::new(
        rot * Vec2::new(
            (keyboard_input.pressed(KeyCode::KeyD) as i32
                - keyboard_input.pressed(KeyCode::KeyA) as i32) as f32,
            -(keyboard_input.pressed(KeyCode::KeyW) as i32) as f32,
        ),
    )
    .ok();
}
