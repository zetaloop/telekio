#[cfg(feature = "rt")]
use super::*;
use std::cell::Cell;
#[cfg(feature = "rt")]
use std::ffi::c_void;

type State = ::telekio_abi::ExecutionState;

thread_local! {
    static ACTIVE: Cell<*mut State> = const { Cell::new(std::ptr::null_mut()) };
}

#[cfg(feature = "rt")]
struct Reset {
    state: *mut State,
    previous: *mut State,
}

#[cfg(feature = "rt")]
impl Drop for Reset {
    fn drop(&mut self) {
        ACTIVE.set(self.previous);
        let state = unsafe { &*self.state };
        set_current_task_id(
            (state.task_id != 0).then(|| crate::runtime::task::Id::from_telekio(state.task_id)),
        );
        _ = super::budget(|budget| {
            budget.set(crate::task::coop::Budget::from_telekio(state.budget));
        });
        CONTEXT.with(access::runtime(|context| {
            context
                .runtime
                .set(EnterRuntime::from_telekio(state.runtime));
        }));
        CONTEXT.with(access::rng(|context| {
            context.rng.set(
                (state.rng_active != 0)
                    .then(|| FastRand::from_telekio(state.rng_one, state.rng_two)),
            );
        }));
        crate::runtime::telekio::record_forced_yields(state.forced_yields);
    }
}

#[cfg(feature = "rt")]
pub(crate) fn with<R>(call: impl FnOnce(*mut c_void) -> R) -> R {
    let active = ACTIVE.get();
    if !active.is_null() {
        return call(active.cast());
    }
    enter(call)
}

#[cfg(feature = "rt")]
pub(crate) fn with_task<R>(call: impl FnOnce(*mut c_void) -> R) -> R {
    enter(call)
}

#[cfg(feature = "rt")]
fn enter<R>(call: impl FnOnce(*mut c_void) -> R) -> R {
    let task_id = current_task_id().map_or(0, |id| id.telekio_value());
    let budget = super::budget(|budget| budget.get().telekio_value()).unwrap_or(u16::MAX);
    let runtime = CONTEXT.with(access::runtime(|context| {
        context.runtime.get().telekio_value()
    }));
    let rng = CONTEXT.with(access::rng(|context| context.rng.get()));
    let (rng_one, rng_two) = rng.map_or((0, 0), FastRand::telekio_parts);
    let previous = ACTIVE.get();
    let mut state = State {
        runtime,
        task_id,
        budget,
        rng_one,
        rng_two,
        rng_active: rng.is_some().into(),
        tracing: crate::runtime::telekio::is_tracing().into(),
        forced_yields: 0,
        panic: if previous.is_null() {
            std::ptr::null()
        } else {
            unsafe { (*previous).panic }
        },
    };
    let _reset = Reset {
        state: &raw mut state,
        previous: ACTIVE.replace(&raw mut state),
    };
    unsafe { ::telekio_abi::with_execution_state(&raw mut state, || call((&raw mut state).cast())) }
}

fn execution_state() -> *mut State {
    ACTIVE.get()
}

#[path = "../../shared/runtime/context.rs"]
mod access;
pub(super) use access::budget;
#[cfg(feature = "rt")]
pub(crate) use access::panicking;
#[cfg(any(feature = "macros", all(feature = "sync", feature = "rt")))]
pub(super) use access::rng;
#[cfg(feature = "rt")]
pub(super) use access::{enter_runtime, runtime, task_id};
