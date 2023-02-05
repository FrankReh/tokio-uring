use std::ffi::CString;
use std::{ffi::CStr, io};

use io_uring::{opcode, types};

use crate::runtime::{
    driver::op::{Completable, CqeResult, Op},
    CONTEXT,
};

use super::SharedFd;

pub(crate) struct Statx {
    #[allow(dead_code)]
    fd: Option<SharedFd>,
    #[allow(dead_code)]
    path: CString,
    statx: Box<libc::statx>,
}

impl Op<Statx> {
    // If we are passed a reference to a shared fd, clone it so we keep it live during the
    // Future. If we aren't, use the libc::AT_FDCWD value.
    // If Path is None, the flags is combined with libc::AT_EMPTY_PATH automatically.
    pub(crate) fn statx(
        fd: Option<&SharedFd>,
        path: Option<CString>,
        flags: i32,
        mask: u32,
    ) -> io::Result<Op<Statx>> {
        let (fd, raw) = match fd {
            Some(shared) => (Some(shared.clone()), shared.raw_fd()),
            None => (None, libc::AT_FDCWD),
        };
        let mut flags = flags;
        let path = match path {
            Some(path) => path,
            None => {
                flags |= libc::AT_EMPTY_PATH;
                CStr::from_bytes_with_nul(b"\0").unwrap().into() // Is there a constant CString we
                                                                 // could use here.
            }
        };
        CONTEXT.with(|x| {
            x.handle().expect("not in a runtime context").submit_op(
                Statx {
                    fd,
                    path,
                    statx: Box::new(unsafe { std::mem::zeroed() }),
                },
                |statx| {
                    opcode::Statx::new(
                        types::Fd(raw),
                        statx.path.as_ptr(),
                        &mut *statx.statx as *mut libc::statx as *mut types::statx,
                    )
                    .flags(flags)
                    .mask(mask)
                    .build()
                },
            )
        })
    }
}

impl Completable for Statx {
    type Output = io::Result<libc::statx>;

    fn complete(self, cqe: CqeResult) -> Self::Output {
        cqe.result?;
        Ok(*self.statx)
    }
}
