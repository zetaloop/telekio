use super::*;

pub(crate) struct Timer {
    handle: scheduler::Handle,
    timer: Option<::telekio_abi::Timer>,
}

impl Timer {
    pub(crate) fn new(handle: scheduler::Handle, _: Instant) -> Self {
        Self {
            handle,
            timer: None,
        }
    }

    pub(crate) fn init(mut self: Pin<&mut Self>, deadline: Instant) {
        self.as_mut().get_mut().set(deadline);
    }

    pub(crate) fn is_elapsed(&self) -> bool {
        self.timer
            .as_ref()
            .is_some_and(::telekio_abi::Timer::is_elapsed)
    }

    pub(crate) fn reset(mut self: Pin<&mut Self>, handle: scheduler::Handle, deadline: Instant) {
        self.handle = handle;
        self.as_mut().get_mut().set(deadline);
    }

    pub(crate) fn poll_elapsed(
        mut self: Pin<&mut Self>,
        context: &mut task::Context<'_>,
    ) -> Poll<Result<(), Error>> {
        self.timer
            .as_mut()
            .expect("timer was not initialized")
            .poll(context)
            .map(Ok)
    }

    fn set(&mut self, deadline: Instant) {
        #[cfg(feature = "rt")]
        {
            let deadline = deadline.into_std();
            if let Some(timer) = &mut self.timer {
                timer.reset(deadline);
            } else {
                self.timer = Some(::telekio_abi::Timer::from_result(
                    self.handle.connection().handle.timer(deadline),
                ));
            }
        }
        #[cfg(not(feature = "rt"))]
        {
            let _ = deadline;
            panic!("{}", crate::util::error::CONTEXT_MISSING_ERROR);
        }
    }
}

impl std::fmt::Debug for Timer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Timer").finish_non_exhaustive()
    }
}
