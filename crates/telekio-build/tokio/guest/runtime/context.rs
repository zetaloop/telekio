#[cfg(feature = "rt")]
use std::{
    cell::Cell,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

type State = ::telekio_abi::ExecutionState;

fn execution_state() -> *mut State {
    ::telekio_abi::execution_state()
}

#[path = "../../shared/runtime/context.rs"]
mod access;
pub(super) use access::budget;
#[cfg(any(feature = "macros", all(feature = "sync", feature = "rt")))]
pub(super) use access::rng;
#[cfg(feature = "rt")]
pub(super) use access::{enter_runtime, runtime, task_id};

#[cfg(feature = "rt")]
thread_local! {
    static ACTIVE: Cell<usize> = const { Cell::new(0) };
}

#[cfg(feature = "rt-multi-thread")]
thread_local! {
    static BLOCKING_BUDGET: Cell<Option<crate::task::coop::Budget>> = const { Cell::new(None) };
}

#[cfg(feature = "rt")]
struct Guard;

#[cfg(feature = "rt")]
impl Drop for Guard {
    fn drop(&mut self) {
        ACTIVE.set(ACTIVE.get() - 1);
    }
}

#[cfg(feature = "rt")]
pub(crate) struct Active<F>(F);

#[cfg(feature = "rt")]
pub(crate) fn active<F: Future>(future: F) -> Active<F> {
    Active(future)
}

#[cfg(feature = "rt")]
pub(crate) fn enter<R>(call: impl FnOnce() -> R) -> R {
    ACTIVE.set(ACTIVE.get() + 1);
    let _guard = Guard;
    call()
}

#[cfg(feature = "rt")]
#[track_caller]
pub(crate) fn block_on<F, R>(
    handle: &crate::runtime::scheduler::Handle,
    allow_block_in_place: bool,
    call: F,
) -> R
where
    F: FnOnce(&mut super::BlockingRegionGuard) -> R,
{
    match super::try_enter_blocking_region() {
        Some(mut blocking) => {
            let _guard = super::try_set_current(handle);
            call(&mut blocking)
        }
        None => super::enter_runtime(handle, allow_block_in_place, call),
    }
}

#[cfg(feature = "rt-multi-thread")]
pub(crate) fn stop() -> crate::task::coop::Budget {
    let budget = crate::task::coop::stop();
    BLOCKING_BUDGET.set(Some(budget));
    budget
}

#[cfg(feature = "rt-multi-thread")]
pub(crate) fn restore() {
    if let Some(budget) = BLOCKING_BUDGET.take() {
        crate::task::coop::set(budget);
    }
}

#[cfg(feature = "rt")]
impl<F: Future> Future for Active<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let future = unsafe { self.map_unchecked_mut(|active| &mut active.0) };
        enter(|| future.poll(context))
    }
}

#[cfg(all(tokio_unstable, feature = "rt"))]
pub(super) fn worker_index<F>(
    _: F,
) -> impl FnOnce(Option<&crate::runtime::scheduler::Context>) -> Option<usize>
where
    F: FnOnce(Option<&crate::runtime::scheduler::Context>) -> Option<usize>,
{
    move |_| {
        super::with_current(|handle| {
            crate::runtime::metrics::telekio::worker_index(&handle.connection().handle)
        })
        .ok()
        .flatten()
    }
}

#[cfg(feature = "rt")]
pub(crate) fn defer(waker: &std::task::Waker) {
    if ACTIVE.get() == 0 {
        super::defer(waker);
        return;
    }
    match super::with_current(|handle| {
        let waker = unsafe { ::telekio_abi::Waker::from_ref(waker) };
        handle.connection().handle.defer(&waker)
    }) {
        Ok(result) => result
            .into_io_result()
            .expect("failed to defer a Tokio task"),
        Err(_) => waker.wake_by_ref(),
    }
}
