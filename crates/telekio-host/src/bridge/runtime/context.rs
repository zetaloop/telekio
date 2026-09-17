use std::{
    future::Future as RustFuture,
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll as RustPoll},
};

use telekio_abi::{CallResult, ExecutionState, Future, Poll, Status, Waker};

use crate::bridge::owner::{Activity, OwnerState};

fn with_execution<R>(call: impl FnOnce(*mut ExecutionState) -> R) -> R {
    crate::runtime::telekio::with_execution(|state| call(state.cast()))
}

pub(super) fn with_task_execution<R>(call: impl FnOnce(*mut ExecutionState) -> R) -> R {
    crate::runtime::telekio::with_task_execution(|state| call(state.cast()))
}

pub(super) fn block_on_result(status: Status) -> CallResult {
    match status {
        Status::Error => CallResult::error("Tokio owner is shutting down"),
        status => CallResult {
            status,
            ..CallResult::ok()
        },
    }
}

pub(super) struct GuestFuture {
    future: Future,
    owner: Arc<OwnerState>,
    activity: u64,
}

impl GuestFuture {
    pub(super) fn new(future: Future, activity: &Activity) -> Self {
        Self {
            future,
            owner: Arc::clone(&activity.owner),
            activity: activity.id,
        }
    }
}

impl RustFuture for GuestFuture {
    type Output = Status;

    fn poll(mut self: Pin<&mut Self>, context: &mut TaskContext<'_>) -> RustPoll<Self::Output> {
        if !self
            .owner
            .update_activity_waker(self.activity, context.waker())
        {
            return RustPoll::Ready(Status::Error);
        }
        let waker = unsafe { Waker::from_ref(context.waker()) };
        match with_execution(|state| self.future.poll(unsafe { &mut *state }, &waker)) {
            Poll::Pending => RustPoll::Pending,
            Poll::Ready => RustPoll::Ready(Status::Ok),
            Poll::Panicked => RustPoll::Ready(Status::Panicked),
        }
    }
}
