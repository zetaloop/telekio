use std::{cell::Cell, ffi::c_void};

thread_local! {
    static EXECUTION: Cell<*mut ExecutionState> = const { Cell::new(std::ptr::null_mut()) };
}

#[derive(Clone, Copy)]
#[repr(C, u8)]
pub enum RuntimeContext {
    NotEntered,
    Entered { allow_block_in_place: bool },
}

#[repr(C)]
pub struct ExecutionState {
    pub runtime: RuntimeContext,
    pub task_id: u64,
    pub budget: u16,
    pub rng_one: u32,
    pub rng_two: u32,
    pub rng_active: u8,
    pub tracing: u8,
    pub forced_yields: u64,
    pub panic: *const c_void,
}

#[repr(C)]
struct PanicContext {
    previous: *const c_void,
    panicking: extern "C" fn() -> bool,
}

extern "C" fn panicking() -> bool {
    std::thread::panicking()
}

impl ExecutionState {
    /// # Safety
    ///
    /// The borrowed panic records must remain valid on the current thread.
    #[doc(hidden)]
    pub unsafe fn is_panicking(&self) -> bool {
        if std::thread::panicking() {
            return true;
        }
        let mut current = self.panic;
        while !current.is_null() {
            let context = unsafe { &*current.cast::<PanicContext>() };
            if (context.panicking)() {
                return true;
            }
            current = context.previous;
        }
        false
    }
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
    struct Reset {
        state: *mut ExecutionState,
        previous: *mut ExecutionState,
        panic: *const c_void,
    }

    impl Drop for Reset {
        fn drop(&mut self) {
            unsafe { (*self.state).panic = self.panic };
            EXECUTION.set(self.previous);
        }
    }

    assert!(!state.is_null(), "Tokio execution state is missing");
    let context = PanicContext {
        previous: unsafe { (*state).panic },
        panicking,
    };
    let _reset = Reset {
        state,
        previous: EXECUTION.replace(state),
        panic: context.previous,
    };
    unsafe { (*state).panic = (&raw const context).cast() };
    call()
}
