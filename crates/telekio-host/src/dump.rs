use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
};

use telekio::{CallResult, DumpOperation, DumpResult};

use super::runtime::HandleContext;

pub(super) unsafe extern "C" fn start(context: *const c_void) -> DumpResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| imp::create(context))) {
        Ok(Ok(dump)) => DumpResult {
            call: CallResult::ok(),
            dump,
        },
        Ok(Err(error)) => DumpResult {
            call: CallResult::error(&error),
            dump: DumpOperation::empty(),
        },
        Err(payload) => DumpResult {
            call: super::host_panic(&*payload),
            dump: DumpOperation::empty(),
        },
    }
}

#[cfg(all(
    feature = "taskdump",
    target_os = "linux",
    any(
        target_arch = "aarch64",
        target_arch = "x86",
        target_arch = "x86_64",
        target_arch = "s390x"
    )
))]
mod imp {
    use super::*;
    use std::{
        future::Future,
        pin::Pin,
        sync::Arc,
        task::{Context, Poll as RustPoll},
    };
    use telekio::{OperationPoll, OwnedBytes, Poll, Status, Waker};

    use crate::owner::HostResource;

    struct Dump {
        future: Pin<Box<dyn Future<Output = Vec<u8>> + Send>>,
    }

    pub(super) fn create(context: &HandleContext) -> Result<DumpOperation, String> {
        let handle = context.handle.clone();
        let dump = HostResource::new(
            &context.owner,
            Dump {
                future: Box::pin(async move { handle.dump().await.telekio_encode() }),
            },
        )?;
        Ok(unsafe { DumpOperation::from_raw(Arc::as_ptr(&dump).cast_mut().cast(), poll, release) })
    }

    unsafe extern "C" fn poll(data: *mut c_void, waker: *const Waker) -> OperationPoll {
        match catch_unwind(AssertUnwindSafe(|| {
            let dump = unsafe { &*data.cast::<HostResource<Dump>>() };
            dump.update_waker(unsafe { &*waker });
            let waker = unsafe { (*waker).clone_rust_waker() };
            let mut context = Context::from_waker(&waker);
            match dump.with_mut(|dump| dump.future.as_mut().poll(&mut context)) {
                Ok(RustPoll::Pending) => OperationPoll {
                    state: Poll::Pending,
                    call: CallResult::ok(),
                },
                Ok(RustPoll::Ready(bytes)) => OperationPoll {
                    state: Poll::Ready,
                    call: CallResult {
                        status: Status::Ok,
                        payload: OwnedBytes::from_vec(bytes),
                    },
                },
                Err(error) => OperationPoll {
                    state: Poll::Ready,
                    call: CallResult::error(&error),
                },
            }
        })) {
            Ok(result) => result,
            Err(payload) => OperationPoll {
                state: Poll::Panicked,
                call: crate::host_panic(&*payload),
            },
        }
    }

    unsafe extern "C" fn release(data: *mut c_void) -> CallResult {
        crate::host_callback(|| unsafe { &*data.cast::<HostResource<Dump>>() }.release())
    }
}

#[cfg(not(all(
    feature = "taskdump",
    target_os = "linux",
    any(
        target_arch = "aarch64",
        target_arch = "x86",
        target_arch = "x86_64",
        target_arch = "s390x"
    )
)))]
mod imp {
    use super::*;

    pub(super) fn create(_: &HandleContext) -> Result<DumpOperation, String> {
        Err("Tokio runtime dumps are unavailable on this platform".to_owned())
    }
}
