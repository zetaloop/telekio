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
struct Reset<'a> {
    context: &'a Context,
    state: *mut State,
    previous: *mut State,
}

#[cfg(feature = "rt")]
impl Drop for Reset<'_> {
    fn drop(&mut self) {
        ACTIVE.set(self.previous);
        let state = unsafe { &*self.state };
        if self.previous.is_null() {
            self.context.current_task_id.set(
                (state.task_id != 0).then(|| crate::runtime::task::Id::from_telekio(state.task_id)),
            );
            self.context
                .budget
                .set(crate::task::coop::Budget::from_telekio(state.budget));
            self.context
                .runtime
                .set(EnterRuntime::from_telekio(state.runtime));
            self.context.rng.set(
                (state.rng_active != 0)
                    .then(|| FastRand::from_telekio(state.rng_one, state.rng_two)),
            );
        } else {
            // The enclosing call keeps its own trace, yield count, and panic frame.
            let previous = unsafe { &mut *self.previous };
            previous.task_id = state.task_id;
            previous.budget = state.budget;
            previous.runtime = state.runtime;
            previous.rng_one = state.rng_one;
            previous.rng_two = state.rng_two;
            previous.rng_active = state.rng_active;
        }
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
    CONTEXT.with(|context| {
        let previous = ACTIVE.get();
        let tracing = crate::runtime::telekio::is_tracing().into();
        let mut state = if previous.is_null() {
            let rng = context.rng.get();
            let (rng_one, rng_two) = rng.map_or((0, 0), FastRand::telekio_parts);
            State {
                runtime: context.runtime.get().telekio_value(),
                task_id: context
                    .current_task_id
                    .get()
                    .map_or(0, |id| id.telekio_value()),
                budget: context.budget.get().telekio_value(),
                rng_one,
                rng_two,
                rng_active: rng.is_some().into(),
                tracing,
                forced_yields: 0,
                panic: std::ptr::null(),
            }
        } else {
            unsafe {
                State {
                    tracing,
                    forced_yields: 0,
                    ..*previous
                }
            }
        };
        let _reset = Reset {
            context,
            state: &raw mut state,
            previous: ACTIVE.replace(&raw mut state),
        };
        unsafe {
            ::telekio_abi::with_execution_state(&raw mut state, || call((&raw mut state).cast()))
        }
    })
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
pub(super) use access::{enter_runtime, runtime, set_task_id, task_id};
