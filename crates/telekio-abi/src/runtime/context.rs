use std::cell::Cell;

thread_local! {
    static EXECUTION: Cell<*mut ExecutionState> = const { Cell::new(std::ptr::null_mut()) };
}

#[repr(C)]
pub struct ExecutionState {
    pub task_id: u64,
    pub budget: u16,
    pub rng_one: u32,
    pub rng_two: u32,
    pub rng_active: u8,
    pub tracing: u8,
    pub forced_yields: u64,
}

#[doc(hidden)]
pub fn execution_state() -> *mut ExecutionState {
    EXECUTION.get()
}

/// # Safety
///
/// `state` must remain exclusively available on the current thread for the
/// duration of `call`.
#[doc(hidden)]
pub unsafe fn with_execution_state<R>(state: *mut ExecutionState, call: impl FnOnce() -> R) -> R {
    struct Reset(*mut ExecutionState);

    impl Drop for Reset {
        fn drop(&mut self) {
            EXECUTION.set(self.0);
        }
    }

    assert!(!state.is_null(), "Tokio execution state is missing");
    let previous = EXECUTION.replace(state);
    let _reset = Reset(previous);
    call()
}
