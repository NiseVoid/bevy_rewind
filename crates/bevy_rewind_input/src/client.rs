//! Logic specific to client apps

use crate::{HistoryFor, InputHistory, InputQueueSet, InputTrait, TickSource};

use bevy::{ecs::schedule::InternedScheduleLabel, prelude::*};
use bevy_replicon::{client::ClientSet, prelude::client_connected};

pub(super) struct InputQueueClientPlugin<T: InputTrait, Tick: TickSource> {
    schedule: InternedScheduleLabel,
    phantom: std::marker::PhantomData<(T, Tick)>,
}

impl<T: InputTrait, Tick: TickSource> InputQueueClientPlugin<T, Tick> {
    #[cfg(feature = "client")]
    pub fn new(schedule: InternedScheduleLabel) -> Self {
        Self {
            schedule,
            phantom: std::marker::PhantomData::<(T, Tick)>,
        }
    }
}

impl<T: InputTrait, Tick: TickSource> Plugin for InputQueueClientPlugin<T, Tick> {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            receive_inputs::<T>
                .run_if(client_connected)
                .after(ClientSet::Receive)
                .in_set(InputQueueSet::Network),
        )
        .add_systems(
            self.schedule,
            load_inputs::<T, Tick>
                .in_set(InputQueueSet::Load)
                .run_if(client_connected),
        )
        .add_systems(
            FixedPostUpdate,
            store_inputs::<T, Tick>
                .in_set(InputQueueSet::Clean)
                .run_if(client_connected),
        )
        .add_systems(
            PostUpdate,
            send_input_events::<T>
                .run_if(client_connected)
                .before(ClientSet::Send)
                .in_set(InputQueueSet::Network),
        );
    }
}

/// A marker component for entities for which this client has authority to send inputs
#[derive(Component)]
pub struct InputAuthority;

fn store_inputs<T: InputTrait, Tick: TickSource>(
    mut query: Query<(&mut InputHistory<T>, &mut T), With<InputAuthority>>,
    tick: Res<Tick>,
) {
    for (mut hist, mut input) in query.iter_mut() {
        match hist.updated_at().partial_cmp(&(*tick).into()).unwrap() {
            std::cmp::Ordering::Greater => {
                hist.reset();
            }
            std::cmp::Ordering::Equal => {
                continue;
            }
            std::cmp::Ordering::Less => {}
        };

        let taken = std::mem::take(&mut *input);
        hist.write(*tick, taken);
    }
}

fn load_inputs<T: InputTrait, Tick: TickSource>(
    mut query: Query<(&InputHistory<T>, &mut T, Has<InputAuthority>)>,
    tick: Res<Tick>,
) {
    for (hist, mut input, authority) in query.iter_mut() {
        let i = hist.get(*tick).cloned();
        if i.is_none() && authority {
            continue;
        }
        *input = i.unwrap_or_default();
    }
}

fn send_input_events<T: InputTrait>(
    hist: Query<&InputHistory<T>, With<InputAuthority>>,
    mut events: EventWriter<InputHistory<T>>,
) {
    for hist in hist.iter() {
        if hist.is_empty() {
            continue;
        }
        events.write(hist.clone());
    }
}

fn receive_inputs<T: InputTrait>(
    mut events: EventReader<HistoryFor<T>>,
    mut query: Query<&mut InputHistory<T>>,
) {
    for HistoryFor {
        entity,
        tick,
        past,
        future,
    } in events.read()
    {
        let Ok(mut history) = query.get_mut(*entity) else {
            warn_once!(
                "Received history for entity without InputHistory: {}",
                entity
            );
            continue;
        };
        let mut past_iter = past.iter().peekable();
        while let (Some((rt, t)), until) = (
            past_iter.next(),
            past_iter.peek().map(|(rt, _)| *rt).unwrap_or_default(),
        ) {
            // Expand each item into the inputs it caused
            history.replace_section((until..=*rt).skip(1).rev().filter_map(|rrt| {
                t.repeated((*rt - rrt) as u32)
                    .map(|t| (*tick - rrt as u32, t))
            }));
        }
        history.replace_section(future.iter().map(|(rt, t)| (*tick + *rt as u32, t.clone())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::*;

    #[test]
    fn stores_inputs_with_authority() {
        let mut app = App::new();
        app.add_systems(Update, store_inputs::<A, Tick>)
            .insert_resource(Tick(5));
        let e1 = app
            .world_mut()
            .spawn((A(4), InputHistory::<A>::default(), InputAuthority))
            .id();
        let e2 = app
            .world_mut()
            .spawn((A(5), InputHistory::<A>::default()))
            .id();

        app.update();

        // Entities with InputAuthority should have history written
        let e = app.world().entity(e1);
        assert_eq!(hist(5, [A(4)]), *e.get::<InputHistory<A>>().unwrap());

        // Entities without should not
        let e = app.world().entity(e2);
        assert_eq!(hist(0, []), *e.get::<InputHistory<A>>().unwrap());
    }

    #[test]
    fn sends_inputs_with_authority() {
        let mut app = App::new();
        app.add_event::<InputHistory<A>>()
            .add_systems(Update, send_input_events::<A>)
            .insert_resource(Tick(5));
        app.world_mut().spawn((hist(5, [A(2)]), InputAuthority));
        app.world_mut().spawn(hist(5, [A(1)]));
        app.world_mut().spawn(hist::<A>(0, []));

        app.update();

        let mut events = app
            .world()
            .resource::<Events<InputHistory<A>>>()
            .iter_current_update_events();
        // An update was sent for the entity with authority
        assert_eq!(Some(&hist(5, [A(2)])), events.next());
        // But not for other entities
        assert_eq!(None, events.next());
    }

    #[test]
    fn loads_inputs_without_authority() {
        let mut app = App::new();
        app.add_systems(Update, load_inputs::<A, Tick>)
            .insert_resource(Tick(5));
        let e1 = app
            .world_mut()
            .spawn((A(15), hist(3, [A(1), A(2), A(3)]), InputAuthority))
            .id();
        let e2 = app
            .world_mut()
            .spawn((A(0), hist(4, [A(1), A(2), A(3)])))
            .id();
        let e3 = app
            .world_mut()
            .spawn((A(0), hist(5, [A(1), A(2), A(3)])))
            .id();

        app.update();

        // Entities with InputAuthority should be reset
        let e = app.world().entity(e1);
        assert_eq!(A(0), *e.get::<A>().unwrap());

        // Entities with InputAuthority should load history
        let e = app.world().entity(e2);
        assert_eq!(A(2), *e.get::<A>().unwrap());
        let e = app.world().entity(e3);
        assert_eq!(A(1), *e.get::<A>().unwrap());
    }

    #[test]
    fn receive_input_writes_history() {
        let mut app = App::new();
        app.add_event::<HistoryFor<A>>()
            .add_systems(Update, receive_inputs::<A>);
        let e1 = app.world_mut().spawn(InputHistory::<A>::default()).id();
        let e2 = app.world_mut().spawn(InputHistory::<A>::default()).id();

        app.world_mut().send_event(HistoryFor {
            entity: e1,
            tick: Tick(5).into(),
            past: [(4u8, A(1)), (1, A(2))].into_iter().collect(),
            future: [(0, A(3)), (2, A(4))].into_iter().collect(),
        });

        app.update();

        // The target entity needs to have history written
        let actual = app.world().entity(e1).get::<InputHistory<A>>();
        let expected = hist(1, [A(1), A(1), A(1), A(2), A(3), A(0), A(4)]);
        assert_eq!(Some(&expected), actual);

        // Other entities need to stay untouched
        let actual = app.world().entity(e2).get::<InputHistory<A>>();
        let expected = hist(0, []);
        assert_eq!(Some(&expected), actual);
    }
}
