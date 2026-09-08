use super::*;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

struct BlockingState<F, R> {
    function: Option<F>,
    result: Option<Result<R, Box<dyn std::any::Any + Send>>>,
}

pub(super) fn exit_host_runtime<F, R>(function: F) -> R
where
    F: FnOnce() -> R,
{
    let connection = scheduler::Handle::current().connection().clone();
    let mut state = BlockingState {
        function: Some(function),
        result: None,
    };
    let call = connection.handle.block_in_place(unsafe {
        ::telekio_abi::Blocking::from_raw((&raw mut state).cast(), run::<F, R>)
    });
    context::telekio::restore();
    match (call.status, state.result) {
        (::telekio_abi::Status::Ok, Some(Ok(output))) => {
            unsafe { call.payload.release() };
            output
        }
        (::telekio_abi::Status::Panicked, Some(Err(payload))) => {
            unsafe { call.payload.release() };
            resume_unwind(payload)
        }
        (::telekio_abi::Status::HostPanicked, _) => {
            resume_unwind(Box::new(unsafe { call.payload.into_string() }))
        }
        (::telekio_abi::Status::Error, _) => panic!("{}", unsafe { call.payload.into_string() }),
        _ => panic!("host Tokio runtime returned an invalid block_in_place result"),
    }
}

unsafe extern "C" fn run<F, R>(data: *mut std::ffi::c_void) -> ::telekio_abi::Status
where
    F: FnOnce() -> R,
{
    let state = unsafe { &mut *data.cast::<BlockingState<F, R>>() };
    match catch_unwind(AssertUnwindSafe(|| {
        context::exit_runtime(|| state.function.take().expect("blocking function ran twice")())
    })) {
        Ok(output) => {
            state.result = Some(Ok(output));
            ::telekio_abi::Status::Ok
        }
        Err(payload) => {
            state.result = Some(Err(payload));
            ::telekio_abi::Status::Panicked
        }
    }
}
