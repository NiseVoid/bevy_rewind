# bevy_rewind

Server-authoritative rollback networking for bevy. This crate is roughly inspired by [this GDC talk about Rocket League](https://youtu.be/ueEmiDM94IE?t=1417).
bevy_rewind is currently built on top of bevy_replicon, but this could change in the future, for example when first-party networking is added to bevy.

## Subcrates

- bevy_rewind: The rollback system itself
- bevy_rewind_input: Rollback compatible input queue logic, can be used as a standalone crate
- bevy_rewind_entity_management: QoL improvements when dealing with spawning/despawning as part of a simulation

## How to use

1. Add the `RollbackPlugin` to your app, providing your own tick type, a schedule to run, and a schedule in which to write component values to history
2. Register components to be rolled back trough `register_authoritative_component` or `register_predicted_component`
3. When replicon receives new data, the world gets rolled back before `RunFixedMainLoop`, and your provided schedule is ran until the world is back to the present again

For more details, you can look at the example app.

## Is this the right crate for me?

This heavily depends on what you are building. This crate applies rollback and resimulation to the entire world, which makes it a great option for games that need physics interactions to work correctly.
However, this approach is fairly expensive and can still produce unexpected results when inputs can lead to instant actions (for example with hitscan weapons, or abilities without any anticipation frames)

## License

All code in this repository is dual-licensed under either:

* MIT License ([LICENSE-MIT](LICENSE-MIT) or [http://opensource.org/licenses/MIT](http://opensource.org/licenses/MIT))
* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or [http://www.apache.org/licenses/LICENSE-2.0](http://www.apache.org/licenses/LICENSE-2.0))

at your option.

