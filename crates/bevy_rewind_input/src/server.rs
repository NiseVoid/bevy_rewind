//! Logic specific to server apps

use std::marker::PhantomData;

use crate::{HistoryFor, InputHistory, InputQueue, InputQueueSet, InputTrait, TickSource};

use bevy::{ecs::schedule::InternedScheduleLabel, prelude::*};
use bevy_replicon::prelude::*;

pub(super) struct InputQueueServerPlugin<T: InputTrait, Tick: TickSource> {
    schedule: InternedScheduleLabel,
    phantom: std::marker::PhantomData<(T, Tick)>,
}

impl<T: InputTrait, Tick: TickSource> InputQueueServerPlugin<T, Tick> {
    #[cfg(feature = "server")]
    pub fn new(schedule: InternedScheduleLabel) -> Self {
        Self {
            schedule,
            phantom: std::marker::PhantomData::<(T, Tick)>,
        }
    }
}

impl<T: InputTrait, Tick: TickSource> Plugin for InputQueueServerPlugin<T, Tick> {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            receive_inputs::<T, Tick>
                .run_if(server_running)
                .after(ServerSet::Receive)
                .in_set(InputQueueSet::Network),
        )
        .add_systems(
            PostUpdate,
            send_inputs::<T, Tick>
                .run_if(server_running)
                .before(ServerSet::Send)
                .in_set(InputQueueSet::Network),
        )
        .add_systems(
            self.schedule,
            load_inputs::<T, Tick>
                .run_if(server_running)
                .in_set(InputQueueSet::Load)
                // In case the configured schedule is PreUpdate
                .after(InputQueueSet::Network),
        );
    }
}

/// The entity to redirect the input to, use () as T to route all inputs, or an InputType
/// to route only that type. If both are specified, the InputType one takes precedence
#[derive(Component, Deref)]
pub struct InputTarget<T = ()>(#[deref] Entity, PhantomData<T>);

impl InputTarget<()> {
    /// Reroute all input for this client to the specified entity.
    /// If a specific-variant is also on this same entity, it will take precedence.
    pub fn all(entity: Entity) -> Self {
        Self(entity, PhantomData)
    }
}

impl<T> InputTarget<T> {
    /// Reroute input for this specific type to the specified entity.
    /// Takes precedence over [`InputTarget::all`] if both are present.
    pub fn specific(entity: Entity) -> Self {
        Self(entity, PhantomData)
    }
}

fn receive_inputs<T: InputTrait, Tick: TickSource>(
    input_target: Query<AnyOf<(&InputTarget<T>, &InputTarget)>>,
    mut events: EventReader<FromClient<InputHistory<T>>>,
    mut query: Query<&mut InputQueue<T>>,
    cur_tick: Res<Tick>,
) {
    for FromClient {
        client_entity,
        event,
    } in events.read()
    {
        let entity = input_target
            .get(*client_entity)
            .map(|(specific, all)| specific.map(|e| **e).unwrap_or(**all.unwrap()))
            .unwrap_or(*client_entity);
        let Ok(mut input_queue) = query.get_mut(entity) else {
            continue;
        };
        input_queue.add(*cur_tick, event);
    }
}

fn send_inputs<T: InputTrait, Tick: TickSource>(
    mut events: EventWriter<ToClients<HistoryFor<T>>>,
    query: Query<(Entity, &InputQueue<T>)>,
    cur_tick: Res<Tick>,
) {
    let cur_tick = (*cur_tick).into();
    for (entity, queue) in query.iter() {
        if queue.past().any(|(t, _)| *t >= cur_tick) || queue.queue().any(|(t, _)| *t < cur_tick) {
            warn_once!(
                "({:?}) Queue has inputs with impossible ticks: {:?}",
                cur_tick.get(),
                queue
            );
        }
        events.write(ToClients {
            mode: SendMode::Broadcast,
            event: HistoryFor {
                entity,
                tick: cur_tick,
                past: queue
                    .past()
                    .map(|(tick, t)| ((cur_tick.get() - tick.get()) as u8, t.clone()))
                    .collect(),
                future: queue
                    .queue()
                    .take(7)
                    .filter(|(tick, _)| tick.get() >= cur_tick.get())
                    .map(|(tick, t)| ((tick.get() - cur_tick.get()) as u8, t.clone()))
                    .collect(),
            },
        });
    }
}

fn load_inputs<T: InputTrait, Tick: TickSource>(
    mut query: Query<(&mut T, &mut InputQueue<T>)>,
    tick: Res<Tick>,
) {
    for (mut input, mut input_queue) in query.iter_mut() {
        match input_queue.next(*tick) {
            Some(new_input) => {
                *input = new_input;
            }
            None => {
                *input = default();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::schedule::ScheduleLabel;

    use super::*;
    use crate::tests::*;

    #[test]
    fn receives_inputs() {
        let mut app = App::new();

        let e1 = app.world_mut().spawn(InputQueue::<A>::default()).id();
        let e2 = app.world_mut().spawn(InputQueue::<A>::default()).id();
        let e3 = app.world_mut().spawn(InputQueue::<A>::default()).id();
        let e4 = app
            .world_mut()
            .spawn((InputQueue::<A>::default(), InputTarget::all(e3)))
            .id();

        app.add_event::<FromClient<InputHistory<A>>>()
            .add_systems(Update, receive_inputs::<A, Tick>)
            .insert_resource(Tick(5));

        app.world_mut().send_event_batch([
            FromClient {
                client_entity: e1,
                event: hist(4, [A(1), A(2), A(3)]),
            },
            FromClient {
                client_entity: e2,
                event: hist(5, [A(1), A(2), A(3)]),
            },
            FromClient {
                client_entity: e4,
                event: hist(6, [A(1), A(2), A(3)]),
            },
        ]);

        app.update();

        // We should not spawn new entities for the unknown clients
        assert_eq!(
            4,
            app.world_mut()
                .query::<&InputQueue<A>>()
                .iter(app.world())
                .count()
        );

        let [e1, e2, e3, e4] = app.world().get_entity([e1, e2, e3, e4]).unwrap();
        assert_eq!(
            vec![&(Tick(5).into(), A(2)), &(Tick(6).into(), A(3))],
            e1.get::<InputQueue<A>>()
                .unwrap()
                .queue()
                .collect::<Vec<_>>()
        );
        assert_eq!(
            vec![
                &(Tick(6).into(), A(1)),
                &(Tick(7).into(), A(2)),
                &(Tick(8).into(), A(3))
            ],
            e2.get::<InputQueue<A>>()
                .unwrap()
                .queue()
                .collect::<Vec<_>>()
        );
        // e3 had no events
        assert_eq!(0, e3.get::<InputQueue<A>>().unwrap().queue().count());
        // e4 isn't in ClientEvents and thus can't receive anything
        assert_eq!(0, e4.get::<InputQueue<A>>().unwrap().queue().count());
    }

    #[test]
    fn sends_inputs() {
        let mut app = App::new();
        app.add_event::<ToClients<HistoryFor<A>>>()
            .add_systems(Update, send_inputs::<A, Tick>)
            .insert_resource(Tick(5));

        let mut queue = InputQueue::<A>::default();
        queue.add(Tick(5), &hist(5, [A(1)]));
        assert_eq!(Some(A(1)), queue.next(Tick(5)));
        queue.add(Tick(6), &hist(7, [A(3), A(4)]));

        let e1 = app.world_mut().spawn(queue).id();

        app.update();

        let mut events = app
            .world()
            .resource::<Events<ToClients<HistoryFor<A>>>>()
            .iter_current_update_events();
        assert_eq!(
            HistoryFor {
                entity: e1,
                tick: Tick(5).into(),
                past: [(0u8, A(1))].into_iter().collect(),
                future: [(2u8, A(3)), (3, A(4))].into_iter().collect(),
            },
            events.next().unwrap().event,
        );
        assert!(events.next().is_none());
    }

    #[test]
    fn loads_inputs_with_queue() {
        let mut app = App::new();
        app.add_systems(Update, load_inputs::<A, Tick>)
            .insert_resource(Tick(5));

        let mut queue = InputQueue::<A>::default();
        queue.add(Tick(5), &hist(4, [A(0), A(1), A(2)]));
        let e1 = app.world_mut().spawn((A(94), queue)).id();

        let mut queue = InputQueue::<A>::default();
        queue.add(Tick(5), &hist(6, [A(1), A(2)]));
        let e2 = app.world_mut().spawn((A(94), queue)).id();

        app.update();

        // These is input so it should be used
        let e = app.world().entity(e1);
        assert_eq!(A(1), *e.get::<A>().unwrap());
        // There is no input for this tick so this entity goes back to default
        let e = app.world().entity(e2);
        assert_eq!(A(0), *e.get::<A>().unwrap());

        app.insert_resource(Tick(6));
        app.update();

        // We load the next input
        let e = app.world().entity(e1);
        assert_eq!(A(2), *e.get::<A>().unwrap());
        // This entity has input now
        let e = app.world().entity(e2);
        assert_eq!(A(1), *e.get::<A>().unwrap());

        app.insert_resource(Tick(7));
        app.update();

        // We repeat an old input
        let e = app.world().entity(e1);
        assert_eq!(A(2), *e.get::<A>().unwrap());
        // This entity has a new input
        let e = app.world().entity(e2);
        assert_eq!(A(2), *e.get::<A>().unwrap());
    }

    #[test]
    fn clears_inputs_without_queue() {
        let mut app = App::new();
        app.add_systems(Update, load_inputs::<A, Tick>)
            .insert_resource(Tick(5));

        let e1 = app.world_mut().spawn(A(94)).id();

        app.update();

        // There is no history, so the input is cleared
        let e = app.world().entity(e1);
        assert_eq!(A(0), *e.get::<A>().unwrap());
    }

    #[test]
    fn receive_and_send_has_no_frame_delay() {
        let mut app = App::new();

        let e1 = app.world_mut().spawn(InputQueue::<A>::default()).id();

        let mut server = RepliconServer::default();
        server.set_running(true);
        app.add_event::<FromClient<InputHistory<A>>>()
            .add_event::<ToClients<HistoryFor<A>>>()
            .add_plugins(InputQueueServerPlugin::<A, Tick>::new(Update.intern()))
            .insert_resource(server)
            .insert_resource(Tick(5));

        app.world_mut().send_event_batch([FromClient {
            client_entity: e1,
            event: hist(4, [A(1), A(2), A(3)]),
        }]);

        app.update();

        let mut events = app
            .world()
            .resource::<Events<ToClients<HistoryFor<A>>>>()
            .iter_current_update_events();
        assert_eq!(
            HistoryFor {
                entity: e1,
                tick: Tick(5).into(),
                past: default(),
                future: [(0u8, A(2)), (1, A(3))].into_iter().collect(),
            },
            events.next().unwrap().event,
        );
        assert!(events.next().is_none());
    }

    #[test]
    fn repeat_late_inputs() {
        let mut app = App::new();

        app.add_systems(Update, load_inputs::<A, Tick>)
            .insert_resource(Tick(7));

        let mut queue = InputQueue::<A>::default();
        queue.add(Tick(0), &hist(4, [A(0), A(1), A(2)]));
        let e1 = app.world_mut().spawn((A(94), queue)).id();

        let mut queue = InputQueue::<A>::default();
        queue.add(Tick(0), &hist(0, [A(0), A(1)]));
        let e2 = app.world_mut().spawn((A(94), queue)).id();

        app.update();

        // All the data was old, but they could still be repeated
        let e = app.world().entity(e1);
        assert_eq!(A(2), *e.get::<A>().unwrap());

        // All the data was old, and could no longer be repeated
        let e = app.world().entity(e2);
        assert_eq!(A(0), *e.get::<A>().unwrap());
    }
}
