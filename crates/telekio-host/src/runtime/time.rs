use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::{Arc, OnceLock},
    task::{Context as TaskContext, Poll as RustPoll},
    time::Duration,
};

use telekio::{
    CallResult, ClockSample, DurationParts, InstantOffset, OperationPoll, OwnedBytes, Poll, Status,
    Timer, TimerResult, Waker,
};

use super::HandleContext;
use crate::owner::HostResource;
use crate::{host_callback, host_panic, result};

static CLOCK_ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();

struct TimeTimer {
    sleep: Pin<Box<tokio::time::Sleep>>,
}

pub(super) unsafe extern "C" fn clock(context: *const c_void) -> telekio::ClockResult {
    match catch_unwind(AssertUnwindSafe(|| {
        let context = unsafe { &*context.cast::<HandleContext>() };
        let origin = *CLOCK_ORIGIN.get_or_init(std::time::Instant::now);
        let realtime = instant_offset(origin, std::time::Instant::now());
        let _guard = context.handle.enter();
        let logical = instant_offset(origin, tokio::time::Instant::now().into_std());
        ClockSample { realtime, logical }
    })) {
        Ok(value) => telekio::ClockResult {
            call: CallResult::ok(),
            value,
        },
        Err(payload) => telekio::ClockResult {
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

pub(super) unsafe extern "C" fn pause(context: *const c_void) -> CallResult {
    time_call(context, tokio::time::pause)
}

pub(super) unsafe extern "C" fn resume(context: *const c_void) -> CallResult {
    time_call(context, tokio::time::resume)
}

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

pub(super) unsafe extern "C" fn advance(
    context: *const c_void,
    duration: DurationParts,
) -> CallResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let _guard = context.handle.enter();
        let mut operation = std::pin::pin!(tokio::time::advance(duration.duration()));
        let mut context = TaskContext::from_waker(std::task::Waker::noop());
        _ = operation.as_mut().poll(&mut context);
    })) {
        Ok(()) => result(Status::Ok, OwnedBytes::empty()),
        Err(payload) => host_panic(&*payload),
    }
}

pub(super) unsafe extern "C" fn timer(
    context: *const c_void,
    deadline: InstantOffset,
) -> TimerResult {
    let context = unsafe { &*context.cast::<HandleContext>() };
    match catch_unwind(AssertUnwindSafe(|| {
        let _guard = context.handle.enter();
        HostResource::new(
            &context.owner,
            TimeTimer {
                sleep: Box::pin(tokio::time::sleep_until(time_instant(deadline))),
            },
        )
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

unsafe extern "C" fn poll_time_timer(data: *mut c_void, waker: *const Waker) -> OperationPoll {
    match catch_unwind(AssertUnwindSafe(|| {
        let timer = unsafe { &*data.cast::<HostResource<TimeTimer>>() };
        timer.update_waker(unsafe { &*waker });
        let waker = unsafe { (*waker).clone_rust_waker() };
        let mut context = TaskContext::from_waker(&waker);
        timer.with_mut(|timer| timer.sleep.as_mut().poll(&mut context))
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

unsafe extern "C" fn reset_time_timer(data: *mut c_void, deadline: InstantOffset) -> CallResult {
    let timer = unsafe { &*data.cast::<HostResource<TimeTimer>>() };
    match catch_unwind(AssertUnwindSafe(|| {
        timer.with_mut(|timer| {
            timer.sleep.as_mut().reset(time_instant(deadline));
        })
    })) {
        Ok(Ok(())) => result(Status::Ok, OwnedBytes::empty()),
        Ok(Err(error)) => result(Status::Error, OwnedBytes::from_string(error)),
        Err(payload) => host_panic(&*payload),
    }
}

unsafe extern "C" fn time_timer_elapsed(data: *const c_void) -> telekio::BoolResult {
    match catch_unwind(AssertUnwindSafe(|| {
        unsafe { &*data.cast::<HostResource<TimeTimer>>() }
            .with(|timer| timer.sleep.is_elapsed())
            .unwrap_or(true)
    })) {
        Ok(value) => telekio::BoolResult {
            call: CallResult::ok(),
            value,
        },
        Err(payload) => telekio::BoolResult {
            call: host_panic(&*payload),
            value: true,
        },
    }
}

unsafe extern "C" fn release_time_timer(data: *mut c_void) -> CallResult {
    host_callback(|| unsafe { &*data.cast::<HostResource<TimeTimer>>() }.release())
}

fn time_instant(offset: InstantOffset) -> tokio::time::Instant {
    let origin = *CLOCK_ORIGIN.get_or_init(std::time::Instant::now);
    tokio::time::Instant::from_std(offset.apply(origin))
}

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
