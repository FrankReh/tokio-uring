use std::{ops::Deref, rc::Rc, sync::Mutex};

/// A Flat is created with a usize index or already in the Taken state.
///
/// A Flat that still has an index can have that index taken from it.
///
/// The invariant is once the Flat reaches the Taken state, it does not go back to having an index.
pub(crate) enum Flat {
    Idx(usize),
    Taken,
}

impl Flat {
    /// Get the index without changing anything about the type.
    pub(crate) fn index(&self) -> Option<usize> {
        match self {
            Flat::Idx(index) => Some(*index),
            Flat::Taken => None,
        }
    }

    /// Take the index with the intention of removing the item from the slab.
    pub(crate) fn take_index(&mut self) -> Option<usize> {
        match self {
            Flat::Idx(index) => {
                let index = *index;
                *self = Flat::Taken;
                Some(index)
            }
            Flat::Taken => None,
        }
    }
}

/// An Index is either an unlatched value or a latched value.
///
/// Both the unlatched and latched values represent an optional index. An index is there until it
/// is taken. The index is taken when the item it represents in a slab is returned to the slab.
///
/// The invariants are
/// 1. All Index start as Unlatched
/// 2. Once an Index is transitioned to Latched, no transition to Unlatched is possible
///
/// The Latched handle exists in order to share the optional index so it can be used in a cancel
/// operation.
pub(crate) enum Index {
    Unlatched(Flat),
    Latched(Handle),
}

impl Index {
    /// A new Index is created when a slab index has been assigned.
    pub(crate) fn new(index: usize) -> Self {
        // The Index starts as Unlatched. It has not yet needed to be shared.
        Index::Unlatched(Flat::Idx(index))
    }

    /// Get the index without changing anything about the type.
    pub(crate) fn index(&self) -> Option<usize> {
        match self {
            Index::Unlatched(flat) => flat.index(),
            Index::Latched(handle) => handle.index(),
        }
    }

    /// Has the operation this Index represents already been canceled?
    /// Useful for the future or stream when deciding if an ECANCELED error should be allowed to
    /// percolate up or if it should be absorbed because it was expected.
    pub(crate) fn was_canceled(&self) -> bool {
        match self {
            // An unlatched index cannot have been canceled, as a handle for cancelling was never
            // issued.
            Index::Unlatched(_) => false,

            Index::Latched(handle) => handle.was_canceled(),
        }
    }

    /// Reset the was_canceled flag so it the Index is asked again whether
    /// it was_canceled, it could return false. Again, only used by the future or stream.
    pub(crate) fn clear_was_canceled(&self) {
        match self {
            Index::Unlatched(_) => {}
            Index::Latched(handle) => handle.clear_was_canceled(),
        }
    }

    /// Take the index with the intention of removing the item from the slab.
    /// The Index will no longer be able to initiate a cancel operation. Safeguarding
    /// the slab so the slab's index can be reused.
    pub(crate) fn take_index(&mut self) -> Option<usize> {
        match self {
            Index::Unlatched(flat) => flat.take_index(),
            Index::Latched(handle) => handle.take_index(),
        }
    }

    /// Return a Handle that allows an index to be used for a cancel operation.
    pub(crate) fn cancel_handle(&mut self) -> Handle {
        match self {
            Index::Unlatched(ref flat) => {
                let handle = Handle::new(flat);
                *self = Index::Latched(handle.clone());
                handle
            }

            Index::Latched(handle) => {
                // When we already have a latched handle, simply return a clone of it.
                handle.clone()
            }
        }
    }
}

/// Handle is a sharable index. It is created when the user wants the ability to cancel an
/// operation, typically a multishot operation. In the future, the Index type could be used in the
/// single shot op operations as well but for now, it is the multishot operations that are
/// targetted.
#[derive(Clone, Debug)]
pub struct Handle(Rc<Mutex<Option<(usize, bool)>>>);

impl Handle {
    /// Return a Handle with either an index or already the Taken state, depending on the given
    /// Flat.
    fn new(flat: &Flat) -> Self {
        match *flat {
            Flat::Idx(index) => Self(Rc::new(Mutex::new(Some((index, false))))),
            Flat::Taken => Self(Rc::new(Mutex::new(None))),
        }
    }

    /// Return the optional index.
    fn index(&self) -> Option<usize> {
        let guard = self.0.lock().unwrap();
        guard.deref().as_ref().map(|(index, _)| *index)
    }

    /// Called from the cancel flow. Provide the slab index of the operation to be canceled
    /// and mark this Handle has having been used to cancel the option. That mark can then
    /// be used by poll_next to absorb the ECANCELED error.
    ///
    /// Returns the index, like the index() call, but also sets the mark to true in order
    /// to return true if was_canceled is called later.
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

    /// Return the indication of the index having been used to cancel the indexed operation.
    fn was_canceled(&self) -> bool {
        let guard = self.0.lock().unwrap();
        match guard.deref() {
            Some((_, canceled)) => *canceled,
            None => false,
        }
    }

    /// Reset the mark so was_canceled would return false if it were to be called again.
    fn clear_was_canceled(&self) {
        let mut guard = self.0.lock().unwrap();
        match guard.deref() {
            Some((index, _)) => {
                *guard = Some((*index, false)); // mark as no longer having been canceled
            }
            None => {}
        }
    }

    /// Take the index, leaving None in its place.
    /// The index will presumably be used one last time to release the item to the slab.
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

    /// Run the async_cancel operation with this Handle's index.
    ///
    /// The raison d'être for this cancellable module.
    ///
    /// The async_cancel operation can only be performed on an index if the index hasn't already
    /// been taken from Handle and the index is guaranteed to have been taken from the Handle
    /// before the index was released to the slab. So there is no chance of cancelling a newer
    /// operation that happened to be assigned the same slab index.
    pub async fn async_cancel(&mut self) {
        // println!("async_cancel called on {:?}", self);
        match self.index_to_cancel() {
            Some(index) => {
                // When the kernel has already completed an operation before its receives the
                // cancel instruction that matches its user_data, it reports an error that the
                // cancel failed. Here we don't care. Either way, we know the operation has
                // completed.
                let op = Op::async_cancel(index).unwrap(); // don't expect an error in the creation.
                let _result = op.await;
                // println!("async_cancel completion {:?}", _result);
            }
            None => (),
        }
    }
}

// Stuff for the AsyncCancel operation, which for now, does not need to be made public because the
// slab index hasn't been made public. In the future, there could be an AsyncCancel operation
// defined for file descriptors and since file desciptors are made public, there could be a reason
// for supporting that publicly.

use crate::driver::Op;
use std::io;

struct AsyncCancel {
    index: usize,
}

impl Op<AsyncCancel> {
    fn async_cancel(index: usize) -> io::Result<Op<AsyncCancel>> {
        use io_uring::opcode;

        Op::submit_with(AsyncCancel { index }, |ac| {
            let index = ac.index;

            opcode::AsyncCancel::new(index as _).build()
        })
    }
}
