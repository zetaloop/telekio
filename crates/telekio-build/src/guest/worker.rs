use super::*;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

struct BlockingState<F, R> {
    function: Option<F>,
    result: Option<Result<R, Box<dyn std::any::Any + Send>>>,
}

pub(super) fn exit_host_runtime<F, R>(function: F) -> R
where
    F: FnOnce() -> R,
{
    let host = scheduler::Handle::current().host().clone();
    let mut state = BlockingState {
        function: Some(function),
        result: None,
    };
    let call = host.block_in_place(::telekio::Blocking {
        data: (&raw mut state).cast(),
        run: run::<F, R>,
    });
    context::telekio::restore();
    match (call.status, state.result) {
        (::telekio::Status::Ok, Some(Ok(output))) => {
            unsafe { call.payload.release() };
            output
        }
        (::telekio::Status::Panicked, Some(Err(payload))) => {
            unsafe { call.payload.release() };
            resume_unwind(payload)
        }
        (::telekio::Status::HostPanicked, _) => {
            resume_unwind(Box::new(unsafe { call.payload.into_string() }))
        }
        (::telekio::Status::Error, _) => panic!("{}", unsafe { call.payload.into_string() }),
        _ => panic!("host Tokio runtime returned an invalid block_in_place result"),
    }
}

unsafe extern "C" fn run<F, R>(data: *mut std::ffi::c_void) -> ::telekio::Status
where
    F: FnOnce() -> R,
{
    let state = unsafe { &mut *data.cast::<BlockingState<F, R>>() };
    match catch_unwind(AssertUnwindSafe(|| {
        context::exit_runtime(|| state.function.take().expect("blocking function ran twice")())
    })) {
        Ok(output) => {
            state.result = Some(Ok(output));
            ::telekio::Status::Ok
        }
        Err(payload) => {
            state.result = Some(Err(payload));
            ::telekio::Status::Panicked
        }
    }
}
