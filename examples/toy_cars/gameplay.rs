use avian3d::prelude::*;
use bevy::{
    image::{ImageLoaderSettings, ImageSampler},
    prelude::*,
};
use bevy_replicon::prelude::*;
use bevy_rewind::Predicted;
use bevy_rewind_entity_management::*;
use bevy_rewind_input::*;
use serde::{Deserialize, Serialize};

use crate::{connect::ConnectionState, input::GameInput, simulation::*};

pub fn gameplay_plugin(app: &mut App) {
    app
        // Set up replication
        .replicate::<Car>()
        .replicate::<Ball>()
        .add_client_event::<LocalEntities>(RepliconChannel::from(ChannelKind::Unordered))
        .add_systems(
            OnEnter(ConnectionState::InGame),
            (
                setup_game,
                (
                    claim_car.run_if(client_connected),
                    add_replicated.run_if(server_running),
                ),
            )
                .chain(),
        )
        .add_systems(FixedPreUpdate, spawn_client_cars.run_if(server_running))
        .add_systems(
            SimulationUpdate,
            (move_cars, spawn_ball.run_if(server_running))
                .run_if(in_state(ConnectionState::InGame)),
        )
        .add_systems(
            SimulationPostUpdate,
            process_goals.ignore_param_missing().after(PhysicsSet::Sync),
        )
        .add_systems(
            Update,
            (
                add_car_models,
                add_ball_model,
                follow_car.ignore_param_missing(),
            )
                .run_if(in_state(ConnectionState::InGame)),
        );
}

#[derive(Component, Default, Serialize, Deserialize)]
#[require(GameInput, Predicted, RigidBody(|| RigidBody::Dynamic), Friction(|| Friction::ZERO), Collider(|| Collider::from(Cuboid::new(0.5, 0.35, 1.))), ColliderDensity(|| ColliderDensity(10.)))]
struct Car;

#[derive(Component)]
#[require(Car)]
pub struct OurCar;

#[derive(Component, Serialize, Deserialize)]
#[require(Predicted, RigidBody(|| RigidBody::Dynamic), Collider(|| Collider::from(Sphere::default())), ColliderDensity(|| ColliderDensity(0.1)))]
struct Ball;

#[derive(Component)]
#[require(Sensor, CollidingEntities)]
struct Goal;

fn setup_game(
    mut commands: Commands,
    loader: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    // Camera
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0., 1., 5.).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Light
    commands.spawn((
        DirectionalLight::default(),
        Transform::from_xyz(0., 100., 20.).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Player
    commands.spawn((Transform::from_xyz(0., (0.3 + 0.35) / 2., 0.), OurCar));

    // Arena
    let arena_size = 50.;
    let arena_mesh = Plane3d::new(Vec3::Y, Vec2::splat(arena_size));
    commands.spawn((
        Transform::from_xyz(0., -0.5, 0.),
        RigidBody::Static,
        Collider::from(Cuboid {
            half_size: Vec3::new(arena_size, 0.5, arena_size),
        }),
        Visibility::default(),
        children![(
            Transform::from_xyz(0., 0.5, 0.),
            Mesh3d(meshes.add(arena_mesh)),
            MeshMaterial3d(mats.add(StandardMaterial {
                base_color_texture: Some(
                    loader.load_with_settings("grid.png", |s: &mut ImageLoaderSettings| {
                        s.sampler = ImageSampler::nearest()
                    })
                ),
                ..default()
            })),
        )],
    ));

    // Walls
    let goal_size = 3.;
    let full_wall = Cuboid::new(1., 5., arena_size * 2. + 2.);

    for side in [-1., 1.] {
        commands.spawn((
            Transform::from_xyz(side * (arena_size + 0.5), 2.5, 0.),
            RigidBody::Static,
            Collider::from(full_wall),
            Mesh3d(meshes.add(full_wall)),
            MeshMaterial3d(mats.add(StandardMaterial::from_color(Color::srgb(0.2, 0.2, 0.2)))),
        ));
    }

    let half_wall = Cuboid::new(arena_size - goal_size, 5., 1.);
    for (side_x, side_y) in [(-1., -1.), (-1., 1.), (1., -1.), (1., 1.)] {
        commands.spawn((
            Transform::from_xyz(
                side_y * (arena_size + goal_size) / 2.,
                2.5,
                side_x * (arena_size + 0.5),
            ),
            RigidBody::Static,
            Collider::from(half_wall),
            Mesh3d(meshes.add(half_wall)),
            MeshMaterial3d(mats.add(StandardMaterial::from_color(Color::srgb(0.2, 0.2, 0.2)))),
        ));
    }

    // Goals
    let goal = Cuboid::new(goal_size * 2., 20., 5.);
    for side in [-1., 1.] {
        commands.spawn((
            Goal,
            Transform::from_xyz(0., 10., side * (arena_size + 2.5)),
            Collider::from(goal),
        ));
    }
}

fn spawn_ball(
    mut commands: Commands,
    time: Res<Time>,
    current: Query<(), With<Ball>>,
    mut counter: Local<f32>,
) {
    if !current.is_empty() {
        return;
    }
    *counter += time.delta_secs();
    if *counter < 0.5 {
        return;
    }
    *counter = 0.;

    commands.spawn((Ball, Transform::from_xyz(0., 5., -1.5), Replicated));
}

#[derive(Event, Serialize, Deserialize)]
struct LocalEntities {
    car: Entity,
}

fn claim_car(mut commands: Commands, car: Single<Entity, With<OurCar>>) {
    commands.send_event(LocalEntities { car: *car });
    commands.entity(*car).insert(InputAuthority);
}

fn spawn_client_cars(mut commands: Commands, mut spawns: EventReader<FromClient<LocalEntities>>) {
    for &FromClient {
        client_entity,
        event: LocalEntities {
            car: client_local_entity,
        },
    } in spawns.read()
    {
        let car_entity = commands
            .spawn((
                Car,
                Replicated,
                InputQueue::<GameInput>::default(),
                Transform::from_xyz(0., (0.3 + 0.35) / 2., 0.),
            ))
            .id();

        let mut entity_map = ClientEntityMap::default();

        entity_map.insert(car_entity, client_local_entity);
        commands.entity(client_entity).insert((
            InputTarget::all(car_entity),
            ReplicatedClient,
            entity_map,
        ));
    }
}

fn add_replicated(mut commands: Commands, car: Single<Entity, With<OurCar>>) {
    commands.entity(*car).insert(Replicated);
}

fn add_car_models(
    mut commands: Commands,
    query: Query<(Entity, Has<OurCar>), (With<Car>, Without<Mesh3d>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, is_ours) in query.iter() {
        let car_color = if is_ours {
            Color::srgb(0.5, 0.9, 0.85)
        } else {
            Color::srgb(1., 0.5, 0.5)
        };
        // TODO: reuse handles
        commands.entity(entity).insert((
            Mesh3d(meshes.add(Cuboid::new(0.5, 0.35, 1.))),
            MeshMaterial3d(mats.add(StandardMaterial::from_color(car_color))),
        ));
    }
}

fn add_ball_model(
    mut commands: Commands,
    query: Query<Entity, (With<Ball>, Without<Mesh3d>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    for entity in query.iter() {
        commands.entity(entity).insert((
            Mesh3d(meshes.add(Sphere::default())),
            MeshMaterial3d(mats.add(StandardMaterial::from_color(Color::WHITE))),
        ));
    }
}

fn move_cars(time: Res<Time>, mut cars: Query<(&mut Rotation, &mut LinearVelocity, &GameInput)>) {
    for (mut rotation, mut velocity, input) in cars.iter_mut() {
        let (mut vel_dir, vel_len) = match Dir2::new_and_length(velocity.xz()) {
            Ok(v) => v,
            _ => (Dir2::NEG_Y, 0.),
        };
        if vel_len < 0.02 {
            let rot2 = rotation.to_euler(EulerRot::YXZ).0;
            let rot2 = Rot2::radians(-rot2);
            vel_dir = rot2 * Dir2::NEG_Y;
        }
        let Some(dir) = input.direction else {
            let new_vel = vel_dir * (vel_len - time.delta_secs() * 5.).max(0.);
            **velocity = Vec3::new(new_vel.x, 0., new_vel.y) + Vec3::new(0., velocity.y, 0.);
            continue;
        };

        vel_dir = Dir2::new(vel_dir.rotate_towards(*dir, 1.5 * time.delta_secs())).unwrap();
        let vel_dir3 = Vec3::new(vel_dir.x, 0., vel_dir.y);
        **velocity =
            vel_dir3 * (vel_len + time.delta_secs() * 10.).min(5.) + Vec3::new(0., velocity.y, 0.);
        **rotation = Transform::default().looking_at(vel_dir3, Vec3::Y).rotation;
    }
}

fn follow_car(
    mut camera: Single<&mut Transform, (With<Camera3d>, Without<OurCar>)>,
    car: Single<&Transform, With<OurCar>>,
) {
    camera.translation = car.translation - car.forward() * 5. + Vec3::new(0., 1., 0.);
    camera.look_at(car.translation, Vec3::Y);
}

fn process_goals(
    mut commands: Commands,
    goals: Query<&CollidingEntities, With<Goal>>,
    ball: Single<Entity, With<Ball>>,
) {
    for colliding in goals.iter() {
        if colliding.contains(&*ball) {
            commands.disable_or_despawn(*ball);
        }
    }
}
