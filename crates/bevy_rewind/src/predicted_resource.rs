// TODO: Share this logic with component history

use crate::{RollbackFrames, StoreFor, TickData};

use std::{collections::VecDeque, fmt::Debug};

use bevy::prelude::*;
use bevy_replicon::shared::replicon_tick::RepliconTick;

/// The prediction history of a resource
#[derive(Resource, Clone)]
pub struct ResourceHistory<T> {
    list: VecDeque<TickData<T>>,
    last_tick: u32,
}

impl<T> Default for ResourceHistory<T> {
    fn default() -> Self {
        Self {
            list: default(),
            last_tick: 0,
        }
    }
}

impl<T> ResourceHistory<T> {
    #[cfg(test)]
    pub(crate) fn from_list<const N: usize>(start_tick: u32, list: [TickData<T>; N]) -> Self {
        let last_tick = start_tick + (list.len() as u32).saturating_sub(1);
        Self {
            list: VecDeque::from(list),
            last_tick,
        }
    }

    /// Get the length of the history
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Check if the history is empty
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Get the value for the specified tick. You always want to load the value stored on
    /// the previous tick
    pub fn get(&self, previous_tick: RepliconTick) -> &TickData<T> {
        if previous_tick.get() > self.last_tick {
            return &TickData::Missing;
        }
        let ago = (self.last_tick - previous_tick.get()) as usize;
        let len = self.list.len();
        if ago >= len {
            return if self
                .list
                .front()
                .is_some_and(|v| matches!(v, TickData::Removed))
            {
                &TickData::Removed
            } else {
                &TickData::Missing
            };
        }
        self.list.get(len - 1 - ago).unwrap_or(&TickData::Missing)
    }

    /// Clean all values after the specified tick. You always want to clean values stored after
    /// the previous tick.
    pub fn clean(&mut self, previous_tick: RepliconTick) {
        let ago = self.last_tick.saturating_sub(previous_tick.get());
        let len = self.list.len();
        // We clean all values after previous tick
        self.list.drain(len.saturating_sub(ago as usize)..);
        self.last_tick = self.last_tick.min(previous_tick.get());
    }

    /// Keep only the first item in the history
    pub fn keep_one(&mut self) {
        let len = self.list.len();
        self.list.truncate(1);
        self.last_tick -= (len as u32).saturating_sub(1);
    }
}

pub(super) fn append_history<T: Resource + Clone + Debug>(
    t: Option<Res<T>>,
    mut hist: ResMut<ResourceHistory<T>>,
    tick: Res<StoreFor>,
    frames: Res<RollbackFrames>,
) {
    let max_ticks = frames.history_size();

    let cap = hist.list.capacity();
    match cap.cmp(&max_ticks) {
        std::cmp::Ordering::Greater => {
            let mut old_list =
                std::mem::replace(&mut hist.list, VecDeque::with_capacity(max_ticks));
            let skip = old_list.len().saturating_sub(max_ticks);
            hist.list.extend(old_list.drain(..).skip(skip));
        }
        std::cmp::Ordering::Less => {
            hist.list.reserve_exact(max_ticks - cap);
        }
        _ => {}
    }

    if !hist.is_empty() {
        if tick.get() <= hist.last_tick {
            // TODO: Overwrite the old parts of the history if the value was not Removed or this wouldn't be the first value
            return;
        }
        // We need to patch gaps
        while tick.get() > hist.last_tick + 1 {
            if hist.list.len() == hist.list.capacity() {
                hist.list.pop_front();
            }
            let cloned = hist.list.back().unwrap().clone();
            hist.list.push_back(cloned);
            hist.last_tick += 1;
        }
    }

    if hist.list.len() == hist.list.capacity() {
        hist.list.pop_front();
    }
    hist.list.push_back(
        t.map(|t| TickData::Value(t.clone()))
            .unwrap_or(TickData::Removed),
    );
    hist.last_tick = tick.get();
}

/// A system that saves the initial spawn value if history is empty
// TODO: Figure something out for reconnecting
pub(super) fn save_initial<T: Resource + Clone + Debug>(
    t: Res<T>,
    mut history: ResMut<ResourceHistory<T>>,
    tick: Res<StoreFor>,
) {
    if history.is_empty() {
        history.last_tick = tick.get();
        history.list.push_back(TickData::Value(t.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{set_store_tick, tests::Tick};
    use TickData::Missing;

    #[derive(Resource, Clone, Copy, Deref, DerefMut, PartialEq, Eq, Debug)]
    struct A(u8);
    #[derive(Resource, Clone, Copy, Deref, DerefMut, PartialEq, Eq, Debug)]
    struct B(u8);

    fn a(v: u8) -> TickData<A> {
        TickData::Value(A(v))
    }
    fn b(v: u8) -> TickData<B> {
        TickData::Value(B(v))
    }

    #[track_caller]
    fn list_array<T: Resource + Clone + Copy + Debug, const N: usize>(
        history: &ResourceHistory<T>,
    ) -> [TickData<T>; N] {
        assert_eq!(N, history.list.len(), "Length mismatch");
        let mut list = [TickData::<T>::Missing; N];
        for (i, item) in history.list.iter().take(N).enumerate() {
            list[i] = *item;
        }
        list
    }

    fn increment_tick(mut tick: ResMut<Tick>) {
        **tick += 1;
    }

    fn init_app() -> App {
        let mut app = App::new();
        let max_ticks = RollbackFrames(3);
        app.init_resource::<Tick>()
            .insert_resource(max_ticks)
            .add_systems(PreUpdate, set_store_tick::<Tick>)
            .add_systems(Update, (append_history::<A>, append_history::<B>))
            .add_systems(PostUpdate, increment_tick);

        app.insert_resource(A(1))
            .init_resource::<ResourceHistory<A>>()
            .insert_resource(B(2))
            .init_resource::<ResourceHistory<B>>();

        app
    }

    #[track_caller]
    fn assert_lengths(app: &App, len: usize) {
        if let Some(a) = app.world().get_resource::<ResourceHistory<A>>() {
            assert_eq!(len, a.list.len(), "Length does not match");
        }
        if let Some(b) = app.world().get_resource::<ResourceHistory<B>>() {
            assert_eq!(len, b.list.len(), "Length does not match");
        }
    }

    #[track_caller]
    fn assert_capacity(app: &App, cap: usize) {
        if let Some(a) = app.world().get_resource::<ResourceHistory<A>>() {
            assert_eq!(cap, a.list.capacity(), "Capacity does not match");
        }
        if let Some(b) = app.world().get_resource::<ResourceHistory<B>>() {
            assert_eq!(cap, b.list.capacity(), "Capacity does not match");
        }
    }

    fn increment_resources(app: &mut App) {
        if let Some(mut a) = app.world_mut().get_resource_mut::<A>() {
            **a += 1;
        }
        if let Some(mut b) = app.world_mut().get_resource_mut::<B>() {
            **b += 1;
        }
    }

    #[test]
    fn history_appends() {
        let mut app = init_app();

        for length in [0, 1] {
            assert_lengths(&app, length);
            app.update();
            increment_resources(&mut app);
            assert_lengths(&app, length + 1);
        }

        let hist_a = app.world().resource::<ResourceHistory<A>>();
        assert_eq!([a(1), a(2)], list_array(hist_a));
        let hist_b = app.world().resource::<ResourceHistory<B>>();
        assert_eq!([b(2), b(3)], list_array(hist_b));
    }

    #[test]
    fn history_removes_and_reinserts() {
        let mut app = init_app();

        assert_lengths(&app, 0);

        app.update();
        assert_lengths(&app, 1);

        increment_resources(&mut app);
        app.world_mut().remove_resource::<A>();
        app.update();
        assert_lengths(&app, 2);

        increment_resources(&mut app);
        app.world_mut().insert_resource(A(3));
        app.world_mut().remove_resource::<B>();
        app.update();
        assert_lengths(&app, 3);

        let hist_a = app.world().resource::<ResourceHistory<A>>();
        assert_eq!([a(1), TickData::Removed, a(3)], list_array(hist_a));
        let hist_b = app.world().resource::<ResourceHistory<B>>();
        assert_eq!([b(2), b(3), TickData::Removed], list_array(hist_b));
    }

    #[test]
    fn history_wraps() {
        let mut app = init_app();

        for length in [1, 2, 3, 4, 5, 5, 5] {
            app.update();
            assert_lengths(&app, length);
            increment_resources(&mut app);
        }

        let hist_a = app.world().resource::<ResourceHistory<A>>();
        assert_eq!([a(3), a(4), a(5), a(6), a(7)], list_array(hist_a));
    }

    #[test]
    fn history_resizes_to_match_rollback_frames() {
        let mut app = init_app();

        app.update();
        assert_lengths(&app, 1);
        assert_capacity(&app, 5);

        *app.world_mut().resource_mut::<RollbackFrames>() = RollbackFrames(1);
        for length in [2, 3, 3, 3] {
            app.update();
            assert_lengths(&app, length);
            assert_capacity(&app, 3);
        }

        *app.world_mut().resource_mut::<RollbackFrames>() = RollbackFrames(5);
        for length in [4, 5, 6, 7, 7, 7] {
            app.update();
            assert_lengths(&app, length);
            assert_capacity(&app, 7);
        }
    }

    #[test]
    fn fast_forwarded() {
        let mut app = init_app();

        app.update();
        assert_lengths(&app, 1);

        *app.world_mut().resource_mut::<Tick>() = Tick(3);

        for _ in 0..3 {
            increment_resources(&mut app);
        }

        app.update();
        assert_lengths(&app, 4);

        let hist_a = app.world().resource::<ResourceHistory<A>>();
        assert_eq!([a(1), a(1), a(1), a(4)], list_array(hist_a));
    }

    #[test]
    fn fast_forwarded_wraps() {
        let mut app = init_app();

        app.update();
        assert_lengths(&app, 1);

        *app.world_mut().resource_mut::<Tick>() = Tick(10);

        for _ in 0..10 {
            increment_resources(&mut app);
        }

        app.update();
        assert_lengths(&app, 5);

        let hist_a = app.world().resource::<ResourceHistory<A>>();
        assert_eq!([a(1), a(1), a(1), a(1), a(11)], list_array(hist_a));
    }

    #[test]
    fn get() {
        let mut history = ResourceHistory {
            list: VecDeque::from([a(5), a(6), TickData::Removed, a(8)]),
            last_tick: 6,
        };

        // A valid tick within the history returns the value
        assert_eq!(&a(5), history.get(RepliconTick::new(3)));
        assert_eq!(&a(6), history.get(RepliconTick::new(4)));
        assert_eq!(&TickData::Removed, history.get(RepliconTick::new(5)));
        assert_eq!(&a(8), history.get(RepliconTick::new(6)));

        // A tick before the history returns Missing
        assert_eq!(&Missing, history.get(RepliconTick::new(1)));
        assert_eq!(&Missing, history.get(RepliconTick::new(2)));

        // A tick after the history returns Missing
        assert_eq!(&Missing, history.get(RepliconTick::new(7)));
        assert_eq!(&Missing, history.get(RepliconTick::new(2589)));

        // If the oldest value is Removed, all ticks before it are considered Removed
        history.list[0] = TickData::Removed;
        assert_eq!(&TickData::Removed, history.get(RepliconTick::new(3)));
        assert_eq!(&TickData::Removed, history.get(RepliconTick::new(1)));
    }

    #[test]
    fn clean() {
        let original = ResourceHistory {
            list: VecDeque::from([a(5), a(6), a(7)]),
            last_tick: 5,
        };

        // A tick before the history clears everything
        for tick in [Tick(1), Tick(2)] {
            let mut history = original.clone();
            history.clean(tick.into());
            assert_eq!(0, history.list.len());
            assert_eq!(RepliconTick::from(tick).get(), history.last_tick);
        }

        // A tick within the history cleans all values after it
        for tick in [Tick(3), Tick(4), Tick(5)] {
            let mut history = original.clone();
            history.clean(tick.into());
            assert_eq!(3 - (5 - *tick as usize), history.list.len());
            assert_eq!(RepliconTick::from(tick).get(), history.last_tick);
        }

        // A tick after the history does nothing
        for tick in [Tick(6), Tick(2589)] {
            let mut history = original.clone();
            history.clean(tick.into());
            assert_eq!(3, history.list.len());
            assert_eq!(5, history.last_tick);
        }
    }

    #[test]
    fn keep_one() {
        let mut history = ResourceHistory {
            list: VecDeque::from([a(5), a(6), a(7)]),
            last_tick: 5,
        };
        assert_eq!(3, history.list.len());
        assert_eq!(5, history.last_tick);

        history.keep_one();

        assert_eq!(1, history.list.len());
        assert_eq!(3, history.last_tick);

        // Calling it with one item should have no effect
        history.keep_one();

        assert_eq!(1, history.list.len());
        assert_eq!(3, history.last_tick);
    }

    #[test]
    fn keep_one_empty() {
        let mut history = ResourceHistory::<A> {
            list: VecDeque::new(),
            last_tick: 5,
        };

        // This shouldn't panic or do anything weird
        history.keep_one();
        assert_eq!(0, history.len());
        assert_eq!(5, history.last_tick);
    }

    #[test]
    fn keep_one_doesnt_get_overridden() {
        let mut app = init_app();
        **app.world_mut().resource_mut::<Tick>() += 3;

        app.update();
        increment_resources(&mut app);
        app.update();

        assert_lengths(&app, 2);

        let world = app.world_mut();
        world.remove_resource::<A>();
        world.resource_mut::<ResourceHistory<A>>().keep_one();
        world.remove_resource::<B>();
        world.resource_mut::<ResourceHistory<B>>().keep_one();

        assert_lengths(&app, 1);

        **app.world_mut().resource_mut::<Tick>() -= 3;

        // While the tick is old, the history should remain unchanged
        app.update();
        assert_lengths(&app, 1);
        app.update();
        assert_lengths(&app, 1);

        // Values get written as normal again when the tick is back
        app.update();
        assert_lengths(&app, 2);

        let history = app.world().resource::<ResourceHistory<A>>();
        assert_eq!([a(1), TickData::Removed], list_array(history));
    }
}
