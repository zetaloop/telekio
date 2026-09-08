#[cfg(feature = "test-util")]
use std::future::Future;
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::OnceLock,
    time::Duration,
};
#[cfg(feature = "time")]
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context as TaskContext, Poll as RustPoll},
};

use telekio_abi::{CallResult, ClockSample, DurationParts, InstantOffset, Timer, TimerResult};
#[cfg(feature = "time")]
use telekio_abi::{OperationPoll, OwnedBytes, Poll, Status, Waker};

#[cfg(feature = "time")]
use super::HandleContext;
use crate::bridge::host_panic;
#[cfg(feature = "time")]
use crate::bridge::{host_callback, owner::HostResource, result};

static CLOCK_ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();

#[cfg(feature = "time")]
struct TimeTimer {
    handle: crate::runtime::scheduler::Handle,
    timer: Pin<Box<crate::runtime::Timer>>,
}

pub(super) unsafe extern "C" fn clock(context: *const c_void) -> telekio_abi::ClockResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let origin = *CLOCK_ORIGIN.get_or_init(std::time::Instant::now);
        let realtime = instant_offset(origin, std::time::Instant::now());
        #[cfg(feature = "time")]
        let logical = {
            let context = unsafe { &*context.cast::<HandleContext>() };
            let _guard = context.handle.enter();
            instant_offset(origin, crate::time::Instant::now().into_std())
        };
        #[cfg(not(feature = "time"))]
        let logical = {
            let _ = context;
            realtime
        };
        ClockSample { realtime, logical }
    })) {
        Ok(value) => telekio_abi::ClockResult {
            call: CallResult::ok(),
            value,
        },
        Err(payload) => telekio_abi::ClockResult {
            call: host_panic(&*payload),
            value: ClockSample {
                realtime: InstantOffset {
                    duration: DurationParts::new(Duration::ZERO),
                    negative: 0,
                },
                logical: InstantOffset {
                    duration: DurationParts::new(Duration::ZERO),
                    negative: 0,
                },
            },
        },
    }
}

#[cfg(feature = "test-util")]
pub(super) unsafe extern "C" fn pause(context: *const c_void) -> CallResult {
    time_call(context, crate::time::pause)
}

#[cfg(feature = "test-util")]
pub(super) unsafe extern "C" fn resume(context: *const c_void) -> CallResult {
    time_call(context, crate::time::resume)
}

#[cfg(not(feature = "test-util"))]
pub(super) unsafe extern "C" fn pause(_: *const c_void) -> CallResult {
    CallResult::error("Tokio host time control requires test-util")
}

#[cfg(not(feature = "test-util"))]
pub(super) use pause as resume;

#[cfg(feature = "test-util")]
fn time_call(context: *const c_void, call: impl FnOnce()) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let _guard = context.handle.enter();
        call();
    })) {
        Ok(()) => result(Status::Ok, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

#[cfg(feature = "test-util")]
pub(super) unsafe extern "C" fn advance(
    context: *const c_void,
    duration: DurationParts,
) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let _guard = context.handle.enter();
        let mut operation = std::pin::pin!(crate::time::advance(duration.duration()));
        let mut context = TaskContext::from_waker(std::task::Waker::noop());
        _ = operation.as_mut().poll(&mut context);
    })) {
        Ok(()) => result(Status::Ok, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

#[cfg(not(feature = "test-util"))]
pub(super) unsafe extern "C" fn advance(_: *const c_void, _: DurationParts) -> CallResult {
    CallResult::error("Tokio host time control requires test-util")
}

#[cfg(feature = "time")]
pub(super) unsafe extern "C" fn timer(
    context: *const c_void,
    deadline: InstantOffset,
) -> TimerResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let handle = context.handle.inner.clone();
        let deadline = time_instant(deadline);
        let mut timer = Box::pin(crate::runtime::Timer::new(handle.clone(), deadline));
        timer.as_mut().init(deadline);
        HostResource::new(&context.owner, TimeTimer { handle, timer })
    })) {
        Ok(Ok(timer)) => TimerResult {
            call: result(Status::Ok, OwnedBytes::empty()),
            timer: unsafe {
                Timer::from_raw(
                    Arc::as_ptr(&timer).cast_mut().cast(),
                    poll_time_timer,
                    reset_time_timer,
                    time_timer_elapsed,
                    release_time_timer,
                )
            },
        },
        Ok(Err(error)) => TimerResult {
            call: result(Status::Error, OwnedBytes::from_string(error)),
            timer: Timer::empty(),
        },
        Err(payload) => TimerResult {
            call: host_panic(&*payload),
            timer: Timer::empty(),
        },
    }
}

#[cfg(not(feature = "time"))]
pub(super) unsafe extern "C" fn timer(_: *const c_void, _: InstantOffset) -> TimerResult {
    TimerResult {
        call: CallResult::error("Tokio host timers require time"),
        timer: Timer::empty(),
    }
}

#[cfg(feature = "time")]
unsafe extern "C" fn poll_time_timer(data: *mut c_void, waker: *const Waker) -> OperationPoll {
    match catch_unwind(AssertUnwindSafe(|| {
        let timer = unsafe { &*data.cast::<HostResource<TimeTimer>>() };
        timer.update_waker(unsafe { &*waker });
        let waker = unsafe { (*waker).clone_rust_waker() };
        let mut context = TaskContext::from_waker(&waker);
        timer.with_mut(|timer| {
            timer
                .timer
                .as_mut()
                .poll_elapsed(&mut context)
                .map(|result| result.unwrap_or_else(|error| panic!("timer error: {error}")))
        })
    })) {
        Ok(Ok(RustPoll::Pending)) => {
            time_poll(Poll::Pending, result(Status::Ok, OwnedBytes::empty()))
        }
        Ok(Ok(RustPoll::Ready(()))) => {
            time_poll(Poll::Ready, result(Status::Ok, OwnedBytes::empty()))
        }
        Ok(Err(error)) => time_poll(
            Poll::Ready,
            result(Status::Error, OwnedBytes::from_string(error)),
        ),
        Err(payload) => time_poll(Poll::Panicked, host_panic(&*payload)),
    }
}

#[cfg(feature = "time")]
unsafe extern "C" fn reset_time_timer(data: *mut c_void, deadline: InstantOffset) -> CallResult {
    let timer = unsafe { &*data.cast::<HostResource<TimeTimer>>() };
    match catch_unwind(AssertUnwindSafe(|| {
        timer.with_mut(|timer| {
            timer
                .timer
                .as_mut()
                .reset(timer.handle.clone(), time_instant(deadline));
        })
    })) {
        Ok(Ok(())) => result(Status::Ok, OwnedBytes::empty()),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}

#[cfg(feature = "time")]
unsafe extern "C" fn time_timer_elapsed(data: *const c_void) -> telekio_abi::BoolResult {
    match catch_unwind(AssertUnwindSafe(|| {
        unsafe { &*data.cast::<HostResource<TimeTimer>>() }
            .with(|timer| timer.timer.is_elapsed())
            .unwrap_or(true)
    })) {
        Ok(value) => telekio_abi::BoolResult {
            call: CallResult::ok(),
            value,
        },
        Err(payload) => telekio_abi::BoolResult {
            call: host_panic(&*payload),
            value: true,
        },
    }
}

#[cfg(feature = "time")]
unsafe extern "C" fn release_time_timer(data: *mut c_void) -> CallResult {
    host_callback(|| unsafe { &*data.cast::<HostResource<TimeTimer>>() }.release())
}

#[cfg(feature = "time")]
fn time_instant(offset: InstantOffset) -> crate::time::Instant {
    let origin = *CLOCK_ORIGIN.get_or_init(std::time::Instant::now);
    crate::time::Instant::from_std(offset.apply(origin))
}

#[cfg(feature = "time")]
fn time_poll(state: Poll, call: CallResult) -> OperationPoll {
    OperationPoll { state, call }
}

fn instant_offset(origin: std::time::Instant, instant: std::time::Instant) -> InstantOffset {
    match instant.checked_duration_since(origin) {
        Some(duration) => InstantOffset {
            duration: DurationParts::new(duration),
            negative: 0,
        },
        None => InstantOffset {
            duration: DurationParts::new(origin.duration_since(instant)),
            negative: 1,
        },
    }
}
