use std::{
    alloc::Layout,
    mem::{ManuallyDrop, MaybeUninit},
    num::NonZero,
};

use bevy::{
    prelude::*,
    ptr::{OwningPtr, Ptr, PtrMut},
};

#[derive(Clone)]
pub struct HistoryComponent {
    layout: Layout,
    store: unsafe fn(Ptr, PtrMut),
    equal: unsafe fn(Ptr, Ptr) -> bool,
    call_load: CallLoad,
    load: unsafe fn(),
    drop: Option<unsafe fn(OwningPtr)>,
}

pub type LoadFn<T> = fn(Option<&T>, Option<&T>, ExistingOrUninit<T>, Commands, entity: Entity);
type CallLoad =
    unsafe fn(unsafe fn(), Option<Ptr>, Option<Ptr>, ErasedExistingOrUninit, Commands, Entity);

impl HistoryComponent {
    /// Get the size of the component
    pub fn size(&self) -> usize {
        self.layout.size()
    }

    /// Get the layout of the component
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Call the component's store function
    /// SAFETY: The types of `src` and `dst` point to MUST match this component's type
    pub unsafe fn store(&self, src: Ptr, dst: PtrMut) {
        unsafe {
            (self.store)(src, dst);
        }
    }

    /// Call the component's equal function
    /// SAFETY: The types of `a` and `b` point to MUST match this component's type
    pub unsafe fn equal(&self, a: Ptr, b: Ptr) -> bool {
        unsafe { (self.equal)(a, b) }
    }

    /// Call the component's load function targeting uninitialized memory
    /// SAFETY: The types of `authoritative`, `predicted`, and `dst` point to MUST match this component's type
    pub unsafe fn load_to_uninit(
        &self,
        authoritative: Option<Ptr>,
        predicted: Option<Ptr>,
        dst: PtrMut,
        commands: Commands,
        entity: Entity,
    ) {
        unsafe {
            (self.call_load)(
                self.load,
                authoritative,
                predicted,
                ErasedExistingOrUninit::Uninit(dst),
                commands,
                entity,
            );
        }
    }

    /// Call the component's load function targeting an existing value
    /// SAFETY: The types of `authoritative`, `predicted`, and `dst` point to MUST match this component's type
    // TODO:
    #[allow(dead_code)]
    pub unsafe fn load_to_component(
        &self,
        authoritative: Option<Ptr>,
        predicted: Option<Ptr>,
        dst: PtrMut,
        commands: Commands,
        entity: Entity,
    ) {
        unsafe {
            (self.call_load)(
                self.load,
                authoritative,
                predicted,
                ErasedExistingOrUninit::Existing(dst),
                commands,
                entity,
            );
        }
    }

    pub fn new<T: Clone + PartialEq>() -> Self {
        Self::new_internal::<T>(
            |_, auth: Option<Ptr>, pred, dst, _, _| unsafe {
                dst.deref::<T>()
                    .write(auth.or(pred).unwrap().deref::<T>().clone());
            },
            || {},
        )
    }

    pub fn with_load<T: Clone + PartialEq>(load_fn: LoadFn<T>) -> Self {
        Self::new_internal::<T>(
            |load, auth, pred, dst, commands, entity| {
                let load = unsafe { std::mem::transmute::<unsafe fn(), LoadFn<T>>(load) };
                (load)(
                    auth.map(|v| unsafe { v.deref::<T>() }),
                    pred.map(|v| unsafe { v.deref::<T>() }),
                    unsafe { dst.deref::<T>() },
                    commands,
                    entity,
                );
            },
            unsafe { std::mem::transmute::<LoadFn<T>, unsafe fn()>(load_fn) },
        )
    }

    fn new_internal<T: Clone + PartialEq>(call_load: CallLoad, load: unsafe fn()) -> Self {
        Self {
            layout: Layout::new::<T>(),
            store: |src, dst| {
                // TODO: Rethink this and the write APIs to ensure our usage is sound and doesn't leak memory
                let value = ManuallyDrop::new(unsafe { src.deref::<T>() }.clone());
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        (&value as *const ManuallyDrop<T>).cast(),
                        dst.as_ptr(),
                        size_of::<T>(),
                    );
                }
            },
            equal: |a, b| unsafe { a.deref::<T>() == b.deref::<T>() },
            call_load,
            load,
            drop: Some(|ptr| unsafe { ptr.drop_as::<T>() }),
        }
    }
}

impl super::sparse_blob_deque::SparseBlobDeque {
    pub(super) fn from_component(component: &HistoryComponent, size: NonZero<u8>) -> Self {
        // SAFETY: We call this using a valid HistoryComponent
        unsafe { Self::new(component.layout, component.drop, size) }
    }

    pub(super) fn from_type<T: Clone + PartialEq>(size: NonZero<u8>) -> Self {
        Self::from_component(&HistoryComponent::new::<T>(), size)
    }
}

pub enum ErasedExistingOrUninit<'a> {
    // TODO:
    #[allow(dead_code)]
    Existing(PtrMut<'a>),
    Uninit(PtrMut<'a>),
}

impl<'a> ErasedExistingOrUninit<'a> {
    unsafe fn deref<T>(self) -> ExistingOrUninit<'a, T> {
        use ErasedExistingOrUninit::*;
        match self {
            Existing(v) => ExistingOrUninit::Existing(unsafe { v.deref_mut::<T>() }),
            Uninit(v) => ExistingOrUninit::Uninit(unsafe { v.deref_mut::<MaybeUninit<T>>() }),
        }
    }
}

/// An existing component, or an uninitialized pointer to one
pub enum ExistingOrUninit<'a, T> {
    /// An existing value
    Existing(&'a mut T),
    /// An uninitialized value
    Uninit(&'a mut MaybeUninit<T>),
}

impl<'a, T> ExistingOrUninit<'a, T> {
    /// Write the provided value
    pub fn write(self, t: T) {
        use ExistingOrUninit::*;
        match self {
            Existing(v) => *v = t,
            Uninit(v) => {
                v.write(t);
            }
        }
    }
}
