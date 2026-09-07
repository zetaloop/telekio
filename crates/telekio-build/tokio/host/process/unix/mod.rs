use super::{OrphanQueueImpl, Wait};
use crate::runtime::signal::Handle as SignalHandle;
use std::{io, os::unix::process::ExitStatusExt, process::ExitStatus, sync::OnceLock};

#[derive(Debug)]
struct Child(u32);

impl Wait for Child {
    fn id(&self) -> u32 {
        self.0
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let mut status = 0;
        match unsafe { libc::waitpid(self.0 as libc::pid_t, &mut status, libc::WNOHANG) } {
            0 => Ok(None),
            pid if pid == self.0 as libc::pid_t => Ok(Some(ExitStatus::from_raw(status))),
            _ => Err(io::Error::last_os_error()),
        }
    }
}

fn queue() -> &'static OrphanQueueImpl<Child> {
    static QUEUE: OnceLock<OrphanQueueImpl<Child>> = OnceLock::new();
    QUEUE.get_or_init(OrphanQueueImpl::new)
}

#[cfg(feature = "rt")]
pub(crate) fn push(id: u32) {
    queue().push_orphan(Child(id));
}

pub(crate) fn reap(handle: &SignalHandle) {
    queue().reap_orphans(handle);
}
