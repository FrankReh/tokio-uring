pub(crate) mod cancellable;

mod accept;

mod close;
/* TODO
 * refer to reason the Closing variant was removed from shared_fd.
pub(crate) use close::Close;
 */

mod connect;

mod fsync;

mod op;
pub(crate) use op::Op;

mod strop;
pub(crate) use strop::StrOp;

mod open;

mod read;

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

use io_uring::{cqueue, IoUring};
use scoped_tls::scoped_thread_local;
use slab::Slab;
use std::cell::RefCell;
use std::io;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;

pub(crate) struct Driver {
    inner: Handle,
}

type Handle = Rc<RefCell<Inner>>;

pub(crate) struct Inner {
    /// In-flight operations
    ops: Ops,

    /// IoUring bindings
    pub(crate) uring: IoUring,
}

pub(crate) enum Split {
    /// The single shot operations.
    Single(op::Lifecycle),

    /// The multi shot operations.
    Multi(strop::Lifecycle),
}

impl Split {
    pub(super) fn complete(&mut self, result: io::Result<u32>, flags: u32) -> bool {
        match self {
            Split::Single(lifecycle) => lifecycle.complete(result, flags),
            Split::Multi(lifecycle) => lifecycle.result(result, flags),
        }
    }
}

// When dropping the driver, all in-flight operations must have completed. This
// type wraps the slab and ensures that, on drop, the slab is empty.
struct Ops(Slab<Split>);

scoped_thread_local!(pub(crate) static CURRENT: Rc<RefCell<Inner>>);

impl Driver {
    pub(crate) fn new(b: &crate::Builder) -> io::Result<Driver> {
        let uring = b.urb.build(b.entries)?;

        let inner = Rc::new(RefCell::new(Inner {
            ops: Ops::new(),
            uring,
        }));

        Ok(Driver { inner })
    }

    /// Enter the driver context. This enables using uring types.
    pub(crate) fn with<R>(&self, f: impl FnOnce() -> R) -> R {
        CURRENT.set(&self.inner, f)
    }

    pub(crate) fn tick(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.tick();
    }

    fn wait(&self) -> io::Result<usize> {
        let mut inner = self.inner.borrow_mut();
        let inner = &mut *inner;

        inner.uring.submit_and_wait(1)
    }

    fn num_operations(&self) -> usize {
        let inner = self.inner.borrow();
        inner.ops.0.len()
    }
}

impl Inner {
    fn tick(&mut self) {
        // loop, calling completion and then submit, and calling completion again
        // and exiting when completion returns no more entries.
        // This should handle the overflow case because the call to submit, in the forked io-uring
        // crate, will get the syscall `enter` made with the appropriate flag set, and it shouldn't
        // return until the cq has had entries fill again.
        // TODO Check with uring author, is it fair to say the cq will be filled by the call to `enter`
        // when the appropriate flag is set and there were overflow results to report?
        let mut loop_cnt: usize = 0;
        // let mut first_len: usize = 0;
        loop {
            loop_cnt += 1;

            let mut cq = self.uring.completion();
            cq.sync();

            if loop_cnt > 1 && cq.is_empty() {
                return;
            }
            //println!("driver/mod.rs: tick: cq entries: {}", cq.len());

            /* TODO reenable for own testing
            if loop_cnt == 1 {
                first_len = cq.len();
            } else if !cq.is_empty() {
                // Don't print anything the first time through the loop.
                // For own debugging, its only interesting what the first was when the loop
                // was needed two or more times.
                if loop_cnt == 2 {
                    println!("\ntick: loop 1, cq.len {}", first_len);
                }
                println!("tick: loop {}, cq.len {}", loop_cnt, cq.len());
            }
            */

            for cqe in cq {
                // TODO This is left over from the first commit, where one way to get an operation
                // canceled was shown. That code was initially commented and now removed but it might
                // come in handy for the StrOp drop case, which is currently a WIP, because it showed a
                // way to initiate an async cancel operation, without creating a future at all. New
                // kernel io_uring features include a way to submit an operation with no cqe being
                // created for it, and also includes a sync version of the cancel, so how much work
                // should be put into this older form of cancellation is still TBD.
                if cqe.user_data() == u64::MAX {
                    // Result of the cancellation action. There isn't anything we
                    // need to do here. We must wait for the CQE for the operation
                    // that was canceled.
                    continue;
                }

                let index = cqe.user_data() as _;

                self.ops.complete(index, resultify(&cqe), cqe.flags());
            }
            match self.uring.submit() {
                Ok(n) => {
                    if n > 0 {
                        println!("tick loop {}, submit returned n {}", loop_cnt, n);
                    }
                }
                Err(ref e) if e.raw_os_error() == Some(libc::EBUSY) => {
                    println!("tick loop {}, submit returned EBUSY", loop_cnt);
                }
                Err(e) => {
                    panic!("tick: submit error: {}", e);
                }
            }
        }
    }

    pub(crate) fn submit2(&mut self) -> io::Result<()> {
        use std::io::Write; // bring Write into scope for flush.

        //let mut debug = false;
        let mut loop_cnt = 0;
        if self.uring.submission().cq_overflow() {
            println!("driver/mod.rs: submit: found cq_overflow at top");
            //debug = true;
        } else {
            //print!(".");
        }
        std::io::stdout().flush().unwrap();
        loop {
            self.uring.submission().sync();
            loop_cnt += 1;
            match self.uring.submit() {
                Ok(_) => {
                    return Ok(());
                }
                /*
                 * Some debug code that may still come in handy as we find better ways to stress
                 * the rings.
                Ok(n) => {
                    if self.uring.submission().cq_overflow() {
                        println!("driver/mod.rs: submit: loop_cnt {}, Ok({}), found cq_overflow after uring.submit, before sync", loop_cnt, n);
                        std::io::stdout().flush().unwrap();
                        debug = true;
                    }

                    self.uring.submission().sync();
                    self.tick(); // TODO should this be here? What's its impact on to the Readable trigger?

                    if self.uring.submission().cq_overflow() {
                        println!("driver/mod.rs: submit: loop_cnt {} found cq_overflow after uring.submit, after sync, looping around", loop_cnt);
                        std::io::stdout().flush().unwrap();
                        debug = true;
                        continue;
                    }
                    if debug {
                        println!("driver/mod.rs: submit: loop_cnt {} cq_overflow clear", loop_cnt);
                        std::io::stdout().flush().unwrap();
                    }

                    return Ok(());
                }
                */
                Err(ref e) if e.raw_os_error() == Some(libc::EBUSY) => {
                    //self.tick(); // TODO should this be here? What's it do to the Readable trigger?
                    // TODO this call to tick could be with a no-recurse flag passed in
                    // as this submit will be called by tick very soon, as part of the normal path.
                    // EBUSY indicates the cq overflow condition, but overflow or not, the cq could
                    // have entries in it, and higher level logic could be responsible for the
                    // enter/tick dance.

                    // TODO trying to even create a test case that triggers this EBUSY with the
                    // forked io-uring crate and the loop that is in tick now.
                    println!("driver/mod.rs: submit: loop_cnt {}, error EBUSY", loop_cnt);
                    std::io::stdout().flush().unwrap();
                }
                Err(e) => {
                    println!("driver/mod.rs: submit: loop_cnt {}, Err({})", loop_cnt, e);
                    std::io::stdout().flush().unwrap();
                    return Err(e);
                }
            }
        }
    }
}

impl AsRawFd for Driver {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.borrow().uring.as_raw_fd()
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
        Ops(Slab::with_capacity(64))
    }

    fn get_mut(&mut self, index: usize) -> Option<&mut Split> {
        self.0.get_mut(index)
    }

    // Insert a new single shot operation
    fn insert_single(&mut self) -> usize {
        self.0.insert(Split::Single(op::Lifecycle::Submitted))
    }

    // Insert a new multi shot operation
    fn insert_multishot(&mut self) -> usize {
        self.0.insert(Split::Multi(
            strop::Lifecycle::Submitted(Default::default()),
        ))
    }

    // Remove an operation
    fn remove(&mut self, index: usize) {
        self.0.remove(index);
    }

    fn complete(&mut self, index: usize, result: io::Result<u32>, flags: u32) {
        if self.0[index].complete(result, flags) {
            self.0.remove(index);
        }
    }
}

impl Drop for Ops {
    fn drop(&mut self) {
        assert!(self.0.is_empty());
    }
}

fn resultify(cqe: &cqueue::Entry) -> io::Result<u32> {
    let res = cqe.result();

    if res >= 0 {
        Ok(res as u32)
    } else {
        Err(io::Error::from_raw_os_error(-res))
    }
}
