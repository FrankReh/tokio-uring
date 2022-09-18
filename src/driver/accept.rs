use crate::driver::{Op, SharedFd, StrOp};
use std::{boxed::Box, io};

// Clone used when struct used for multishot stream.

#[derive(Clone)]
pub(crate) struct Accept {
    fd: SharedFd,
    pub(crate) socketaddr: Box<(libc::sockaddr_storage, libc::socklen_t)>,
}

impl Op<Accept> {
    pub(crate) fn accept(fd: &SharedFd) -> io::Result<Op<Accept>> {
        use io_uring::{opcode, types};

        let socketaddr = Box::new((
            unsafe { std::mem::zeroed() },
            std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t,
        ));
        Op::submit_with(
            Accept {
                fd: fd.clone(),
                socketaddr,
            },
            |accept| {
                opcode::Accept::new(
                    types::Fd(accept.fd.raw_fd()),
                    &mut accept.socketaddr.0 as *mut _ as *mut _,
                    &mut accept.socketaddr.1,
                )
                .flags(libc::O_CLOEXEC)
                .build()
            },
        )
    }
}

impl StrOp<Accept> {
    pub(crate) fn accept_multishot(fd: &SharedFd) -> io::Result<StrOp<Accept>> {
        use io_uring::{opcode, types};

        let socketaddr = Box::new((
            unsafe { std::mem::zeroed() },
            std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t,
        ));
        StrOp::submit_with(
            Accept {
                fd: fd.clone(),
                socketaddr,
            },
            |accept| {
                opcode::Accept::new(
                    types::Fd(accept.fd.raw_fd()),
                    &mut accept.socketaddr.0 as *mut _ as *mut _,
                    &mut accept.socketaddr.1,
                )
                .ioprio(io_uring::sys::IORING_ACCEPT_MULTISHOT as _)
                .flags(libc::O_CLOEXEC)
                .build()
            },
        )
    }
}
