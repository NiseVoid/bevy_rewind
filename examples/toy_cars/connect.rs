use bevy::prelude::*;
use bevy_replicon::prelude::*;
use bevy_replicon_example_backend::{ExampleClient, ExampleServer};
use serde::{Deserialize, Serialize};

use crate::tick::GameTick;

pub fn connect_plugin(app: &mut App) {
    app
        // Connection events
        .add_client_event::<Connect>(RepliconChannel::from(ChannelKind::Unordered))
        .add_server_event::<CurrentTick>(RepliconChannel::from(ChannelKind::Unreliable))
        .make_independent::<CurrentTick>()
        // Set up state changes
        .init_state::<ConnectionState>()
        .enable_state_scoped_entities::<ConnectionState>()
        .add_systems(OnEnter(ConnectionState::Menu), setup_connect_ui)
        .add_systems(OnEnter(ConnectionState::Connecting), send_connect)
        .add_systems(
            Update,
            send_current_tick.run_if(resource_exists::<ExampleServer>),
        )
        .add_systems(
            Update,
            receive_tick.run_if(in_state(ConnectionState::Connecting)),
        )
        // Menu systems
        .add_systems(
            Update,
            (
                change_port.ignore_param_missing(),
                host_or_join.ignore_param_missing(),
            )
                .run_if(in_state(ConnectionState::Menu)),
        );
}

#[derive(States, Default, Clone, PartialEq, Eq, Debug, Hash)]
pub enum ConnectionState {
    #[default]
    Menu,
    Connecting,
    InGame,
}

#[derive(Event, Serialize, Deserialize)]
struct Connect;

#[derive(Event, Serialize, Deserialize)]
struct CurrentTick(GameTick);

#[derive(Component)]
struct PortInput;

#[derive(Component)]
#[require(Button, Text(|| Text("Host".into())))]
struct HostButton;

#[derive(Component)]
#[require(Button, Text(|| Text("Join".into())))]
struct JoinButton;

fn setup_connect_ui(mut commands: Commands) {
    let dark_gray = Color::srgb(0.2, 0.2, 0.2);

    commands.spawn((
        Node {
            padding: UiRect::all(Val::Px(10.)),
            ..default()
        },
        BackgroundColor(Color::BLACK),
        StateScoped(ConnectionState::Menu),
        children![
            (PortInput, Text("12345".into()), BackgroundColor(dark_gray)),
            (
                HostButton,
                Node {
                    margin: UiRect::left(Val::Px(5.)),
                    ..default()
                },
                BackgroundColor(dark_gray),
            ),
            (
                JoinButton,
                Node {
                    margin: UiRect::left(Val::Px(5.)),
                    ..default()
                },
                BackgroundColor(dark_gray),
            )
        ],
    ));

    commands.spawn((Camera2d::default(), StateScoped(ConnectionState::Menu)));
}

fn send_current_tick(
    mut commands: Commands,
    mut spawns: EventReader<FromClient<Connect>>,
    tick: Res<GameTick>,
) {
    for &FromClient { client_entity, .. } in spawns.read() {
        commands.send_event(ToClients {
            mode: SendMode::Direct(client_entity),
            event: CurrentTick(*tick),
        });
    }
}

fn change_port(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut text: Single<&mut Text, With<PortInput>>,
) {
    if keyboard_input.just_pressed(KeyCode::Backspace) {
        text.pop();
    }
    for (key, char) in [
        (KeyCode::Digit0, '0'),
        (KeyCode::Digit1, '1'),
        (KeyCode::Digit2, '2'),
        (KeyCode::Digit3, '3'),
        (KeyCode::Digit4, '4'),
        (KeyCode::Digit5, '5'),
        (KeyCode::Digit6, '6'),
        (KeyCode::Digit7, '7'),
        (KeyCode::Digit8, '8'),
        (KeyCode::Digit9, '9'),
    ] {
        if keyboard_input.just_pressed(key) {
            text.push(char);
            if !text.is_empty() && text.parse::<u16>().is_err() {
                text.pop();
            }
        }
    }
}

fn host_or_join(
    mut commands: Commands,
    port_input: Single<&Text, With<PortInput>>,
    host_button: Single<&Interaction, With<HostButton>>,
    join_button: Single<&Interaction, With<JoinButton>>,
) {
    if port_input.is_empty() {
        return;
    }
    let port = port_input.parse::<u16>().unwrap();
    if **host_button == Interaction::Pressed {
        eprintln!("Hosting on {port}");
        let Ok(socket) = ExampleServer::new(port) else {
            return;
        };

        commands.insert_resource(socket);
        commands.set_state(ConnectionState::InGame);
    } else if **join_button == Interaction::Pressed {
        eprintln!("Joining server on {port}");
        let Ok(socket) = ExampleClient::new(port) else {
            return;
        };
        commands.insert_resource(socket);
        commands.set_state(ConnectionState::Connecting);
    } else {
        return;
    }
}

fn send_connect(mut commands: Commands) {
    commands.send_event(Connect);
}

fn receive_tick(mut commands: Commands, mut events: EventReader<CurrentTick>) {
    let Some(&CurrentTick(mut tick)) = events.read().last() else {
        eprintln!("No tick :(");
        return;
    };
    eprintln!("Received tick!");
    commands.set_state(ConnectionState::InGame);
    *tick += 5; // Add a few ticks so we are ahead of the server
    commands.insert_resource(tick);
}
