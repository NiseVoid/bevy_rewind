use std::ptr::NonNull;

use bevy::{
    ecs::{component::ComponentId, system::EntityCommand},
    prelude::*,
    ptr::PtrMut,
};

use super::component::HistoryComponent;

#[derive(Clone, Debug)]
pub struct InsertBatch {
    ids: Vec<ComponentId>,
    offsets: Vec<usize>,
    data: Vec<u8>,
}

impl InsertBatch {
    pub fn new() -> Self {
        Self {
            ids: Vec::with_capacity(128),
            offsets: Vec::with_capacity(128),
            data: Vec::with_capacity(2048),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn push(
        &mut self,
        comp_id: ComponentId,
        comp: &HistoryComponent,
        write_fn: impl FnOnce(PtrMut),
    ) {
        self.ids.push(comp_id);
        if comp.size() == 0 {
            return;
        }

        // If items would otherwise not be aligned, add alignment
        let align = comp.layout().align();
        let extra_offset = if self.data.len() % align != 0 {
            align - (self.data.len() % align)
        } else {
            0
        };

        let grow = comp.size() + extra_offset;
        let offset = self.data.len() + extra_offset;

        self.offsets.push(offset);
        self.data.extend((0..grow).map(|_| 0));
        write_fn(unsafe {
            PtrMut::new(NonNull::new_unchecked(
                (&mut self.data[offset..] as *mut [u8]).cast(),
            ))
        });
    }

    pub fn clear(&mut self) {
        self.ids.clear();
        self.offsets.clear();
        self.data.clear();
    }
}

impl EntityCommand for InsertBatch {
    fn apply(mut self, mut entity: EntityWorldMut) {
        let iter = self.offsets.iter().map(|&offset| {
            let ptr = unsafe {
                PtrMut::new(NonNull::new_unchecked(
                    (&mut self.data[offset..] as *mut [u8]).cast(),
                ))
            };
            unsafe { ptr.promote() }
        });
        unsafe { entity.insert_by_ids(&self.ids, iter) };
    }
}

#[derive(Clone)]
pub struct RemoveBatch {
    ids: Vec<ComponentId>,
}

impl RemoveBatch {
    pub fn new() -> Self {
        Self {
            ids: Vec::with_capacity(128),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn push(&mut self, comp_id: ComponentId) {
        self.ids.push(comp_id);
    }

    pub fn clear(&mut self) {
        self.ids.clear();
    }
}

impl EntityCommand for RemoveBatch {
    fn apply(self, mut entity: EntityWorldMut) {
        for id in self.ids {
            entity.remove_by_id(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::history::component::HistoryComponent;

    use super::{super::test_utils::*, InsertBatch};
    use bevy::{ecs::system::EntityCommand, prelude::*};

    #[test]
    fn insert_minimal_archetype_moves() {
        let mut world = World::new();

        let comp_a = world.register_component::<A>();
        let comp_c = world.register_component::<C>();

        let mut batch = InsertBatch::new();
        batch.push(comp_a, &HistoryComponent::new::<A>(), |ptr| {
            *unsafe { ptr.deref_mut::<A>() } = A(5);
        });
        batch.push(comp_c, &HistoryComponent::new::<C>(), |ptr| {
            *unsafe { ptr.deref_mut::<C>() } = C(12, 2);
        });

        let e1 = world.spawn_empty().id();
        world.flush();

        let archetypes_before = world.archetypes().len();
        let e = world.entity_mut(e1);
        assert_eq!(None, e.get::<A>());
        assert_eq!(None, e.get::<C>());

        batch.apply(e);
        world.flush();

        let e = world.entity(e1);
        assert_eq!(Some(&A(5)), e.get::<A>());
        assert_eq!(Some(&C(12, 2)), e.get::<C>());
        let archetypes_after = world.archetypes().len();
        assert_eq!(archetypes_before + 1, archetypes_after);
    }
}
