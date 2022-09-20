use std::ops::Deref;
use std::rc::Rc;
use std::sync::Mutex;

/// TODO comment
pub(crate) enum Cancellable {
    Base(usize),
    Ptr(Ptr),
    Cancelled,
}

/// TODO comment
impl Cancellable {
    /// TODO comment
    pub(crate) fn new(index: usize) -> Self {
        Cancellable::Base(index)
    }

    /// Get the index without changing anything about the type.
    pub(crate) fn index(&self) -> Option<usize> {
        match self {
            Cancellable::Base(index) => Some(*index),
            Cancellable::Ptr(ptr) => ptr.index(),
            Cancellable::Cancelled => None,
        }
    }

    /// Take the index with the intention of removing the item from the slab.
    pub(crate) fn take_index(&mut self) -> Option<usize> {
        match self {
            Cancellable::Base(index) => {
                let index = *index;
                *self = Cancellable::Cancelled;
                Some(index)
            }
            Cancellable::Ptr(ptr) => ptr.take_index(),
            Cancellable::Cancelled => None,
        }
    }

    /// TODO comment
    pub(crate) fn cancel_clone(&mut self) -> Ptr {
        match self {
            Cancellable::Base(index) => {
                let ptr = Ptr::new_index(*index);
                *self = Cancellable::Ptr(ptr.clone());
                ptr
            }

            Cancellable::Ptr(ptr) => {
                // When we already have a cancel_clone ptr, return it,
                // but incremented.
                ptr.clone()
            }
            Cancellable::Cancelled => {
                // When the op is already done or cancelled and only now a cancel_clone is
                // asked for, create one but create it already in the done state and switch
                // ourself to it to avoid allocating more if cancel_clone is called again.
                let ptr = Ptr::new_done();
                *self = Cancellable::Ptr(ptr.clone());
                ptr
            }
        }
    }
}

/// TODO comment
#[derive(Clone)]
pub struct Ptr(Rc<Mutex<Option<usize>>>);

impl Ptr {
    /// TODO comment
    fn new_index(index: usize) -> Self {
        Self(Rc::new(Mutex::new(Some(index))))
    }

    /// TODO comment
    fn new_done() -> Self {
        Self(Rc::new(Mutex::new(None)))
    }

    /// TODO comment
    fn index(&self) -> Option<usize> {
        let guard = self.0.lock().unwrap();
        guard.deref().as_ref().copied()
        /*
        //guard.deref().as_ref().map(|index| *index)
        match guard.deref() {
            Some(index) => {
                Some(*index)
            },
            None => {None}
        }
        */
    }

    /// TODO comment
    fn take_index(&mut self) -> Option<usize> {
        let mut guard = self.0.lock().unwrap();
        match guard.deref() {
            Some(index) => {
                let index = *index;
                *guard = None;
                Some(index)
            }
            None => None,
        }
    }
}
