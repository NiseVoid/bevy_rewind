use crate::AuthoritativeHistory;

use super::{
    component::HistoryComponent,
    component_history::{ComponentHistory, TickData},
    PredictedHistory,
};

use std::{
    num::NonZero,
    sync::{Arc, RwLock},
};

use bevy::{
    ecs::component::ComponentId,
    platform_support::collections::HashSet,
    prelude::*,
    ptr::{Ptr, PtrMut},
};
use bevy_replicon::{client::confirm_history::ConfirmHistory, core::replicon_tick::RepliconTick};

// Test components

// A simple component with a value
#[derive(Component, Clone, PartialEq, Eq, Deref, DerefMut, Debug)]
pub struct A(pub u16);

pub fn a(v: u16) -> TickData<A> {
    TickData::Value(A(v))
}

// A simple component without a value
#[derive(Component, Clone, PartialEq, Eq, Debug)]
pub struct B;

pub fn b() -> TickData<B> {
    TickData::Value(B)
}

// A component with multiple fields
#[derive(Component, Clone, PartialEq, Eq, Debug)]
pub struct C(pub u8, pub u16);

#[derive(Resource, Clone, Deref, DerefMut, Debug, Default)]
pub struct DropList(Arc<RwLock<Drops>>);

#[derive(Clone, Debug, Default)]
pub struct Drops {
    pub present: HashSet<u16>,
    pub order: Vec<u16>,
}

#[track_caller]
pub fn assert_drops(drops: &DropList, order: impl Into<Vec<u16>>) {
    let order = order.into();

    let guard = drops.read().unwrap();
    assert_eq!(order, guard.order);
    assert_eq!(order.len(), guard.present.len());
}

// A component with a drop function to track if items actually get dropped exactly once
#[derive(Component, Clone, Debug)]
pub struct D(pub u16, DropList);

impl D {
    pub fn new(v: u16, list: &DropList) -> Self {
        Self(v, list.clone())
    }
}

impl PartialEq for D {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Drop for D {
    fn drop(&mut self) {
        let mut guard = self.1.write().unwrap();
        if guard.present.contains(&self.0) {
            panic!("Detected double drop!");
        }
        guard.present.insert(self.0);
        guard.order.push(self.0);
    }
}

// A component with a f32, useful for testing non-Eq cases with NaN
#[derive(Component, Clone, PartialEq, Deref, DerefMut, Debug)]
pub struct F(pub f32);

// Helpers

pub fn r_tick(tick: u32) -> RepliconTick {
    RepliconTick::new(tick)
}

pub trait MapDeref<'a> {
    fn deref<T>(self) -> Option<&'a T>;
}

// WARN: This function is actually unsafe, but not marked as such to avoid cluttering the tests
// DO NOT USE THIS OUTSIDE OF TESTS!
impl<'a> MapDeref<'a> for Option<Ptr<'a>> {
    fn deref<T>(self) -> Option<&'a T> {
        self.map(|v| unsafe { v.deref::<T>() })
    }
}

pub trait MapDerefMut<'a> {
    fn deref<T>(self) -> Option<&'a mut T>;
}

// WARN: This function is actually unsafe, but not marked as such to avoid cluttering the tests
// DO NOT USE THIS OUTSIDE OF TESTS!
impl<'a> MapDerefMut<'a> for Option<PtrMut<'a>> {
    fn deref<T>(self) -> Option<&'a mut T> {
        self.map(|v| unsafe { v.deref_mut::<T>() })
    }
}

pub trait TickDataDeref {
    fn deref<T>(&self) -> TickData<&T>;
}

// WARN: This function is actually unsafe, but not marked as such to avoid cluttering the tests
// DO NOT USE THIS OUTSIDE OF TESTS!
impl<'a> TickDataDeref for TickData<Ptr<'a>> {
    fn deref<T>(&self) -> TickData<&T> {
        self.map(|v| unsafe { v.deref::<T>() })
    }
}

pub trait IterEnumerate {
    type Item;
    fn iter_enumerate(self) -> impl Iterator<Item = (usize, Self::Item)>;
}

impl<V, I: IntoIterator<Item = V>> IterEnumerate for I {
    type Item = V;
    fn iter_enumerate(self) -> impl Iterator<Item = (usize, Self::Item)> {
        self.into_iter().enumerate()
    }
}

// Shorthand constructors

pub fn comp_history<T: Component + Clone + PartialEq>(
    first_tick: u32,
    data: impl IntoIterator<Item = TickData<T>>,
) -> ComponentHistory {
    let data = data.into_iter();
    let len = data.size_hint().0;
    let mut comp_hist = ComponentHistory::from_component(&HistoryComponent::new::<T>(), unsafe {
        NonZero::new_unchecked(len.max(5) as u8)
    });

    for (offset, v) in data.enumerate() {
        let tick = first_tick + offset as u32;
        match v {
            TickData::Value(v) => {
                unsafe { comp_hist.write(tick, |ptr| *ptr.deref_mut() = v) };
            }
            TickData::Removed => {
                comp_hist.mark_removed(tick);
            }
            TickData::Missing => {
                todo!();
            }
        }
    }
    comp_hist
}

pub fn pred_history<T: Component + Clone + PartialEq>(
    first_tick: u32,
    comp_id: ComponentId,
    data: impl IntoIterator<Item = TickData<T>>,
) -> PredictedHistory {
    let mut pred_hist = PredictedHistory::default();
    pred_hist.insert(comp_id, comp_history(first_tick, data));
    pred_hist
}

pub fn auth_history<T: Component + Clone + PartialEq>(
    first_tick: u32,
    comp_id: ComponentId,
    data: impl IntoIterator<Item = TickData<T>>,
) -> AuthoritativeHistory {
    let mut auth_hist = AuthoritativeHistory::default();
    auth_hist.insert(comp_id, comp_history(first_tick, data));
    auth_hist
}

pub fn confirm_history(confirmed: impl IntoIterator<Item = u32>) -> ConfirmHistory {
    let mut confirm = ConfirmHistory::new(RepliconTick::new(0));
    for tick in confirmed.into_iter() {
        confirm.confirm(RepliconTick::new(tick));
    }
    confirm
}
