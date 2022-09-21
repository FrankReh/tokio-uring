use std::ops::Deref;
use std::rc::Rc;
use std::sync::Mutex;

/// TODO comment
pub(crate) enum Cancellable {
    Base(usize),
    Ptr(Handle),
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

    pub(crate) fn was_canceled(&self) -> bool {
        match self {
            Cancellable::Base(_) => false,
            Cancellable::Ptr(ptr) => ptr.was_canceled(),
            Cancellable::Cancelled => false,
        }
    }

    pub(crate) fn clear_was_canceled(&self) {
        match self {
            Cancellable::Base(_) => {}
            Cancellable::Ptr(ptr) => ptr.clear_was_canceled(),
            Cancellable::Cancelled => {}
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
    pub(crate) fn cancel_clone(&mut self) -> Handle {
        match self {
            Cancellable::Base(index) => {
                let ptr = Handle::new_index(*index);
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
                let ptr = Handle::new_done();
                *self = Cancellable::Ptr(ptr.clone());
                ptr
            }
        }
    }
}

/// TODO comment
#[derive(Clone, Debug)]
pub struct Handle(Rc<Mutex<Option<(usize, bool)>>>);

impl Handle {
    /// TODO comment
    fn new_index(index: usize) -> Self {
        Self(Rc::new(Mutex::new(Some((index, false)))))
    }

    /// TODO comment
    fn new_done() -> Self {
        Self(Rc::new(Mutex::new(None)))
    }

    /// TODO comment
    fn index(&self) -> Option<usize> {
        let guard = self.0.lock().unwrap();
        guard.deref().as_ref().map(|(index, _)| *index)
    }

    /// Called from the cancel flow. Provide the slab index of the operation to be canceled
    /// and mark this Handle has having been used to cancel the option. That mark can then
    /// be used by poll_next to filter out the ECANCELED error and return a successful result of 0
    /// ... Wait a minute - that should not be returned by the stream.
    ///
    /// TODO WIP
    fn index_to_cancel(&mut self) -> Option<usize> {
        let mut guard = self.0.lock().unwrap();
        match guard.deref() {
            Some((index, _)) => {
                let index = *index;
                *guard = Some((index, true)); // mark has having been canceled
                Some(index)
            }
            None => None,
        }
    }

    /// TODO comment
    fn was_canceled(&self) -> bool {
        let guard = self.0.lock().unwrap();
        match guard.deref() {
            Some((_, canceled)) => *canceled,
            None => false,
        }
    }

    /// TODO comment
    fn clear_was_canceled(&self) {
        let mut guard = self.0.lock().unwrap();
        match guard.deref() {
            Some((index, _)) => {
                *guard = Some((*index, false)); // mark as no longer having been canceled
            }
            None => {}
        }
    }

    /// TODO comment
    fn take_index(&mut self) -> Option<usize> {
        let mut guard = self.0.lock().unwrap();
        match guard.deref() {
            Some((index, _)) => {
                let index = *index;
                *guard = None;
                Some(index)
            }
            None => None,
        }
    }

    /// TODO comment
    pub async fn async_cancel(&mut self) {
        println!("async_cancel called on {:?}", self);
        match self.index_to_cancel() {
            Some(index) => {
                let op = Op::async_cancel(index).unwrap(); // TODO don't expect an error
                let completion = op.await;
                println!("async_cancel completion {:?}", completion);
                // Ignore completion.
            }
            None => (),
        }
    }
}

// TODO improve comment
// Stuff for the AsyncCancel operation, which for now, does not need to be made public because the
// slab index hasn't been made public. In the future, there could be an AsyncCancel operation
// defined for file descriptors and since file desciptors are made public, there could be a reason
// for supporting that publicly.

use crate::driver::Op;
use std::io;

#[derive(Debug)]
pub(crate) struct AsyncCancel {
    index: usize,
}

impl Op<AsyncCancel> {
    pub(crate) fn async_cancel(index: usize) -> io::Result<Op<AsyncCancel>> {
        use io_uring::opcode;

        Op::submit_with(AsyncCancel { index }, |ac| {
            let index = ac.index;

            opcode::AsyncCancel::new(index as _).build()
        })
    }
}
