use crate::simulation::*;

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy_replicon::prelude::*;
use bevy_rewind::*;

pub fn avian_plugin(app: &mut App) {
    app.add_plugins(
        PhysicsPlugins::new(SimulationPostUpdate)
            .build()
            .disable::<SleepingPlugin>()
            .disable::<SyncPlugin>(),
    )
    .replicate::<Position>()
    .replicate::<Rotation>()
    .replicate::<LinearVelocity>()
    .replicate::<AngularVelocity>()
    // Set up rollback on avian components/resources
    .register_authoritative_component::<Position>()
    .register_authoritative_component::<Rotation>()
    .register_authoritative_component::<LinearVelocity>()
    .register_authoritative_component::<AngularVelocity>()
    .register_predicted_resource::<Collisions>()
    .add_systems(
        bevy::app::RunFixedMainLoop,
        (
            avian3d::sync::position_to_transform,
            non_body_position_to_transform,
        )
            .in_set(bevy::app::RunFixedMainLoopSystem::AfterFixedMainLoop),
    );
}

fn non_body_position_to_transform(
    mut query: Query<
        (&mut Transform, &Position, &Rotation),
        (
            Without<RigidBody>,
            Or<(Added<Transform>, Changed<Position>, Changed<Rotation>)>,
        ),
    >,
) {
    for (mut transform, pos, rot) in query.iter_mut() {
        transform.translation = **pos;
        transform.rotation = **rot;
    }
}
