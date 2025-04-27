use crate::{InputHistory, InputTrait};

use arraydeque::{ArrayDeque, Wrapping};
use bevy::prelude::*;
use bevy_replicon::shared::replicon_tick::RepliconTick;

/// A queue containing inputs
#[derive(Component, Debug)]
pub struct InputQueue<T: InputTrait> {
    past: ArrayDeque<(RepliconTick, T), 3, Wrapping>,
    queue: ArrayDeque<(RepliconTick, T), 30>,
}

impl<T: InputTrait> Default for InputQueue<T> {
    fn default() -> Self {
        Self {
            past: ArrayDeque::new(),
            queue: ArrayDeque::new(),
        }
    }
}

impl<T: InputTrait> InputQueue<T> {
    pub(crate) fn past(&self) -> impl Iterator<Item = &(RepliconTick, T)> {
        self.past.iter()
    }

    pub(crate) fn queue(&self) -> impl Iterator<Item = &(RepliconTick, T)> {
        self.queue.iter()
    }

    pub(crate) fn add(&mut self, tick: impl Into<RepliconTick>, history: &InputHistory<T>) {
        let newest_missing = RepliconTick::new(
            tick.into().get().max(
                self.queue
                    .back()
                    .map(|(tick, _)| tick.get() + 1)
                    .unwrap_or_default(),
            ),
        );
        if history.updated_at() < newest_missing {
            return;
        }

        let first_tick = history.first_tick();
        let offset = newest_missing.get().saturating_sub(first_tick.get()) as usize;
        let remaining_capacity = self.queue.capacity() - self.queue.len();

        self.queue.extend_back(
            history
                .iter()
                .enumerate()
                .skip(offset)
                .take(remaining_capacity)
                .map(|(i, t)| (first_tick + i as u32, t.clone())),
        );
    }

    pub(crate) fn next(&mut self, tick: impl Into<RepliconTick>) -> Option<T> {
        let tick = tick.into();
        let mut newest_miss = None;
        while !self.queue.is_empty() && self.queue[0].0 < tick {
            newest_miss = self.queue.pop_front();
        }
        if self.queue.is_empty() || self.queue[0].0 != tick {
            if let Some((from_tick, t)) = newest_miss {
                if let Some(input) = t.repeated(tick - from_tick) {
                    self.past.push_back((tick, input.clone()));
                    return Some(input);
                }
            }
            return self
                .past
                .back()
                .and_then(|(from_tick, t)| t.repeated(tick - *from_tick));
        }

        let (tick, t) = self.queue.pop_front()?;
        self.past.push_back((tick, t.clone()));
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::*;

    use bevy::ecs::entity::MapEntities;
    use serde::{Deserialize, Serialize};

    #[derive(Component, Clone, Default, Serialize, Deserialize, Debug, PartialEq, TypePath)]
    pub struct NoRepeat(u8);

    impl InputTrait for NoRepeat {
        fn repeats() -> bool {
            false
        }
    }

    impl MapEntities for NoRepeat {
        fn map_entities<M: EntityMapper>(&mut self, _: &mut M) {}
    }

    #[test]
    fn queue_skips_older_inputs() {
        let mut queue = InputQueue::<A>::default();

        // List starts empty
        assert_eq!(queue.queue.len(), 0);

        // If the entire history is from before the current tick, it is ignored
        queue.add(Tick(10), &hist(7, [A(79), A(80)]));
        assert_eq!(queue.queue.len(), 0);

        // When adding items to an empty queue, only the new items get added
        queue.add(Tick(10), &hist(9, [A(0), A(1), A(2)]));
        assert_eq!(queue.queue.len(), 2);

        // When adding items to a queue that has items, only newer items get added
        queue.add(Tick(10), &hist(10, [A(1), A(2), A(3)]));
        assert_eq!(queue.queue.len(), 3);

        // If for whatever reason there is a gap nothing should break
        queue.add(Tick(10), &hist(15, [A(6), A(7)]));
        assert_eq!(queue.queue.len(), 5);

        assert_eq!(
            ArrayDeque::from([
                (RepliconTick::new(10), A(1)),
                (RepliconTick::new(11), A(2)),
                (RepliconTick::new(12), A(3)),
                (RepliconTick::new(15), A(6)),
                (RepliconTick::new(16), A(7)),
            ]),
            queue.queue
        );
    }

    #[test]
    fn queue_doesnt_overflow() {
        let mut queue = InputQueue::<A>::default();

        queue.add(Tick(10), &hist(7, (0..100).map(A)));
        assert_eq!(queue.queue.len(), 30);
    }

    #[test]
    fn queue_repeats_actions_when_none_available() {
        let mut queue = InputQueue::<A>::default();
        queue.add(Tick(10), &hist(10, [A(0)]));
        queue.add(Tick(10), &hist(17, [A(7)]));

        // We get the actual input
        assert_eq!(queue.next(Tick(10)), Some(A(0)));
        // There is no input, but the last one should still repeat
        assert_eq!(queue.next(Tick(11)), Some(A(0)));
        // Still repeating
        assert_eq!(queue.next(Tick(15)), Some(A(0)));
        // Now it should no longer repeat
        assert_eq!(queue.next(Tick(16)), None);
        // And now we should get the next input
        assert_eq!(queue.next(Tick(17)), Some(A(7)));
    }

    #[test]
    fn queue_repeat_is_optional() {
        let mut queue = InputQueue::<NoRepeat>::default();
        queue.add(Tick(10), &hist(10, [NoRepeat(0)]));
        queue.add(Tick(10), &hist(17, [NoRepeat(7)]));

        // We get the actual input
        assert_eq!(queue.next(Tick(10)), Some(NoRepeat(0)));
        // There is no input, and we shouldn't repeat
        assert_eq!(queue.next(Tick(11)), None);
        // Still no repeating
        assert_eq!(queue.next(Tick(15)), None);
        // And now we should get the next input
        assert_eq!(queue.next(Tick(17)), Some(NoRepeat(7)));
    }

    #[test]
    fn queue_skips_old_values() {
        let mut queue = InputQueue::<A>::default();
        queue.add(Tick(9), &hist(9, [A(0), A(1), A(2)]));

        assert_eq!(queue.next(Tick(10)), Some(A(1)));
    }

    #[test]
    fn queue_tracks_past_inputs() {
        let mut queue = InputQueue::<A>::default();
        queue.add(Tick(9), &hist(9, [A(0), A(1), A(2)]));
        queue.add(Tick(9), &hist(13, [A(4)]));

        assert_eq!(queue.next(Tick(10)), Some(A(1)));
        assert_eq!(queue.past.len(), 1);
        assert_eq!(queue.next(Tick(11)), Some(A(2)));
        assert_eq!(queue.past.len(), 2);

        // Repeated inputs don't need to get written
        assert_eq!(queue.next(Tick(12)), Some(A(2)));
        assert_eq!(queue.past.len(), 2);

        assert_eq!(queue.next(Tick(13)), Some(A(4)));
        assert_eq!(queue.past.len(), 3);

        assert_eq!(
            ArrayDeque::from([
                (RepliconTick::new(10), A(1)),
                (RepliconTick::new(11), A(2)),
                (RepliconTick::new(13), A(4))
            ]),
            queue.past
        );
    }
}
