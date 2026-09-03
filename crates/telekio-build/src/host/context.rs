#[cfg(feature = "rt")]
use super::*;
use std::cell::Cell;
#[cfg(feature = "rt")]
use std::ffi::c_void;

#[repr(C)]
struct State {
    task_id: u64,
    budget: u16,
    rng_one: u32,
    rng_two: u32,
    rng_active: u8,
}

thread_local! {
    static ACTIVE: Cell<*mut State> = const { Cell::new(std::ptr::null_mut()) };
}

#[cfg(feature = "rt")]
struct Reset {
    state: *mut State,
}

#[cfg(feature = "rt")]
impl Drop for Reset {
    fn drop(&mut self) {
        ACTIVE.set(std::ptr::null_mut());
        let state = unsafe { &*self.state };
        set_current_task_id((state.task_id != 0).then(|| {
            crate::runtime::task::Id::from_telekio(state.task_id)
        }));
        _ = super::budget(|budget| {
            budget.set(crate::task::coop::Budget::from_telekio(state.budget));
        });
        CONTEXT.with(|context| {
            context.rng.set((state.rng_active != 0).then(|| {
                FastRand::from_telekio(state.rng_one, state.rng_two)
            }));
        });
    }
}

#[cfg(feature = "rt")]
pub(crate) fn with<R>(call: impl FnOnce(*mut c_void) -> R) -> R {
    let active = ACTIVE.get();
    if !active.is_null() {
        return call(active.cast());
    }
    let task_id = current_task_id().map_or(0, |id| id.telekio_value());
    let budget = super::budget(|budget| budget.get().telekio_value()).unwrap_or(u16::MAX);
    let rng = CONTEXT.with(|context| context.rng.get());
    let (rng_one, rng_two) = rng.map_or((0, 0), FastRand::telekio_parts);
    let mut state = State {
        task_id,
        budget,
        rng_one,
        rng_two,
        rng_active: rng.is_some().into(),
    };
    ACTIVE.set(&raw mut state);
    let _reset = Reset {
        state: &raw mut state,
    };
    call((&raw mut state).cast())
}

fn execution_state() -> *mut State {
    ACTIVE.get()
}

#[path = "../execution.rs"]
mod access;
pub(super) use access::budget;
#[cfg(any(feature = "macros", all(feature = "sync", feature = "rt")))]
pub(super) use access::rng;
#[cfg(feature = "rt")]
pub(super) use access::task_id;
