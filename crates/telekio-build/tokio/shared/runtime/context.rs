use super::super::Context;
#[cfg(any(feature = "macros", feature = "rt"))]
use super::super::FastRand;
use super::{execution_state, State};

pub(in crate::runtime::context) fn budget<F, R>(local: F) -> impl FnOnce(&Context) -> R
where
    F: FnOnce(&Context) -> R,
{
    move |context| {
        let state = execution_state();
        if state.is_null() {
            return local(context);
        }
        struct Reset<'a> {
            context: &'a Context,
            state: *mut State,
            previous: crate::task::coop::Budget,
        }
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                let state = unsafe { &mut *self.state };
                state.budget = self.context.budget.get().telekio_value();
                self.context.budget.set(self.previous);
            }
        }
        let previous = context
            .budget
            .replace(crate::task::coop::Budget::from_telekio(unsafe {
                (*state).budget
            }));
        let _reset = Reset {
            context,
            state,
            previous,
        };
        local(context)
    }
}

#[cfg(any(feature = "macros", feature = "rt"))]
pub(in crate::runtime::context) fn rng<F, R>(local: F) -> impl FnOnce(&Context) -> R
where
    F: FnOnce(&Context) -> R,
{
    move |context| {
        let state = execution_state();
        if state.is_null() {
            return local(context);
        }
        struct Reset<'a> {
            context: &'a Context,
            state: *mut State,
            previous: Option<FastRand>,
        }
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                let state = unsafe { &mut *self.state };
                let current = self.context.rng.get();
                if let Some(rng) = current {
                    (state.rng_one, state.rng_two) = rng.telekio_parts();
                    state.rng_active = 1;
                } else {
                    state.rng_active = 0;
                }
                self.context.rng.set(self.previous);
            }
        }
        let current = unsafe { &*state };
        let previous = context.rng.replace(
            (current.rng_active != 0)
                .then(|| FastRand::from_telekio(current.rng_one, current.rng_two)),
        );
        let _reset = Reset {
            context,
            state,
            previous,
        };
        local(context)
    }
}

#[cfg(feature = "rt")]
impl super::super::EnterRuntime {
    pub(in crate::runtime::context) fn from_telekio(value: ::telekio_abi::RuntimeContext) -> Self {
        match value {
            ::telekio_abi::RuntimeContext::NotEntered => Self::NotEntered,
            ::telekio_abi::RuntimeContext::Entered {
                allow_block_in_place,
            } => Self::Entered {
                allow_block_in_place,
            },
        }
    }

    pub(in crate::runtime::context) fn telekio_value(self) -> ::telekio_abi::RuntimeContext {
        match self {
            Self::NotEntered => ::telekio_abi::RuntimeContext::NotEntered,
            Self::Entered {
                allow_block_in_place,
            } => ::telekio_abi::RuntimeContext::Entered {
                allow_block_in_place,
            },
        }
    }
}

#[cfg(feature = "rt")]
pub(in crate::runtime::context) fn runtime<F, R>(local: F) -> impl FnOnce(&Context) -> R
where
    F: FnOnce(&Context) -> R,
{
    move |context| {
        let state = execution_state();
        if state.is_null() {
            return local(context);
        }
        struct Reset<'a> {
            context: &'a Context,
            state: *mut State,
            previous: super::super::EnterRuntime,
        }
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                unsafe { (*self.state).runtime = self.context.runtime.get().telekio_value() };
                self.context.runtime.set(self.previous);
            }
        }
        let previous = context
            .runtime
            .replace(super::super::EnterRuntime::from_telekio(unsafe {
                (*state).runtime
            }));
        let _reset = Reset {
            context,
            state,
            previous,
        };
        local(context)
    }
}

#[cfg(feature = "rt")]
pub(in crate::runtime::context) fn enter_runtime<F, R>(local: F) -> impl FnOnce(&Context) -> R
where
    F: FnOnce(&Context) -> R,
{
    runtime(rng(local))
}

#[cfg(feature = "rt")]
pub(in crate::runtime::context) fn task_id<F, R>(local: F) -> impl FnOnce(&Context) -> R
where
    F: FnOnce(&Context) -> R,
{
    move |context| {
        let state = execution_state();
        if state.is_null() {
            return local(context);
        }
        struct Reset<'a> {
            context: &'a Context,
            state: *mut State,
            previous: Option<crate::runtime::task::Id>,
        }
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                let state = unsafe { &mut *self.state };
                state.task_id = self
                    .context
                    .current_task_id
                    .get()
                    .map_or(0, crate::runtime::task::Id::telekio_value);
                self.context.current_task_id.set(self.previous);
            }
        }
        let current = unsafe { &*state };
        let previous = context.current_task_id.replace(
            (current.task_id != 0).then(|| crate::runtime::task::Id::from_telekio(current.task_id)),
        );
        let _reset = Reset {
            context,
            state,
            previous,
        };
        local(context)
    }
}
