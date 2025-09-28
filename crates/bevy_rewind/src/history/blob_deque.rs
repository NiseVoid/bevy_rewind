#![deny(clippy::std_instead_of_alloc)]
#![deny(clippy::std_instead_of_core)]

extern crate alloc;
use alloc::alloc::{Layout, alloc, dealloc, handle_alloc_error};
use core::{fmt::Write, num::NonZero, ptr::NonNull};

use bevy::ptr::{OwningPtr, Ptr, PtrMut};

/// A blobby ring buffer with support for gaps
pub struct BlobDeque {
    /// The memory layout of each item
    layout: Layout,
    /// Capacity in items, not bytes
    capacity: u8,
    /// The length in items, not bytes
    len: u8,
    /// The start of the ringbuffer in items, not bytes
    start: u8,
    /// The ring buffer's data
    data: NonNull<u8>,
    /// The function to drop items, if any
    drop: Option<unsafe fn(OwningPtr<'_>)>,
}

impl core::fmt::Debug for BlobDeque {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut items = String::new();
        items.write_char('[')?;

        let size = self.layout.size();
        for i in 0..(self.len as usize) {
            if i != 0 {
                items.write_str(", ")?;
            }
            if size == 0 {
                items.write_char('-')?;
            } else {
                items.write_str("0x")?;
                let ptr = self.get(i).unwrap();
                for offset in 0..size {
                    write!(items, "{:02x}", unsafe {
                        ptr.byte_add(offset).as_ptr().read()
                    },)?;
                }
            }
        }

        items.write_char(']')?;

        f.debug_struct("BlobDeque")
            .field("capacity", &self.capacity)
            .field("len", &self.len)
            .field("start", &self.start)
            .field("items", &items)
            .finish()
    }
}

unsafe impl Send for BlobDeque {}
unsafe impl Sync for BlobDeque {}

impl BlobDeque {
    pub(crate) fn new(
        layout: Layout,
        drop: Option<unsafe fn(OwningPtr<'_>)>,
        size: NonZero<u8>,
    ) -> Self {
        if layout.size() == 0 {
            let align = NonZero::<usize>::new(layout.align()).expect("alignment must be > 0");
            Self {
                layout,
                capacity: size.get(),
                len: 0,
                start: 0,
                data: bevy::ptr::dangling_with_align(align),
                drop,
            }
        } else {
            let data = alloc_items(&layout, size.get() as usize);
            Self {
                layout,
                capacity: size.get(),
                len: 0,
                start: 0,
                data,
                drop,
            }
        }
    }

    /// Get the length of the `BlobDeque`
    pub fn len(&self) -> usize {
        self.len as usize
    }

    #[allow(dead_code)]
    /// Check if the `BlobDeque` has no items
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Get the capacity of the `BlobDeque`
    pub fn capacity(&self) -> usize {
        self.capacity as usize
    }

    pub fn drop(&self) -> Option<unsafe fn(OwningPtr<'_>)> {
        self.drop
    }

    pub(crate) fn get<'a>(&'a self, index: usize) -> Option<Ptr<'a>> {
        if (self.len as usize) < index + 1 {
            return None;
        }
        let size = self.layout.size();
        if size == 0 {
            return Some(unsafe { Ptr::new(self.data) });
        }
        let offset = self.get_offset(index);
        Some(unsafe { Ptr::new(self.data).byte_add(offset) })
    }

    pub(crate) fn get_mut<'a>(&'a mut self, index: usize) -> Option<PtrMut<'a>> {
        let size = self.layout.size();
        if size == 0 || (self.len as usize) < index + 1 {
            // size 0 cannot be mutated
            return None;
        }
        let offset = self.get_offset(index);
        Some(unsafe { PtrMut::new(self.data).byte_add(offset) })
    }

    fn get_offset(&self, index: usize) -> usize {
        ((self.start as usize + index) % self.capacity as usize) * self.layout.size()
    }

    pub(crate) fn drop_front(&mut self) {
        if self.len == 0 {
            return;
        }

        if self.layout.size() != 0 {
            self.drop
                .inspect(|f| unsafe { f(self.get_mut(0).unwrap_unchecked().promote()) });
            self.start = (self.start + 1) % self.capacity;
        }
        self.len -= 1;
    }

    pub(crate) fn drop_back(&mut self) {
        if self.len == 0 {
            return;
        }

        if self.layout.size() != 0 {
            self.drop.inspect(|f| unsafe {
                f(self.get_mut(self.len() - 1).unwrap_unchecked().promote());
            });
        }
        self.len -= 1;
    }

    /// SAFETY:
    /// - The value written in `write_fn` MUST match the type the `BlobDeque` was made for
    /// - `write_fn` MUST write to the [`PtrMut`], or the value will be uninitialized
    pub(crate) unsafe fn append<'a>(&mut self, write_fn: impl FnOnce(PtrMut<'a>)) {
        if let Some(ptr) = unsafe { self.new_ptr() } {
            write_fn(ptr);
        }
    }

    unsafe fn new_ptr<'a>(&mut self) -> Option<PtrMut<'a>> {
        if self.layout.size() == 0 {
            self.len = (self.len + 1).min(self.capacity);
            return None;
        }
        if self.len == self.capacity {
            self.drop
                .inspect(|f| unsafe { f(self.get_mut(0).unwrap_unchecked().promote()) });
            self.len -= 1;
            self.start = (self.start + 1) % self.capacity;
        }
        let offset = self.get_offset(self.len as usize);

        self.len += 1;
        Some(unsafe { PtrMut::new(self.data).byte_add(offset) })
    }

    // TODO: Return capacity error instead of Option
    #[must_use]
    pub(crate) unsafe fn insert<'a>(
        &mut self,
        at: usize,
        write_fn: impl FnOnce(PtrMut<'a>),
    ) -> Option<()> {
        if let Some(maybe_ptr) = unsafe { self.new_ptr_at(at) } {
            if let Some(ptr) = maybe_ptr {
                write_fn(ptr);
            }
            Some(())
        } else {
            None
        }
    }

    unsafe fn new_ptr_at<'a>(&mut self, at: usize) -> Option<Option<PtrMut<'a>>> {
        if self.len == self.capacity || at > self.len() {
            return None;
        }

        let size = self.layout.size();
        if size == 0 {
            self.len = (self.len + 1).min(self.capacity);
            return Some(None);
        }

        if at == self.len() {
            // No op
        } else if at == 0 {
            if self.start == 0 {
                self.start = self.capacity - 1;
            } else {
                self.start -= 1;
            }
        } else {
            if self.capacity - self.len < self.start {
                // Shift the wrapped part of the buffer forward by one item
                let first_half = (self.capacity - self.start) as usize;

                let raw_pos = at.saturating_sub(first_half);
                let to_move = self.len() - first_half - raw_pos;
                unsafe {
                    core::ptr::copy(
                        self.data.byte_add(raw_pos * size).as_ptr(),
                        self.data.byte_add((raw_pos + 1) * size).as_ptr(),
                        to_move * size,
                    );
                }
            }

            if self.start as usize + at < self.capacity() {
                if self.start != 0 && self.capacity - self.len <= self.start {
                    // Move the item at the end to the front of the memory (before start)
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            self.data.byte_add((self.capacity() - 1) * size).as_ptr(),
                            self.data.as_ptr(),
                            size,
                        );
                    }
                }

                let move_start = self.start as usize + at;
                let to_move = self.capacity() - move_start - 1;

                if to_move != 0 {
                    unsafe {
                        core::ptr::copy(
                            self.data.byte_add(move_start * size).as_ptr(),
                            self.data.byte_add((move_start + 1) * size).as_ptr(),
                            to_move * size,
                        );
                    }
                }
            }
        }

        let offset = self.get_offset(at);

        self.len += 1;
        Some(Some(unsafe { PtrMut::new(self.data).byte_add(offset) }))
    }

    pub fn resize(&mut self, capacity: NonZero<u8>) {
        let capacity = capacity.get();
        if capacity == self.capacity {
            return;
        }

        let size = self.layout.size();
        let lost = self.len.saturating_sub(capacity);

        if size == 0 {
            self.len = self.len.min(capacity);
            self.capacity = capacity;
            return;
        }

        if lost > 0 {
            if let Some(drop) = self.drop {
                for i in 0..lost {
                    let item = unsafe { self.get_mut(i as usize).unwrap_unchecked().promote() };
                    unsafe { drop(item) };
                }
            }
            self.len -= lost;
            self.start += lost;
        }

        let new_data = alloc_items(&self.layout, capacity as usize);

        let start = self.start;
        let overflow = start.saturating_sub(self.capacity);
        let first_part = self.capacity.saturating_sub(start).min(capacity);
        if first_part > 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.data.byte_add(start as usize * size).as_ptr(),
                    new_data.as_ptr(),
                    first_part as usize * size,
                );
            }
        }
        if self.start != 0 && capacity > first_part && self.len > first_part {
            let l = capacity.min(self.len) - first_part;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.data.byte_add(overflow as usize * size).as_ptr(),
                    new_data.byte_add(first_part as usize * size).as_ptr(),
                    l as usize * size,
                );
            }
        }

        let layout = array_layout(&self.layout, self.capacity as usize).unwrap();
        unsafe { dealloc(self.data.as_ptr(), layout) };

        self.data = new_data;
        self.capacity = capacity;
        self.start = 0;
    }

    pub fn clear(&mut self) {
        if self.layout.size() == 0 {
            self.len = 0;
            return;
        }

        if let Some(drop) = self.drop {
            for i in 0..self.len {
                let item = unsafe { self.get_mut(i as usize).unwrap_unchecked().promote() };
                unsafe { drop(item) };
            }
        }
        self.len = 0;
        self.start = 0;
    }
}

impl Drop for BlobDeque {
    fn drop(&mut self) {
        self.clear();

        if self.layout.size() > 0 {
            let layout = array_layout(&self.layout, self.capacity as usize).unwrap();
            unsafe { dealloc(self.data.as_ptr(), layout) };
        }

        self.capacity = 0;
    }
}

fn alloc_items(layout: &Layout, size: usize) -> NonNull<u8> {
    let array_layout = array_layout(layout, size).unwrap();
    let data = unsafe { alloc(array_layout) };
    let Some(data) = NonNull::new(data) else {
        handle_alloc_error(*layout)
    };
    data
}

/// From <https://doc.rust-lang.org/beta/src/core/alloc/layout.rs.html>
pub(super) fn array_layout(layout: &Layout, n: usize) -> Option<Layout> {
    let (array_layout, offset) = repeat_layout(layout, n)?;
    debug_assert_eq!(layout.size(), offset);
    Some(array_layout)
}

// TODO: replace with `Layout::repeat` if/when it stabilizes
/// From <https://doc.rust-lang.org/beta/src/core/alloc/layout.rs.html>
fn repeat_layout(layout: &Layout, n: usize) -> Option<(Layout, usize)> {
    // This cannot overflow. Quoting from the invariant of Layout:
    // > `size`, when rounded up to the nearest multiple of `align`,
    // > must not overflow (i.e., the rounded value must be less than
    // > `usize::MAX`)
    let padded_size = layout.size() + padding_needed_for(layout, layout.align());
    let alloc_size = padded_size.checked_mul(n)?;

    // SAFETY: self.align is already known to be valid and alloc_size has been
    // padded already.
    unsafe {
        Some((
            Layout::from_size_align_unchecked(alloc_size, layout.align()),
            padded_size,
        ))
    }
}

/// From <https://doc.rust-lang.org/beta/src/core/alloc/layout.rs.html>
const fn padding_needed_for(layout: &Layout, align: usize) -> usize {
    let len = layout.size();

    // Rounded up value is:
    //   len_rounded_up = (len + align - 1) & !(align - 1);
    // and then we return the padding difference: `len_rounded_up - len`.
    //
    // We use modular arithmetic throughout:
    //
    // 1. align is guaranteed to be > 0, so align - 1 is always
    //    valid.
    //
    // 2. `len + align - 1` can overflow by at most `align - 1`,
    //    so the &-mask with `!(align - 1)` will ensure that in the
    //    case of overflow, `len_rounded_up` will itself be 0.
    //    Thus the returned padding, when added to `len`, yields 0,
    //    which trivially satisfies the alignment `align`.
    //
    // (Of course, attempts to allocate blocks of memory whose
    // size and padding overflow in the above manner should cause
    // the allocator to yield an error anyway.)

    let len_rounded_up = len.wrapping_add(align).wrapping_sub(1) & !align.wrapping_sub(1);
    len_rounded_up.wrapping_sub(len)
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use alloc::alloc::Layout;
    use core::mem::MaybeUninit;
    use core::num::NonZero;

    use super::{super::test_utils::*, BlobDeque};

    #[test]
    fn get_in_bounds() {
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(5).unwrap());

        for i in 1..=5 {
            unsafe { history.append(|ptr| *ptr.deref_mut() = A(i)) };
        }

        for i in 0..5 {
            assert_eq!(Some(&A(i as u16 + 1)), history.get(i).deref::<A>());
        }
    }

    #[test]
    fn get_out_of_bounds() {
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(5).unwrap());

        for i in 1..=4 {
            unsafe { history.append(|ptr| *ptr.deref_mut() = A(i)) };
        }

        // Out of bounds, within capacity
        assert_eq!(None, history.get(4).deref::<A>());
        // Out of bounds and out of capacity
        assert_eq!(None, history.get(5).deref::<A>());
    }

    #[test]
    fn get_mut_in_bounds() {
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(5).unwrap());

        for i in 1..=5 {
            unsafe { history.append(|ptr| *ptr.deref_mut() = A(i)) };
        }

        for i in 0..5 {
            assert_eq!(Some(&mut A(i as u16 + 1)), history.get_mut(i).deref::<A>());
        }
    }

    #[test]
    fn get_mut_out_of_bounds() {
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(5).unwrap());

        for i in 1..=4 {
            unsafe { history.append(|ptr| *ptr.deref_mut() = A(i)) };
        }

        // Out of bounds, within capacity
        assert_eq!(None, history.get_mut(4).deref::<A>());
        // Out of bounds and out of capacity
        assert_eq!(None, history.get_mut(5).deref::<A>());
    }

    #[test]
    fn get_mut_zst_is_none() {
        let mut history = BlobDeque::new(Layout::new::<B>(), None, NonZero::new(5).unwrap());

        for _ in 1..=5 {
            unsafe { history.append(|_| {}) };
        }

        for i in 0..=6 {
            assert_eq!(None, history.get_mut(i).deref::<B>());
        }
    }

    #[test]
    fn append_get_sized() {
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(5).unwrap());

        assert_eq!(None, unsafe { history.get(0).map(|v| v.deref::<A>()) });

        unsafe { history.append(|ptr| *ptr.deref_mut() = A(1)) };
        assert_eq!(Some(&A(1)), unsafe { history.get(0).map(|v| v.deref()) });
        assert_eq!(None, unsafe { history.get(1).map(|v| v.deref::<A>()) });

        unsafe { history.append(|ptr| *ptr.deref_mut() = A(2)) };
        assert_eq!(Some(&A(1)), unsafe { history.get(0).map(|v| v.deref()) });
        assert_eq!(Some(&A(2)), unsafe { history.get(1).map(|v| v.deref()) });
        assert_eq!(None, unsafe { history.get(2).map(|v| v.deref::<A>()) });
    }

    #[test]
    fn append_get_zst() {
        let mut history = BlobDeque::new(Layout::new::<B>(), None, NonZero::new(5).unwrap());

        assert_eq!(None, history.get(0).map(|v| unsafe { v.deref::<B>() }));
        assert_eq!(None, history.get(1).map(|v| unsafe { v.deref::<B>() }));

        unsafe { history.append(|_| {}) };
        assert_eq!(Some(&B), history.get(0).map(|v| unsafe { v.deref() }));
        assert_eq!(None, history.get(1).map(|v| unsafe { v.deref::<B>() }));

        unsafe { history.append(|_| {}) };
        assert_eq!(Some(&B), history.get(0).map(|v| unsafe { v.deref() }));
        assert_eq!(Some(&B), history.get(1).map(|v| unsafe { v.deref() }));
        assert_eq!(None, history.get(2).map(|v| unsafe { v.deref::<B>() }));
    }

    #[test]
    fn append_get_alignment() {
        let mut history = BlobDeque::new(Layout::new::<C>(), None, NonZero::new(5).unwrap());

        assert_eq!(None, history.get(0).map(|v| unsafe { v.deref::<C>() }));
        assert_eq!(None, history.get(1).map(|v| unsafe { v.deref::<C>() }));

        unsafe { history.append(|ptr| *ptr.deref_mut() = C(1, 2)) };
        assert_eq!(Some(&C(1, 2)), history.get(0).map(|v| unsafe { v.deref() }));
        assert_eq!(None, history.get(1).map(|v| unsafe { v.deref::<C>() }));

        unsafe { history.append(|ptr| *ptr.deref_mut() = C(4, 3)) };
        assert_eq!(Some(&C(1, 2)), history.get(0).map(|v| unsafe { v.deref() }));
        assert_eq!(Some(&C(4, 3)), history.get(1).map(|v| unsafe { v.deref() }));
        assert_eq!(None, history.get(2).map(|v| unsafe { v.deref::<C>() }));
    }

    #[test]
    fn wraps() {
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(3).unwrap());

        for i in 1..=5 {
            // Write 1, 2, 3, 4, 5
            unsafe { history.append(|ptr| *ptr.deref_mut() = A(i)) };
        }
        // Only 3, 4, 5 should be in the list
        assert_eq!(3, history.len);
        assert_eq!(3, history.capacity);
        assert_eq!(Some(&A(3)), history.get(0).map(|v| unsafe { v.deref() }));
        assert_eq!(Some(&A(4)), history.get(1).map(|v| unsafe { v.deref() }));
        assert_eq!(Some(&A(5)), history.get(2).map(|v| unsafe { v.deref() }));
        assert_eq!(None, history.get(3).map(|v| unsafe { v.deref::<A>() }));
    }

    #[test]
    fn wraps_zst() {
        let mut history = BlobDeque::new(Layout::new::<B>(), None, NonZero::new(3).unwrap());

        for _ in 1..=20 {
            unsafe { history.append(|_| {}) };
        }
        // Only 3 values should be in the history
        assert_eq!(3, history.len);
        assert_eq!(3, history.capacity);
        assert_eq!(Some(&B), history.get(0).map(|v| unsafe { v.deref() }));
        assert_eq!(Some(&B), history.get(1).map(|v| unsafe { v.deref() }));
        assert_eq!(Some(&B), history.get(2).map(|v| unsafe { v.deref() }));
        assert_eq!(None, history.get(3).map(|v| unsafe { v.deref::<A>() }));
    }

    #[test]
    fn wraps_many_times() {
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(3).unwrap());

        for i in 0..100 {
            unsafe { history.append(|ptr| *ptr.deref_mut() = A(i)) };
        }
        assert_eq!(Some(&A(97)), history.get(0).map(|v| unsafe { v.deref() }));
        assert_eq!(Some(&A(98)), history.get(1).map(|v| unsafe { v.deref() }));
        assert_eq!(Some(&A(99)), history.get(2).map(|v| unsafe { v.deref() }));
        assert_eq!(None, history.get(3).map(|v| unsafe { v.deref::<A>() }));
    }

    #[test]
    fn insert_trivial() {
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(5).unwrap());

        // Add the item to the back
        unsafe { history.insert(0, |ptr| *ptr.deref_mut() = A(2)).unwrap() };
        // Add the item to the front
        unsafe { history.insert(0, |ptr| *ptr.deref_mut() = A(1)).unwrap() };
        // Add the item to the back, but this time the list isn't empty
        unsafe { history.insert(2, |ptr| *ptr.deref_mut() = A(3)).unwrap() };

        assert_eq!(3, history.len());
        for i in 0..3 {
            assert_eq!(
                Some(&A(i as u16 + 1)),
                history.get(i).map(|v| unsafe { v.deref() })
            );
        }
        assert_eq!(None, history.get(3).map(|v| unsafe { v.deref::<A>() }));
    }

    #[test]
    fn insert_errors() {
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(5).unwrap());

        unsafe { history.append(|ptr| *ptr.deref_mut() = A(1)) };

        // Not connected to current items
        let res = unsafe { history.insert(2, |_| {}) };
        assert!(res.is_none());

        for _ in 0..4 {
            unsafe { history.append(|ptr| *ptr.deref_mut() = A(1)) };
        }

        // No capacity
        let res = unsafe { history.insert(0, |_| {}) };
        assert!(res.is_none());
    }

    #[test]
    fn insert_moves() {
        // Check both at wrapping capacity and some space over
        for cap in 7..=8 {
            // Check at all start positions to make sure we hit every move condition
            for start in 0..cap {
                insert_move_with_start(start, cap);
            }
        }
    }

    fn insert_move_with_start(start: u8, cap: u8) {
        let case_str = format!("Case: start {}, cap {}", start, cap);
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(cap).unwrap());

        history.start = start;
        for i in [1, 2, 3, 5, 6, 7] {
            unsafe { history.append(|ptr| *ptr.deref_mut() = A(i)) };
        }

        // Insert an item in between
        unsafe { history.insert(3, |ptr| *ptr.deref_mut() = A(4)).unwrap() };

        assert_eq!(7, history.len(), "{}", case_str);
        for i in 0..7 {
            assert_eq!(
                Some(&A(i as u16 + 1)),
                history.get(i).map(|v| unsafe { v.deref() }),
                "{}",
                case_str
            );
        }
        assert_eq!(
            None,
            history.get(7).map(|v| unsafe { v.deref::<A>() }),
            "{}",
            case_str
        );
    }

    #[test]
    fn shrink() {
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(5).unwrap());
        let old_ptr = history.data;

        // We write enough values so we can test items getting removed after shrinking
        for i in 1..=5 {
            unsafe { history.append(|ptr| *ptr.deref_mut() = A(i)) };
        }
        assert_eq!(5, history.len);
        assert_eq!(5, history.capacity);
        assert_eq!(Some(&A(1)), history.get(0).map(|v| unsafe { v.deref() }));

        history.resize(NonZero::new(3).unwrap());

        assert_ne!(old_ptr, history.data);
        assert_eq!(3, history.len);
        assert_eq!(3, history.capacity);
        assert_eq!(0, history.start);

        // We should only have the last 3 values
        for (i, v) in (3..=5).enumerate() {
            assert_eq!(Some(&A(v)), history.get(i).map(|v| unsafe { v.deref() }));
        }
    }

    #[test]
    fn shrink_wrapped() {
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(5).unwrap());
        let old_ptr = history.data;

        // We write 7 values to a history of 5 items, so it's wrapped in such a way
        // that shrinking it down to 3 items needs to copy from both sides
        for i in 1..=7 {
            unsafe { history.append(|ptr| *ptr.deref_mut() = A(i)) };
        }

        assert_eq!(5, history.len);
        assert_eq!(5, history.capacity);
        assert_eq!(2, history.start);
        assert_eq!(Some(&A(3)), history.get(0).map(|v| unsafe { v.deref() }));

        history.resize(NonZero::new(3).unwrap());

        assert_ne!(old_ptr, history.data);
        assert_eq!(3, history.len);
        assert_eq!(3, history.capacity);
        assert_eq!(0, history.start);

        // We should only have the last 3 values
        for (i, v) in (5..=7).enumerate() {
            assert_eq!(Some(&A(v)), history.get(i).map(|v| unsafe { v.deref() }));
        }
    }

    #[test]
    fn resize_same_size() {
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(3).unwrap());
        let old_ptr = history.data;

        history.resize(NonZero::new(3).unwrap());

        assert_eq!(old_ptr, history.data);
    }

    #[test]
    fn grow() {
        let mut history = BlobDeque::new(Layout::new::<A>(), None, NonZero::new(3).unwrap());
        let old_ptr = history.data;

        // We fully fill up our history
        for i in 1..=3 {
            unsafe { history.append(|ptr| *ptr.deref_mut() = A(i)) };
        }
        assert_eq!(3, history.len);
        assert_eq!(3, history.capacity);
        assert_eq!(Some(&A(1)), history.get(0).map(|v| unsafe { v.deref() }));

        history.resize(NonZero::new(5).unwrap());

        assert_ne!(old_ptr, history.data);
        assert_eq!(3, history.len);
        assert_eq!(5, history.capacity);
        assert_eq!(0, history.start);

        // We should be able to write more values
        for i in 4..=5 {
            unsafe { history.append(|ptr| *ptr.deref_mut() = A(i)) };
        }

        assert_eq!(5, history.len);
        assert_eq!(0, history.start);

        for (i, v) in (1..=5).enumerate() {
            assert_eq!(Some(&A(v)), history.get(i).map(|v| unsafe { v.deref() }));
        }
    }

    fn d_hist(size: u8) -> BlobDeque {
        BlobDeque::new(
            Layout::new::<D>(),
            Some(|ptr| unsafe { ptr.drop_as::<D>() }),
            NonZero::new(size).unwrap(),
        )
    }

    #[test]
    fn drop_history() {
        drop_history_with_start(0);
    }

    #[test]
    fn drop_history_offset() {
        for i in 1..=4 {
            drop_history_with_start(i);
        }
    }

    fn drop_history_with_start(start: u8) {
        let drops = DropList::default();
        let mut history = d_hist(5);
        history.start = start;

        for i in 1..=5 {
            unsafe {
                history.append(|ptr| {
                    ptr.deref_mut::<MaybeUninit<D>>().write(D::new(i, &drops));
                });
            };
        }
        assert_eq!(5, history.len);
        assert_drops(&drops, []);

        drop(history);

        assert_drops(&drops, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn shrink_drop() {
        let drops = DropList::default();
        let mut history = d_hist(5);

        for i in 1..=5 {
            unsafe {
                history.append(|ptr| {
                    ptr.deref_mut::<MaybeUninit<D>>().write(D::new(i, &drops));
                });
            };
        }
        assert_eq!(5, history.len);
        assert_drops(&drops, []);

        history.resize(NonZero::new(3).unwrap());

        assert_eq!(3, history.len);
        assert_drops(&drops, [1, 2]);

        drop(history);

        assert_drops(&drops, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn wrap_drop() {
        let drops = DropList::default();
        let mut history = d_hist(5);

        for i in 1..=5 {
            unsafe {
                history.append(|ptr| {
                    ptr.deref_mut::<MaybeUninit<D>>().write(D::new(i, &drops));
                });
            };
        }
        assert_eq!(5, history.len);
        assert_drops(&drops, []);

        for i in 6..=9 {
            unsafe {
                history.append(|ptr| {
                    ptr.deref_mut::<MaybeUninit<D>>().write(D::new(i, &drops));
                });
            };
        }
        assert_eq!(5, history.len);
        assert_drops(&drops, [1, 2, 3, 4]);

        drop(history);

        assert_drops(&drops, [1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn drop_front() {
        let drops = DropList::default();
        let mut history = d_hist(5);

        for i in 1..=5 {
            unsafe {
                history.append(|ptr| {
                    ptr.deref_mut::<MaybeUninit<D>>().write(D::new(i, &drops));
                });
            };
        }
        assert_eq!(5, history.len);
        assert_drops(&drops, []);

        history.drop_front();

        assert_eq!(4, history.len);
        assert_drops(&drops, [1]);

        history.drop_front();

        assert_eq!(3, history.len);
        assert_drops(&drops, [1, 2]);

        drop(history);
        assert_drops(&drops, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn drop_front_small_or_empty() {
        let drops = DropList::default();
        let mut history = d_hist(5);

        unsafe {
            history.append(|ptr| {
                ptr.deref_mut::<MaybeUninit<D>>().write(D::new(1, &drops));
            });
        };
        assert_eq!(1, history.len);
        assert_drops(&drops, []);

        history.drop_front();

        assert_eq!(0, history.len);
        assert_drops(&drops, [1]);

        history.drop_front();
        assert_drops(&drops, [1]);

        drop(history);
        assert_drops(&drops, [1]);
    }

    #[test]
    fn drop_back() {
        let drops = DropList::default();
        let mut history = d_hist(5);

        for i in 1..=5 {
            unsafe {
                history.append(|ptr| {
                    ptr.deref_mut::<MaybeUninit<D>>().write(D::new(i, &drops));
                });
            };
        }
        assert_eq!(5, history.len);
        assert_drops(&drops, []);

        history.drop_back();

        assert_eq!(4, history.len);
        assert_drops(&drops, [5]);

        history.drop_back();

        assert_eq!(3, history.len);
        assert_drops(&drops, [5, 4]);

        drop(history);
        assert_drops(&drops, [5, 4, 1, 2, 3]);
    }

    #[test]
    fn drop_back_small_or_empty() {
        let drops = DropList::default();
        let mut history = d_hist(5);

        unsafe {
            history.append(|ptr| {
                ptr.deref_mut::<MaybeUninit<D>>().write(D::new(1, &drops));
            });
        };
        assert_eq!(1, history.len);
        assert_drops(&drops, []);

        history.drop_back();

        assert_eq!(0, history.len);
        assert_drops(&drops, [1]);

        history.drop_back();
        assert_drops(&drops, [1]);

        drop(history);
        assert_drops(&drops, [1]);
    }
}
