use crate::driver::{Op, SharedFd, StrOp};
use std::{boxed::Box, io};

// Clone used when struct used for multishot stream.

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

#[derive(Clone)]
pub(crate) struct AcceptMulti {
    fd: SharedFd,
}

impl StrOp<AcceptMulti> {
    pub(crate) fn accept_multishot(fd: &SharedFd) -> io::Result<StrOp<AcceptMulti>> {
        use io_uring::{opcode, types};

        StrOp::submit_with(AcceptMulti { fd: fd.clone() }, |accept| {
            opcode::AcceptMulti::new(types::Fd(accept.fd.raw_fd()))
                .flags(libc::O_CLOEXEC)
                .build()
        })
    }
}
