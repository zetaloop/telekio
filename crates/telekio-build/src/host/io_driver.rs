use super::*;
use std::{
    os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd},
    sync::{Arc, Condvar, Mutex, Weak},
    task::{Context, Poll, Wake, Waker},
};

pub(crate) struct Registration {
    io: Arc<ScheduledIo>,
    fd: OwnedFd,
    wake: Arc<WakeState>,
}

struct WakeState {
    io: Weak<ScheduledIo>,
    callback: Arc<dyn Fn() + Send + Sync>,
    lifecycle: Mutex<WakeLifecycle>,
    closed: Condvar,
}

#[derive(Default)]
struct WakeLifecycle {
    active: usize,
    closing: bool,
}

struct WakeGuard<'a>(&'a WakeState);

impl Handle {
    pub(crate) fn telekio_register(
        &self,
        fd: RawFd,
        callback: Arc<dyn Fn() + Send + Sync>,
    ) -> io::Result<Registration> {
        if fd < 0 {
            return Err(io::Error::from_raw_os_error(libc::EBADF));
        }
        let fd = unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?;
        let raw = fd.as_raw_fd();
        let mut source = mio::unix::SourceFd(&raw);
        let io = self.add_source(&mut source, Interest::READABLE)?;
        let wake = Arc::new(WakeState {
            io: Arc::downgrade(&io),
            callback,
            lifecycle: Mutex::new(WakeLifecycle::default()),
            closed: Condvar::new(),
        });
        wake.arm();
        Ok(Registration { io, fd, wake })
    }

    pub(crate) fn telekio_deregister(&self, registration: Registration) {
        registration.wake.begin_close();
        registration.io.clear_wakers();
        let raw = registration.fd.as_raw_fd();
        let mut source = mio::unix::SourceFd(&raw);
        _ = self.deregister_source(&registration.io, &mut source);
        registration.wake.wait_closed();
    }
}

impl WakeState {
    fn arm(self: &Arc<Self>) {
        let Some(_guard) = self.enter() else {
            return;
        };
        let Some(io) = self.io.upgrade() else {
            return;
        };
        let waker = Waker::from(Arc::clone(self));
        let mut context = Context::from_waker(&waker);
        loop {
            match io.poll_readiness(&mut context, Direction::Read) {
                Poll::Pending => return,
                Poll::Ready(event) if event.is_shutdown => return,
                Poll::Ready(event) => {
                    (self.callback)();
                    io.clear_readiness(event);
                }
            }
        }
    }

    fn enter(&self) -> Option<WakeGuard<'_>> {
        let mut lifecycle = self.lifecycle.lock().unwrap();
        if lifecycle.closing {
            return None;
        }
        lifecycle.active += 1;
        Some(WakeGuard(self))
    }

    fn begin_close(&self) {
        self.lifecycle.lock().unwrap().closing = true;
    }

    fn wait_closed(&self) {
        let mut lifecycle = self.lifecycle.lock().unwrap();
        while lifecycle.active != 0 {
            lifecycle = self.closed.wait(lifecycle).unwrap();
        }
    }
}

impl Drop for WakeGuard<'_> {
    fn drop(&mut self) {
        let mut lifecycle = self.0.lifecycle.lock().unwrap();
        lifecycle.active -= 1;
        if lifecycle.active == 0 {
            self.0.closed.notify_all();
        }
    }
}

impl Wake for WakeState {
    fn wake(self: Arc<Self>) {
        self.arm();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.arm();
    }
}
