use std::panic::resume_unwind;

use telekio_abi::{Callback, Status};

pub(super) struct CallbackOwner(pub(super) Callback);

impl CallbackOwner {
    pub(super) fn is_some(&self) -> bool {
        self.0.is_some()
    }

    pub(super) fn call(&self) {
        let result = self.0.call();
        match result.status {
            Status::Ok => unsafe { result.payload.release() },
            Status::Panicked | Status::HostPanicked | Status::Error => {
                resume_unwind(Box::new(unsafe { result.payload.into_string() }));
            }
        }
    }
}
