mod accept;

mod close;
pub(crate) use close::Close;

mod connect;

mod fsync;

mod noop;
pub(crate) use noop::NoOp;

mod op;
pub(crate) use op::Op;

mod open;

mod read;
mod read_bg;

mod readv;

mod recv_from;

mod rename_at;

mod send_to;

mod shared_fd;
pub(crate) use shared_fd::SharedFd;

mod socket;
pub(crate) use socket::Socket;

mod unlink_at;

mod util;

mod write;

mod writev;

use io_uring::IoUring;
use slab::Slab;
use std::io;
use std::os::unix::io::{AsRawFd, RawFd};

pub(crate) struct Driver {
    /// In-flight operations
    ops: Ops,

    /// IoUring bindings
    pub(crate) uring: IoUring,
}

struct Ops {
    // When dropping the driver, all in-flight operations must have completed. This
    // type wraps the slab and ensures that, on drop, the slab is empty.
    lifecycle: Slab<op::Lifecycle>,
}

impl Driver {
    pub(crate) fn new(b: &crate::Builder) -> io::Result<Driver> {
        let uring = b.urb.build(b.entries)?;

        Ok(Driver {
            ops: Ops::new(),
            uring,
        })
    }

    fn wait(&self) -> io::Result<usize> {
        self.uring.submit_and_wait(1)
    }

    fn num_operations(&self) -> usize {
        self.ops.lifecycle.len()
    }

    pub(crate) fn tick(&mut self) {
        let mut cq = self.uring.completion();
        cq.sync();

        for cqe in cq {
            if cqe.user_data() == u64::MAX {
                // Result of the cancellation action. There isn't anything we
                // need to do here. We must wait for the CQE for the operation
                // that was canceled.
                continue;
            }

            let index = cqe.user_data() as _;

            self.ops.complete(index, cqe.into());
        }
    }

    pub(crate) fn submit(&mut self) -> io::Result<()> {
        loop {
            match self.uring.submit() {
                Ok(_) => {
                    self.uring.submission().sync();
                    return Ok(());
                }
                Err(ref e) if e.raw_os_error() == Some(libc::EBUSY) => {
                    self.tick();
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }
    }
}

impl AsRawFd for Driver {
    fn as_raw_fd(&self) -> RawFd {
        self.uring.as_raw_fd()
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        while self.num_operations() > 0 {
            // If waiting fails, ignore the error. The wait will be attempted
            // again on the next loop.
            let _ = self.wait().unwrap();
            self.tick();
        }
    }
}

impl Ops {
    fn new() -> Ops {
        Ops {
            lifecycle: Slab::with_capacity(64),
        }
    }

    fn get_mut(&mut self, index: usize) -> Option<&mut op::Lifecycle> {
        self.lifecycle.get_mut(index)
    }

    // Insert a new operation
    fn insert(&mut self) -> usize {
        self.lifecycle.insert(op::Lifecycle::Submitted)
    }

    // Remove an operation
    fn remove(&mut self, index: usize) {
        self.lifecycle.remove(index);
    }

    fn complete(&mut self, index: usize, cqe: op::CqeResult) {
        if self.lifecycle[index].complete(cqe) {
            self.lifecycle.remove(index);
        }
    }
}

impl Drop for Ops {
    fn drop(&mut self) {
        assert!(self.lifecycle.is_empty());
    }
}
