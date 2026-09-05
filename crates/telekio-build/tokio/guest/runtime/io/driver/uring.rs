use super::*;
use std::{os::fd::RawFd, sync::Arc};

struct UringHandle(*const Handle);

// Handle::drop closes the registration before this pointer becomes invalid.
unsafe impl Send for UringHandle {}
unsafe impl Sync for UringHandle {}

impl UringHandle {
    unsafe fn dispatch(&self) {
        unsafe { (*self.0).dispatch_uring_completions() };
    }
}

impl Handle {
    pub(crate) fn add_uring_source_host(&self, fd: RawFd) -> io::Result<()> {
        let handle = UringHandle(self);
        let callback = ::telekio::Callback::from_arc(Arc::new(move || unsafe {
            handle.dispatch();
        }));
        let result = crate::runtime::Handle::current()
            .inner
            .connection()
            .handle
            .register_io_driver(unsafe { ::telekio::IoResource::fd(fd) }, callback);
        self.telekio_uring
            .install(::telekio::IoDriverRegistration::from_result(result)?)
    }

    fn dispatch_uring_completions(&self) {
        let mut context = self.get_uring().lock();
        context.dispatch_completions();
        while context
            .uring
            .as_mut()
            .is_some_and(|uring| uring.submission().cq_overflow())
        {
            context
                .submit()
                .expect("failed to flush io_uring completion queue overflow");
            context.dispatch_completions();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.telekio_uring.close();
    }
}
