use std::{
    ffi::c_void,
    sync::OnceLock,
    task::{Context, Poll as RustPoll},
    time::Duration,
};

use crate::{CallResult, Handle, Poll, Status, Waker};

static CLOCK_ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();

#[derive(Clone, Copy)]
#[repr(C)]
pub struct DurationParts {
    pub seconds: u64,
    pub nanoseconds: u32,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct InstantOffset {
    pub duration: DurationParts,
    pub negative: u8,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct ClockSample {
    pub realtime: InstantOffset,
    pub logical: InstantOffset,
}

#[repr(C)]
pub struct Timer {
    pub data: *mut c_void,
    pub poll: unsafe extern "C" fn(*mut c_void, *const Waker) -> OperationPoll,
    pub reset: unsafe extern "C" fn(*mut c_void, DurationParts) -> CallResult,
    pub is_elapsed: unsafe extern "C" fn(*const c_void) -> bool,
    pub release: unsafe extern "C" fn(*mut c_void),
}

#[repr(C)]
pub struct TimerResult {
    pub call: CallResult,
    pub timer: Timer,
}

#[repr(C)]
pub struct OperationPoll {
    pub state: Poll,
    pub call: CallResult,
}

unsafe impl Send for Timer {}
unsafe impl Sync for Timer {}

impl InstantOffset {
    pub fn apply(self, instant: std::time::Instant) -> std::time::Instant {
        if self.negative == 0 {
            instant + self.duration.duration()
        } else {
            instant - self.duration.duration()
        }
    }

    pub fn remove(self, instant: std::time::Instant) -> std::time::Instant {
        if self.negative == 0 {
            instant - self.duration.duration()
        } else {
            instant + self.duration.duration()
        }
    }
}

impl DurationParts {
    pub fn new(duration: Duration) -> Self {
        Self {
            seconds: duration.as_secs(),
            nanoseconds: duration.subsec_nanos(),
        }
    }

    pub fn duration(self) -> Duration {
        Duration::new(self.seconds, self.nanoseconds)
    }
}

impl Handle {
    #[doc(hidden)]
    pub fn clock(&self) -> ClockSample {
        unsafe { ((*self.raw.api).clock)(self.raw.context) }
    }

    #[doc(hidden)]
    pub fn now(&self) -> std::time::Instant {
        let sample = self.clock();
        let origin =
            *CLOCK_ORIGIN.get_or_init(|| sample.realtime.remove(std::time::Instant::now()));
        sample.logical.apply(origin)
    }

    #[doc(hidden)]
    pub fn pause(&self) -> CallResult {
        unsafe { ((*self.raw.api).pause)(self.raw.context) }
    }

    #[doc(hidden)]
    pub fn resume(&self) -> CallResult {
        unsafe { ((*self.raw.api).resume)(self.raw.context) }
    }

    #[doc(hidden)]
    pub fn advance(&self, duration: Duration) -> CallResult {
        unsafe { ((*self.raw.api).advance)(self.raw.context, DurationParts::new(duration)) }
    }

    #[doc(hidden)]
    pub fn timer(&self, duration: Duration) -> TimerResult {
        unsafe { ((*self.raw.api).timer)(self.raw.context, DurationParts::new(duration)) }
    }
}

impl Timer {
    pub fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            poll: poll_empty,
            reset: reset_empty,
            is_elapsed: elapsed_empty,
            release: release_empty,
        }
    }

    pub fn from_result(result: TimerResult) -> Self {
        result
            .call
            .into_io_result()
            .expect("failed to create Tokio timer");
        result.timer
    }

    pub fn poll(&mut self, context: &mut Context<'_>) -> RustPoll<()> {
        let waker = Waker::from_ref(context.waker());
        let result = unsafe { (self.poll)(self.data, &raw const waker) };
        poll_result(result)
    }

    pub fn reset(&mut self, duration: Duration) {
        unsafe { (self.reset)(self.data, DurationParts::new(duration)) }
            .into_io_result()
            .expect("failed to reset Tokio timer");
    }

    pub fn is_elapsed(&self) -> bool {
        unsafe { (self.is_elapsed)(self.data) }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        unsafe { (self.release)(self.data) };
    }
}

unsafe extern "C" fn poll_empty(_: *mut c_void, _: *const Waker) -> OperationPoll {
    OperationPoll {
        state: Poll::Ready,
        call: CallResult {
            status: Status::Ok,
            payload: crate::OwnedBytes::empty(),
        },
    }
}

unsafe extern "C" fn reset_empty(_: *mut c_void, _: DurationParts) -> CallResult {
    CallResult {
        status: Status::Ok,
        payload: crate::OwnedBytes::empty(),
    }
}

unsafe extern "C" fn elapsed_empty(_: *const c_void) -> bool {
    true
}

unsafe extern "C" fn release_empty(_: *mut c_void) {}

fn poll_result(result: OperationPoll) -> RustPoll<()> {
    match result.state {
        Poll::Pending => {
            unsafe { result.call.payload.release() };
            RustPoll::Pending
        }
        Poll::Ready => {
            result
                .call
                .into_io_result()
                .expect("host Tokio time operation failed");
            RustPoll::Ready(())
        }
        Poll::Panicked => match result.call.status {
            Status::HostPanicked | Status::Panicked => {
                result.call.into_io_result().unwrap();
                unreachable!()
            }
            _ => panic!("host Tokio time operation returned an invalid poll result"),
        },
    }
}
