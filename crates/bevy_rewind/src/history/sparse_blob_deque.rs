#![deny(clippy::std_instead_of_alloc)]
#![deny(clippy::std_instead_of_core)]

use super::blob_deque::BlobDeque;

extern crate alloc;
use alloc::alloc::Layout;
use core::num::NonZero;

use bevy::ptr::{OwningPtr, Ptr, PtrMut};

pub(crate) struct SparseBlobDeque {
    mask: u64,
    len: u8,
    capacity: u8,
    items: BlobDeque,
}

impl core::fmt::Debug for SparseBlobDeque {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SparseBlobDeque")
            .field("capacity", &self.capacity)
            .field("len", &self.len)
            .field("mask", &format!("{:01$b}", self.mask, self.len as usize))
            .field("items", &self.items)
            .finish()
    }
}

impl SparseBlobDeque {
    /// SAFETY: The layout and drop function MUST match the type this collection will be used for
    pub(super) unsafe fn new(
        layout: Layout,
        drop: Option<unsafe fn(OwningPtr<'_>)>,
        cap: NonZero<u8>,
    ) -> Self {
        let capacity = cap.get();
        if !(1..=64).contains(&capacity) {
            panic!("SparseBlobDeque capacity MUST be at least 1 and at most 64");
        }
        Self {
            mask: 0,
            len: 0,
            capacity,
            items: BlobDeque::new(layout, drop, unsafe { NonZero::new_unchecked(1) }),
        }
    }

    /// The length of this collection, including the None items
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Check if the collection has no items, including None items
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The capacity of the collection, in sparse entries
    pub fn capacity(&self) -> usize {
        self.capacity as usize
    }

    /// The number of stored items, counting only the Some entries
    pub fn stored_items(&self) -> usize {
        self.items.len()
    }

    /// Get the mask for this collection.
    /// The least significant bit is the back of the collection.
    pub fn mask(&self) -> u64 {
        self.mask
    }

    pub fn mask_mut(&mut self) -> &mut u64 {
        &mut self.mask
    }

    pub fn get<'a>(&'a self, index: usize) -> Option<Ptr<'a>> {
        if index >= self.len as usize {
            return None;
        }
        let index_bit = 1 << (self.len as u64 - 1 - index as u64);
        if self.mask & index_bit == 0 {
            return None;
        }
        let search_mask = !(index_bit - 1);
        let item_index = (self.mask & search_mask).count_ones() - 1;
        self.items.get(item_index as usize)
    }

    pub unsafe fn append<'a>(&mut self, write_fn: Option<impl FnOnce(PtrMut<'a>)>) {
        if self.len == self.capacity {
            let index_bit = 1 << (self.len - 1);
            if self.mask & index_bit != 0 {
                // If the first bit was enabled, there is an item to drop
                self.items.drop_front();
            }
            self.mask &= !index_bit;
            self.len -= 1;
        }

        self.mask = self.mask.wrapping_shl(1);
        if let Some(write_fn) = write_fn {
            if self.items.capacity() == self.items.len() && self.items.capacity() != self.capacity()
            {
                // If we are out of space, allocate enough space for the new item unless we are at capacity
                let new_cap = unsafe { NonZero::new_unchecked(self.items.capacity() as u8 + 1) };
                self.items.resize(new_cap);
            }
            unsafe { self.items.append(write_fn) };
            self.mask |= 1;
        }
        self.len += 1;
    }

    pub fn extend_front(&mut self, n: usize) {
        self.len += (n as u8).min(self.capacity - self.len);
    }

    pub fn extend_back(&mut self, n: usize) {
        if n >= self.capacity() {
            self.items.clear();
            self.mask = 0;
            self.len = self.capacity;
            return;
        }

        let search_mask = ((1u64 << n) - 1).wrapping_shl(self.capacity as u32 - n as u32);
        let ones = (self.mask & search_mask).count_ones();
        for _ in 0..ones {
            self.items.drop_front();
        }

        self.mask = (self.mask & !search_mask).wrapping_shl(n as u32);
        self.len = (self.len + n as u8).min(self.capacity);
    }

    pub fn trim_back(&mut self, n: usize) {
        if n >= self.len() {
            self.clear();
            return;
        }

        let search_mask = (1 << n) - 1;
        let items_to_drop = (self.mask & search_mask).count_ones();
        for _ in 0..items_to_drop {
            self.items.drop_back();
        }
        self.mask = self.mask.wrapping_shr(n as u32);
        self.len -= n as u8;
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.mask = 0;
        self.len = 0;
    }

    pub unsafe fn replace(&mut self, index: usize, write_fn: impl FnOnce(PtrMut)) {
        if index >= self.len() {
            return;
        }

        let index_bit = 1 << (self.len as u64 - 1 - index as u64);
        let search_mask = !(index_bit - 1);
        let ones = (self.mask & search_mask).count_ones();
        if self.mask & index_bit != 0 {
            let drop_fn = self.items.drop();
            // We had an item here, replace it
            if let Some(mut ptr) = self.items.get_mut(ones as usize - 1) {
                drop_fn.inspect(|f| unsafe { f(ptr.reborrow().promote()) });
                write_fn(ptr);
            }
            return;
        }

        if self.items.len() == self.items.capacity() {
            self.items
                .resize(unsafe { NonZero::new_unchecked(self.items.capacity() as u8 + 1) });
        }

        if (self.mask & !search_mask) == 0 {
            self.mask |= index_bit;
            unsafe { self.items.append(write_fn) };
            return;
        }

        self.mask |= index_bit;
        unsafe { self.items.insert(ones as usize, write_fn).unwrap() };
    }
}

#[cfg(test)]
mod tests {
    use core::mem::MaybeUninit;
    use core::num::NonZero;

    use bevy::ptr::PtrMut;

    use super::{super::test_utils::*, SparseBlobDeque};

    #[test]
    fn get() {
        let mut history = SparseBlobDeque::from_type::<A>(NonZero::new(5).unwrap());
        assert_eq!(None, history.get(0).deref::<A>());

        for i in 0..3 {
            if i % 2 == 0 {
                unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut() = A(i * 5))) };
            } else {
                unsafe { history.append(None::<fn(PtrMut)>) };
            }
        }
        unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut() = A(3))) };

        for (i, a) in [Some(&A(0)), None, Some(&A(10)), Some(&A(3)), None].iter_enumerate() {
            assert_eq!(a, history.get(i).deref());
        }
    }

    #[test]
    fn append_full() {
        let mut history = SparseBlobDeque::from_type::<A>(NonZero::new(5).unwrap());

        for i in 0..5 {
            unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut::<A>() = A(i + 1))) };
        }

        assert_eq!(5, history.len());
        assert_eq!(5, history.stored_items());
        for i in 0..5 {
            assert_eq!(Some(&A(i as u16 + 1)), history.get(i).deref::<A>());
        }

        unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut::<A>() = A(6))) };
        assert_eq!(5, history.len());
        assert_eq!(5, history.stored_items());
        for i in 0..5 {
            assert_eq!(Some(&A(i as u16 + 2)), history.get(i).deref::<A>());
        }
    }

    #[test]
    fn dense_storage() {
        let mut history = SparseBlobDeque::from_type::<A>(NonZero::new(10).unwrap());
        assert_eq!(None, history.get(0).deref::<A>());
        assert_eq!(1, history.items.capacity());

        for _ in 0..5 {
            unsafe { history.append(None::<fn(PtrMut)>) };
        }

        // None items shouldn't add capacity
        assert_eq!(5, history.len());
        assert_eq!(1, history.items.capacity());

        unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut() = A(1))) };
        // We shouldn't need to expand yet
        assert_eq!(1, history.items.len());
        assert_eq!(1, history.items.capacity());

        unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut() = A(2))) };
        // Expand to fit just the new item
        assert_eq!(2, history.items.len());
        assert_eq!(2, history.items.capacity());

        for _ in 0..10 {
            unsafe { history.append(None::<fn(PtrMut)>) };
        }

        // We don't release memory if the items are wrapped out of history
        assert_eq!(0, history.items.len());
        assert_eq!(2, history.items.capacity());

        for i in 0..10 {
            unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut() = A(i))) };
        }

        // We should never make it exceed our own capacity
        assert_eq!(10, history.items.len());
        assert_eq!(10, history.items.capacity());
    }

    #[test]
    fn append_get_max_mask() {
        let mut history = SparseBlobDeque::from_type::<A>(NonZero::new(64).unwrap());
        assert_eq!(None, history.get(0).deref::<A>());

        for i in 0..(64 + 24) {
            if i % 2 == 0 {
                unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut() = A(i))) };
            } else {
                unsafe { history.append(None::<fn(PtrMut)>) };
            }
        }

        assert_eq!(64, history.len());

        for i in 0..64 {
            let a = history.get(i);
            if i % 2 == 0 {
                assert_eq!(Some(&A(i as u16 + 24)), a.deref());
            } else {
                assert_eq!(None, a.deref::<A>());
            }
        }
    }

    #[test]
    fn append_sparse_wrap_drops_items() {
        let mut history = SparseBlobDeque::from_type::<D>(NonZero::new(5).unwrap());
        let drops = DropList::default();

        eprintln!("Before");
        for i in 0..6 {
            eprintln!("Appending {i}");
            if i % 2 == 0 {
                unsafe {
                    history.append(Some(|ptr: PtrMut| {
                        ptr.deref_mut::<MaybeUninit<D>>().write(D::new(i, &drops));
                    }));
                };
            } else {
                unsafe { history.append(None::<fn(PtrMut)>) };
            }
            eprintln!("- done {i}");
        }

        assert_eq!(5, history.len());
        assert_eq!(2, history.stored_items());
        assert_drops(&drops, [0]);

        for i in [0, 2, 4, 5] {
            assert_eq!(None, history.get(i).deref::<D>());
        }
        for i in [1, 3] {
            assert_eq!(Some(i as u16 + 1), history.get(i).deref::<D>().map(|v| v.0));
        }

        drop(history);
        assert_drops(&drops, [0, 2, 4]);
    }

    #[test]
    fn append_dense_wrap_drops_items_full() {
        let mut history = SparseBlobDeque::from_type::<D>(NonZero::new(5).unwrap());
        assert_eq!(None, history.get(0).deref::<D>());
        let drops = DropList::default();

        for i in 0..5 {
            unsafe {
                history.append(Some(|ptr: PtrMut| {
                    ptr.deref_mut::<MaybeUninit<D>>()
                        .write(D::new(i + 1, &drops));
                }));
            };
        }

        assert_eq!(5, history.len());
        assert_eq!(5, history.stored_items());
        assert_drops(&drops, []);

        unsafe {
            history.append(Some(|ptr: PtrMut| {
                ptr.deref_mut::<MaybeUninit<D>>().write(D::new(6, &drops));
            }));
        };
        assert_eq!(5, history.len());
        assert_eq!(5, history.stored_items());
        assert_drops(&drops, [1]);

        drop(history);
        assert_drops(&drops, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn extend_front() {
        let mut history = SparseBlobDeque::from_type::<A>(NonZero::new(5).unwrap());

        unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut::<A>() = A(1))) };
        assert_eq!(1, history.len());
        assert_eq!(Some(&A(1)), history.get(0).deref());

        history.extend_front(2);
        assert_eq!(3, history.len());
        for i in 0..2 {
            assert_eq!(None, history.get(i).deref::<A>());
        }
        assert_eq!(Some(&A(1)), history.get(2).deref());

        history.extend_front(7);
        assert_eq!(5, history.len());
        for i in 0..4 {
            assert_eq!(None, history.get(i).deref::<A>());
        }
        assert_eq!(Some(&A(1)), history.get(4).deref());
    }

    #[test]
    fn extend_back() {
        let mut history = SparseBlobDeque::from_type::<A>(NonZero::new(5).unwrap());

        unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut::<A>() = A(1))) };
        assert_eq!(1, history.len());

        // Extend the back without needing to remove anything
        history.extend_back(2);
        assert_eq!(3, history.len());
        assert_eq!(Some(&A(1)), history.get(0).deref());
        for i in 1..4 {
            assert_eq!(None, history.get(i).deref::<A>());
        }

        unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut::<A>() = A(2))) };
        unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut::<A>() = A(3))) };
        assert_eq!(5, history.len());
        assert_eq!(3, history.stored_items());

        // Wrap items out of history with empty items
        history.extend_back(4);
        eprintln!("{:?}", history);
        assert_eq!(5, history.len());
        assert_eq!(1, history.stored_items());
        assert_eq!(Some(&A(3)), history.get(0).deref());
        for i in 1..6 {
            assert_eq!(None, history.get(i).deref::<A>());
        }

        // Wrap more than full capacity
        history.extend_back(7);
        assert_eq!(5, history.len());
        assert_eq!(0, history.stored_items());
        for i in 0..6 {
            assert_eq!(None, history.get(i).deref::<A>());
        }
    }

    #[test]
    fn trim_back() {
        let mut history = SparseBlobDeque::from_type::<A>(NonZero::new(5).unwrap());

        for i in 1..=5 {
            unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut::<A>() = A(i))) };
        }
        assert_eq!(5, history.len());

        history.trim_back(1);
        assert_eq!(4, history.len());
        for (i, v) in (1..=4).iter_enumerate() {
            assert_eq!(Some(&A(v)), history.get(i).deref());
        }
        assert_eq!(None, history.get(4).deref::<A>());

        history.trim_back(2);
        assert_eq!(2, history.len());
        for (i, v) in (1..=2).iter_enumerate() {
            assert_eq!(Some(&A(v)), history.get(i).deref());
        }
        assert_eq!(None, history.get(3).deref::<A>());

        history.extend_back(2);
        assert_eq!(4, history.len());
        for (i, v) in (1..=2).iter_enumerate() {
            assert_eq!(Some(&A(v)), history.get(i).deref());
        }
        for i in 3..5 {
            assert_eq!(None, history.get(i).deref::<A>());
        }

        history.trim_back(1);
        assert_eq!(3, history.len());
        for (i, v) in (1..=2).iter_enumerate() {
            assert_eq!(Some(&A(v)), history.get(i).deref());
        }

        history.trim_back(6);
        assert_eq!(0, history.len());
        for i in 0..6 {
            assert_eq!(None, history.get(i).deref::<A>());
        }
    }

    #[test]
    fn replace() {
        let mut history = SparseBlobDeque::from_type::<A>(NonZero::new(5).unwrap());

        for i in 1..=3 {
            unsafe { history.append(Some(|ptr: PtrMut| *ptr.deref_mut() = A(i))) };
        }

        assert_eq!(3, history.len());
        assert_eq!(3, history.stored_items());

        unsafe { history.replace(1, |ptr| *ptr.deref_mut() = A(5)) };
        for (i, v) in [1, 5, 3].into_iter().enumerate() {
            assert_eq!(Some(&A(v)), history.get(i).deref::<A>());
        }

        unsafe { history.replace(2, |ptr| *ptr.deref_mut() = A(6)) };
        for (i, v) in [1, 5, 6].into_iter().enumerate() {
            assert_eq!(Some(&A(v)), history.get(i).deref::<A>());
        }

        unsafe { history.replace(0, |ptr| *ptr.deref_mut() = A(4)) };
        for (i, v) in (4..=6).enumerate() {
            assert_eq!(Some(&A(v)), history.get(i).deref::<A>());
        }
    }

    #[test]
    fn replace_empty() {
        let mut history = SparseBlobDeque::from_type::<A>(NonZero::new(5).unwrap());

        for _ in 0..3 {
            unsafe { history.append(None::<fn(PtrMut)>) };
        }

        assert_eq!(3, history.len());
        assert_eq!(0, history.stored_items());

        unsafe { history.replace(1, |ptr| *ptr.deref_mut() = A(2)) };
        assert_eq!(Some(&A(2)), history.get(1).deref());
        assert_eq!(None, history.get(0).deref::<A>());
        assert_eq!(None, history.get(2).deref::<A>());

        unsafe { history.replace(2, |ptr| *ptr.deref_mut() = A(3)) };
        assert_eq!(Some(&A(3)), history.get(2).deref::<A>());
        assert_eq!(None, history.get(0).deref::<A>());

        unsafe { history.replace(0, |ptr| *ptr.deref_mut() = A(1)) };

        for i in 0..3 {
            assert_eq!(Some(&A(i as u16 + 1)), history.get(i).deref::<A>());
        }
    }
}
