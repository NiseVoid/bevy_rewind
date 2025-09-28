use super::component::HistoryComponent;
use super::sparse_blob_deque::SparseBlobDeque;

use std::num::NonZero;

use bevy::{
    ecs::component::ComponentId,
    platform::collections::HashMap,
    prelude::{Deref, DerefMut},
    ptr::{Ptr, PtrMut},
};

#[derive(Default, Deref, DerefMut, Debug)]
pub struct EntityHistory {
    components: HashMap<ComponentId, ComponentHistory>,
}

pub struct ComponentHistory {
    removed_mask: u64,
    list: SparseBlobDeque,
    last_tick: u32,
}

impl core::fmt::Debug for ComponentHistory {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ComponentHistory")
            .field("last_tick", &self.last_tick)
            .field(
                "removed_mask",
                &format!("{:01$b}", self.removed_mask, self.list.len()),
            )
            .field("list", &self.list)
            .finish()
    }
}

/// Data for a single tick
#[derive(Debug)]
pub enum TickData<T> {
    /// A value
    Value(T),
    /// The value was removed this tick
    Removed,
    /// There is no data for this tick
    Missing,
}

impl<T: PartialEq> PartialEq for TickData<T> {
    fn eq(&self, other: &Self) -> bool {
        use TickData::*;
        match self {
            Value(t) => match other {
                Value(other) => t == other,
                _ => false,
            },
            Removed => {
                matches!(other, Removed)
            }
            Missing => {
                matches!(other, Missing)
            }
        }
    }
}

impl<T: Eq> Eq for TickData<T> {}

impl<T> TickData<T> {
    // Get the value, if any
    pub fn value(self) -> Option<T> {
        match self {
            TickData::Value(t) => Some(t),
            _ => None,
        }
    }
}

impl<T: Clone> TickData<&T> {
    pub fn cloned(&self) -> TickData<T> {
        use TickData::*;
        match *self {
            Value(t) => Value(t.clone()),
            Removed => Removed,
            Missing => Missing,
        }
    }
}

impl<T: Copy> TickData<&T> {
    pub fn copied(&self) -> TickData<T> {
        use TickData::*;
        match *self {
            Value(&t) => Value(t),
            Removed => Removed,
            Missing => Missing,
        }
    }
}

impl<T> TickData<T> {
    pub fn map<O>(&self, f: impl Fn(&T) -> O) -> TickData<O> {
        match self {
            TickData::Value(t) => TickData::Value(f(t)),
            TickData::Removed => TickData::Removed,
            TickData::Missing => TickData::Missing,
        }
    }
}

impl ComponentHistory {
    pub(crate) fn from_component(component: &HistoryComponent, size: NonZero<u8>) -> Self {
        Self {
            removed_mask: 0,
            list: SparseBlobDeque::from_component(component, size),
            last_tick: 0,
        }
    }

    pub(crate) fn from_type<T: Clone + PartialEq>(size: NonZero<u8>) -> Self {
        Self {
            removed_mask: 0,
            list: SparseBlobDeque::from_type::<T>(size),
            last_tick: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    #[cfg(test)]
    pub fn stored_items(&self) -> usize {
        self.list.stored_items()
    }

    #[cfg(test)]
    pub fn set_last_tick(&mut self, last_tick: u32) {
        self.last_tick = last_tick;
    }

    pub fn first_tick(&self) -> u32 {
        self.last_tick.saturating_sub(
            63u32.saturating_sub((self.removed_mask | self.list.mask()).leading_zeros()),
        )
    }

    pub fn get<'a>(&'a self, tick: u32) -> TickData<Ptr<'a>> {
        if tick > self.last_tick {
            return TickData::Missing;
        }
        let ago = (self.last_tick - tick) as usize;
        if ago >= self.len() {
            return TickData::Missing;
        }
        let index = self.len() - 1 - ago;
        let index_bit = 1 << ago as u64;
        if self.removed_mask & index_bit != 0 {
            return TickData::Removed;
        }

        match self.list.get(index) {
            Some(ptr) => TickData::Value(ptr),
            None => TickData::Missing,
        }
    }

    pub fn get_latest<'a>(&'a self, tick: u32) -> TickData<Ptr<'a>> {
        let ago = self.last_tick.saturating_sub(tick) as usize;
        if ago >= self.len() {
            return TickData::Missing;
        }

        let search_mask = !((1 << ago as u64) - 1);
        let removed_ago = (self.removed_mask & search_mask).trailing_zeros();
        let item_ago = (self.list.mask() & search_mask).trailing_zeros();
        let len = self.list.len() as u32;
        if removed_ago > len && item_ago > len {
            // No removed or items found
            return TickData::Missing;
        }
        if removed_ago <= item_ago {
            return TickData::Removed;
        }

        let index = self.len() - 1 - item_ago as usize;

        match self.list.get(index) {
            Some(ptr) => TickData::Value(ptr),
            None => TickData::Missing,
        }
    }

    // Get the number of empty items after the specified tick
    pub fn empty_after(&self, tick: u32) -> u32 {
        if self.list.is_empty() {
            return 0;
        }
        if tick >= self.last_tick {
            return 64;
        }

        let ago = ((self.last_tick - tick) as usize).min(self.len().saturating_sub(1));
        let search_mask = if ago >= 64 {
            u64::MAX
        } else {
            (1 << (ago as u64)) - 1
        };

        let empty = (self.list.mask() | self.removed_mask) & search_mask;
        empty.leading_zeros() - (64u32.saturating_sub(ago as u32))
    }

    pub unsafe fn write(&mut self, tick: u32, write_fn: impl FnOnce(PtrMut)) {
        self.fill_gaps(tick);

        if !self.list.is_empty() && tick <= self.last_tick {
            let ago = (self.last_tick - tick) as usize;
            if ago >= self.list.capacity() {
                return;
            }
            if ago >= self.list.len() {
                self.list.extend_front(ago - (self.list.len() - 1));
            }

            let index = self.len() - 1 - ago;
            unsafe { self.list.replace(index, write_fn) };
            return;
        }

        if self.list.capacity() == self.list.len() {
            self.trim_front();
        }

        self.removed_mask = self.removed_mask.wrapping_shl(1);
        unsafe { self.list.append(Some(write_fn)) }
        self.last_tick = tick;
    }

    pub fn mark_removed(&mut self, tick: u32) {
        if !self.list.is_empty() && tick <= self.last_tick {
            let ago = (self.last_tick - tick) as usize;
            if ago >= self.list.capacity() {
                return;
            }
            if ago >= self.list.len() {
                self.list.extend_front(ago - (self.list.len() - 1));
            }

            self.removed_mask |= 1 << ago;

            // TODO: Remove item if there was one
            return;
        }

        self.fill_gaps(tick);

        if self.list.capacity() == self.list.len() {
            self.trim_front();
        }

        self.removed_mask = self.removed_mask.wrapping_shl(1) | 1;
        unsafe { self.list.append(None::<fn(PtrMut)>) };
        self.last_tick = tick;
    }

    fn fill_gaps(&mut self, tick: u32) {
        if self.list.is_empty() || tick <= self.last_tick + 1 {
            return;
        }

        let gap = tick - 1 - self.last_tick;

        if gap as usize >= self.list.capacity() {
            // Nothing of the current history fits in the new history

            if self.list.stored_items() == 0 && self.removed_mask == 0 {
                // If there are no items we just need to set the size
                self.list
                    .extend_back((gap as usize).min(self.list.capacity()));
                self.last_tick += gap;
                return;
            }

            // If the last item isn't at the back, move it to the back, then clear the rest
            let newest_item = self.list.mask().trailing_zeros();
            let newest_remove = self.removed_mask.trailing_zeros();
            let newest_bit = newest_item.min(newest_remove);
            if newest_bit != 0 {
                let bits_to_swap = (1 << newest_bit) | 1;

                if newest_item < newest_remove {
                    *self.list.mask_mut() ^= bits_to_swap;
                } else {
                    self.removed_mask = 1;
                }
            }

            let cap_mask = if self.list.capacity() < 64 {
                (1 << self.list.capacity()) - 1
            } else {
                u64::MAX
            };
            let n = self.list.capacity() - 1;
            self.list.extend_back(n);
            self.removed_mask = self.removed_mask.wrapping_shl(n as u32) & cap_mask;

            self.last_tick += gap;
            return;
        }

        if self.list.len() + gap as usize > self.list.capacity() {
            let new_first = self.list.len() + gap as usize - self.list.capacity();
            let retained = self.list.len() - new_first;
            let search_mask = 1 << (retained - 1);
            let has_value =
                (self.removed_mask & search_mask) | (self.list.mask() & search_mask) != 0;

            if !has_value {
                let item_ago = (self.list.mask().wrapping_shr(retained as u32)).trailing_zeros();
                let removed_ago =
                    (self.removed_mask.wrapping_shr(retained as u32)).trailing_zeros();
                if item_ago < 64 || removed_ago < 64 {
                    let to_move = item_ago.min(removed_ago) + 1;
                    let bits_to_swap = 1 << (retained - 1) | 1 << (retained - 1 + to_move as usize);

                    if item_ago < removed_ago {
                        *self.list.mask_mut() ^= bits_to_swap;
                    } else {
                        self.removed_mask ^= bits_to_swap;
                    }
                }
            }
        }

        self.removed_mask = self.removed_mask.wrapping_shl(gap);
        self.list.extend_back(gap as usize);
        self.last_tick += gap;
    }

    fn trim_front(&mut self) {
        let search_mask = 1 << (self.list.len() - 2);
        let has_value = (self.removed_mask & search_mask) | (self.list.mask() & search_mask) != 0;

        if !has_value {
            let retained = self.list.len() - 1;
            let bits_to_swap = 0b11 << (retained - 1);
            if self.list.mask() & (search_mask << 1) != 0 {
                // Swapping item
                *self.list.mask_mut() ^= bits_to_swap;
            } else if self.removed_mask & (search_mask << 1) != 0 {
                // Swapping removed
                self.removed_mask ^= bits_to_swap;
            }
        }
    }

    pub fn clean(&mut self, retain_until: u32) {
        if retain_until >= self.last_tick {
            return;
        }

        let to_drop = self.last_tick - retain_until;
        if to_drop >= self.len() as u32 {
            self.list.clear();
            self.last_tick = retain_until;
            return;
        }
        self.removed_mask = self.removed_mask.wrapping_shr(to_drop);
        self.list.trim_back(to_drop as usize);
        self.last_tick -= to_drop;
    }

    pub fn keep_first_item(&mut self) {
        if self.list.stored_items() == 0 {
            return;
        }

        let zeros = self.list.mask().leading_zeros();
        let ago = 63 - zeros;
        self.clean(self.last_tick.saturating_sub(ago));
    }
}

#[cfg(test)]
mod tests {
    use bevy::ptr::PtrMut;

    use super::{super::test_utils::*, ComponentHistory, TickData::*};
    use crate::history::component::HistoryComponent;

    use std::num::NonZero;

    #[test]
    fn append() {
        let a = HistoryComponent::new::<A>();
        let mut history = ComponentHistory::from_component(&a, NonZero::new(5).unwrap());
        assert_eq!(0, history.len());

        unsafe { history.write(0, |ptr| *ptr.deref_mut() = A(1)) };
        assert_eq!(1, history.len());
        unsafe { history.write(1, |ptr| *ptr.deref_mut() = A(2)) };
        assert_eq!(2, history.len());
        unsafe { history.write(2, |ptr| *ptr.deref_mut() = A(3)) };
        assert_eq!(3, history.len());

        assert_eq!(Value(&A(1)), history.get(0).deref());
        assert_eq!(Value(&A(2)), history.get(1).deref());
        assert_eq!(Value(&A(3)), history.get(2).deref());
        assert_eq!(Missing, history.get(3).deref::<A>());
    }

    #[test]
    fn get_latest() {
        let a = HistoryComponent::new::<A>();
        let mut history = ComponentHistory::from_component(&a, NonZero::new(5).unwrap());
        assert_eq!(0, history.len());

        unsafe { history.write(0, |ptr| *ptr.deref_mut() = A(1)) };
        unsafe { history.write(4, |ptr| *ptr.deref_mut() = A(2)) };
        assert_eq!(5, history.len());

        for i in 0..=3 {
            assert_eq!(Value(&A(1)), history.get_latest(i).deref());
        }

        history.mark_removed(1);
        for i in 1..=3 {
            assert_eq!(Removed, history.get_latest(i).deref::<A>());
        }
    }

    #[test]
    fn start_non_zero_tick() {
        let a = HistoryComponent::new::<A>();
        let mut history = ComponentHistory::from_component(&a, NonZero::new(5).unwrap());
        assert_eq!(0, history.len());

        unsafe { history.write(25, |ptr| *ptr.deref_mut() = A(1)) };
        assert_eq!(1, history.len());
        assert_eq!(25, history.last_tick);

        assert_eq!(Missing, history.get(24).deref::<A>());
        assert_eq!(Value(&A(1)), history.get(25).deref::<A>());
        assert_eq!(Missing, history.get(26).deref::<A>());
    }

    #[test]
    fn repeated_tick() {
        let a = HistoryComponent::new::<A>();
        let mut history = ComponentHistory::from_component(&a, NonZero::new(5).unwrap());
        assert_eq!(0, history.len());

        // Write some initial data
        unsafe { history.write(0, |ptr| *ptr.deref_mut() = A(1)) };
        unsafe { history.write(1, |ptr| *ptr.deref_mut() = A(2)) };
        assert_eq!(2, history.len());

        // Write to ticks already written
        unsafe { history.write(1, |ptr| *ptr.deref_mut() = A(4)) };
        assert_eq!(2, history.len());
        unsafe { history.write(0, |ptr| *ptr.deref_mut() = A(3)) };
        assert_eq!(2, history.len());

        assert_eq!(Value(&A(3)), history.get(0).deref());
        assert_eq!(Value(&A(4)), history.get(1).deref());
        assert_eq!(Missing, history.get(2).deref::<A>());
    }

    #[test]
    fn gaps() {
        let a = HistoryComponent::new::<A>();
        let mut history = ComponentHistory::from_component(&a, NonZero::new(5).unwrap());
        assert_eq!(0, history.len());

        unsafe { history.write(0, |ptr| *ptr.deref_mut() = A(1)) };
        // Tick 1 is never written
        unsafe { history.write(2, |ptr| *ptr.deref_mut() = A(2)) };

        assert_eq!(3, history.len());
        assert_eq!(2, history.stored_items());

        assert_eq!(Value(&A(1)), history.get(0).deref());
        assert_eq!(Missing, history.get(1).deref::<A>());
        assert_eq!(Value(&A(2)), history.get(2).deref());
        assert_eq!(Missing, history.get(3).deref::<A>());
    }

    #[test]
    fn wrap_retains_first_value() {
        let a = HistoryComponent::new::<A>();
        let mut history = ComponentHistory::from_component(&a, NonZero::new(5).unwrap());
        assert_eq!(0, history.len());

        unsafe { history.write(0, |ptr| *ptr.deref_mut() = A(1)) };
        // Tick 1-3 are never written
        unsafe { history.write(4, |ptr| *ptr.deref_mut() = A(2)) };
        // Tick 5 is never written
        unsafe { history.write(6, |ptr| *ptr.deref_mut() = A(3)) };

        assert_eq!(5, history.len());
        assert_eq!(3, history.stored_items());
        // The first item was moved to tick 2 to retain a valid value
        assert_eq!(Value(&A(1)), history.get(2).deref());
        assert_eq!(Value(&A(2)), history.get(4).deref());
        assert_eq!(Value(&A(3)), history.get(6).deref());
        for i in [1, 3, 5] {
            assert_eq!(Missing, history.get(i).deref::<A>());
        }
    }

    #[test]
    fn wrap_with_removed() {
        let a = HistoryComponent::new::<A>();
        let mut history = ComponentHistory::from_component(&a, NonZero::new(5).unwrap());
        assert_eq!(0, history.len());

        history.mark_removed(0);
        // Tick 1-4 are never written
        unsafe { history.write(5, |ptr| *ptr.deref_mut() = A(1)) };

        assert_eq!(5, history.len());
        assert_eq!(1, history.stored_items());
        // The Removed was moved to tick 2 to retain a valid value
        assert_eq!(Removed, history.get(1).deref::<A>());
        assert_eq!(Value(&A(1)), history.get(5).deref());
        for i in [0, 2, 3, 4, 6] {
            assert_eq!(Missing, history.get(i).deref::<A>());
        }
    }

    #[test]
    fn wrap_more_than_capacity() {
        let a = HistoryComponent::new::<A>();
        let mut history = ComponentHistory::from_component(&a, NonZero::new(20).unwrap());
        assert_eq!(0, history.len());

        history.mark_removed(0);
        // Tick 1-80 are never written
        unsafe { history.write(81, |ptr| *ptr.deref_mut() = A(1)) };

        assert_eq!(20, history.len());
        assert_eq!(1, history.stored_items());
        // The Removed was moved to tick 62 to retain a valid value in the gap
        assert_eq!(Removed, history.get(62).deref::<A>());
        assert_eq!(Value(&A(1)), history.get(81).deref());

        // Tick 82-119 are never written
        history.mark_removed(120);
        // The value was moved to tick 101 to retain a valid value in the gap
        assert_eq!(Value(&A(1)), history.get(101).deref());
        assert_eq!(Removed, history.get(120).deref::<A>());
    }

    #[test]
    fn out_of_order() {
        let a = HistoryComponent::new::<A>();
        let mut history = ComponentHistory::from_component(&a, NonZero::new(5).unwrap());
        assert_eq!(0, history.len());

        // Data is written out of order
        unsafe { history.write(2, |ptr| *ptr.deref_mut() = A(3)) };
        unsafe { history.write(1, |ptr| *ptr.deref_mut() = A(2)) };
        unsafe { history.write(3, |ptr| *ptr.deref_mut() = A(4)) };
        unsafe { history.write(0, |ptr| *ptr.deref_mut() = A(1)) };
        assert_eq!(4, history.len());

        assert_eq!(Value(&A(1)), history.get(0).deref());
        assert_eq!(Value(&A(2)), history.get(1).deref());
        assert_eq!(Value(&A(3)), history.get(2).deref());
        assert_eq!(Value(&A(4)), history.get(3).deref());
        assert_eq!(Missing, history.get(4).deref::<A>());
    }

    #[test]
    fn clean() {
        let a = HistoryComponent::new::<A>();
        let mut history = ComponentHistory::from_component(&a, NonZero::new(5).unwrap());

        unsafe { history.write(0, |ptr| *ptr.deref_mut() = A(1)) };
        history.mark_removed(2);
        unsafe { history.write(3, |ptr| *ptr.deref_mut() = A(2)) };
        assert_eq!(4, history.len());
        assert_eq!(2, history.stored_items());

        // Target the last tick, this shouldn't do anything
        history.clean(3);
        assert_eq!(4, history.len());
        assert_eq!(2, history.stored_items());

        // Target tick 2, which should only remove data for ticks after it
        history.clean(2);
        assert_eq!(3, history.len());
        assert_eq!(1, history.stored_items());

        assert_eq!(Value(&A(1)), history.get(0).deref());
        assert_eq!(Missing, history.get(1).deref::<A>());
        assert_eq!(Removed, history.get(2).deref::<A>());
        assert_eq!(Missing, history.get(3).deref::<A>());

        // Cleaning should also remove gaps and removed
        history.clean(0);
        assert_eq!(1, history.len());
        assert_eq!(1, history.stored_items());
        assert_eq!(0, history.removed_mask);

        assert_eq!(Value(&A(1)), history.get(0).deref());
        for i in 1..=3 {
            assert_eq!(Missing, history.get(i).deref::<A>());
        }

        for i in 5..=9 {
            unsafe { history.write(i, |ptr| *ptr.deref_mut() = A(i as u16)) };
        }
        assert_eq!(5, history.len());
        assert_eq!(5, history.stored_items());

        // Target a tick before all items
        history.clean(4);
        assert_eq!(0, history.len());
        assert_eq!(0, history.stored_items());
    }

    #[test]
    fn keep_first_item() {
        let a = HistoryComponent::new::<A>();
        let mut history = ComponentHistory::from_component(&a, NonZero::new(5).unwrap());

        unsafe { history.list.append(None::<fn(PtrMut)>) };
        assert_eq!(1, history.len());

        // Calling keep_first_item on a history with only Missing should do nothing
        history.keep_first_item();
        assert_eq!(1, history.len());

        history.mark_removed(1);
        assert_eq!(2, history.len());

        // Calling keep_first_item on a history with only Missing and Removed should do nothing
        history.keep_first_item();
        assert_eq!(2, history.len());

        unsafe { history.write(2, |ptr| *ptr.deref_mut() = A(1)) };
        history.mark_removed(3);
        unsafe { history.write(4, |ptr| *ptr.deref_mut() = A(2)) };
        assert_eq!(5, history.len());

        // Calling keep_first_item on a history with multiple items should keep only the first one
        history.keep_first_item();
        assert_eq!(3, history.len());

        assert_eq!(Missing, history.get(0).deref::<A>());
        assert_eq!(Removed, history.get(1).deref::<A>());
        assert_eq!(Value(&A(1)), history.get(2).deref());
    }

    #[test]
    fn empty_after() {
        let a = HistoryComponent::new::<A>();
        let mut history = ComponentHistory::from_component(&a, NonZero::new(64).unwrap());
        assert_eq!(0, history.len());
        assert_eq!(0, history.empty_after(0));

        // Start with a None at index 0
        unsafe { history.list.append(None::<fn(PtrMut)>) };
        // Ticks at or after the end are always considered to have an arbitrary number of trailing empties
        assert_eq!(64, history.empty_after(0));
        assert_eq!(64, history.empty_after(1));

        unsafe { history.write(3, |ptr| *ptr.deref_mut() = A(1)) };
        assert_eq!(2, history.empty_after(0));
        assert_eq!(1, history.empty_after(1));
        assert_eq!(0, history.empty_after(2));
        for i in 3..=4 {
            assert_eq!(64, history.empty_after(i));
        }

        unsafe { history.write(20, |ptr| *ptr.deref_mut() = A(2)) };
        assert_eq!(1, history.empty_after(1));
        assert_eq!(16, history.empty_after(3));
        assert_eq!(1, history.empty_after(18));
        assert_eq!(0, history.empty_after(19));
        for i in 20..=21 {
            assert_eq!(64, history.empty_after(i));
        }

        history.mark_removed(25);
        assert_eq!(3, history.empty_after(21));
        assert_eq!(1, history.empty_after(23));
        assert_eq!(0, history.empty_after(24));
        for i in 25..=26 {
            assert_eq!(64, history.empty_after(i));
        }

        unsafe { history.write(64, |ptr| *ptr.deref_mut() = A(3)) };
        assert_eq!(37, history.empty_after(26));
        assert_eq!(1, history.empty_after(62));
        assert_eq!(0, history.empty_after(63));
        for i in 64..=65 {
            assert_eq!(64, history.empty_after(i));
        }

        // Index 0 has wrapped, and should count from the start which is now tick 1
        assert_eq!(1, history.empty_after(0));
        assert_eq!(1, history.empty_after(1));
    }
}
