use super::Builder;
#[cfg(all(tokio_unstable, feature = "time", feature = "rt-multi-thread"))]
use super::TimerFlavor;
#[cfg(tokio_unstable)]
use crate::runtime::{HistogramBuilder, UnhandledPanic};
use crate::util::{rand::RngSeed, RngSeedGenerator};

impl Builder {
    #[doc(hidden)]
    pub fn telekio_rng_seed(&mut self, one: u32, two: u32) {
        self.seed_generator = RngSeedGenerator::new(RngSeed::from_telekio(one, two));
    }

    #[cfg(tokio_unstable)]
    #[doc(hidden)]
    pub fn telekio_unhandled_panic(&mut self, shutdown: bool) {
        self.unhandled_panic = if shutdown {
            UnhandledPanic::ShutdownRuntime
        } else {
            UnhandledPanic::Ignore
        };
    }

    #[cfg(tokio_unstable)]
    #[doc(hidden)]
    pub fn telekio_disable_lifo_slot(&mut self) {
        self.disable_lifo_slot = true;
    }

    #[cfg(all(tokio_unstable, feature = "rt-multi-thread"))]
    #[doc(hidden)]
    pub fn telekio_enable_eager_driver_handoff(&mut self) {
        self.enable_eager_driver_handoff = true;
    }

    #[cfg(all(tokio_unstable, feature = "time", feature = "rt-multi-thread"))]
    #[doc(hidden)]
    pub fn telekio_enable_alt_timer(&mut self) {
        self.enable_time();
        self.timer_flavor = TimerFlavor::Alternative;
    }

    #[cfg(tokio_unstable)]
    #[doc(hidden)]
    pub fn telekio_poll_histogram(&mut self, kind: u8, a: u64, b: u64, c: u64) {
        let histogram = HistogramBuilder::telekio_from_parts(kind, a, b, c);
        self.metrics_poll_count_histogram_enable = histogram.is_some();
        if let Some(histogram) = histogram {
            self.metrics_poll_count_histogram = histogram;
        }
    }

    #[cfg(all(tokio_unstable, feature = "schedule-latency"))]
    #[doc(hidden)]
    pub fn telekio_schedule_histogram(&mut self, kind: u8, a: u64, b: u64, c: u64) {
        #[cfg(all(any(unix, windows), target_pointer_width = "64"))]
        {
            let histogram = HistogramBuilder::telekio_from_parts(kind, a, b, c);
            self.metrics_schedule_latency_histogram_enabled = histogram.is_some();
            if let Some(histogram) = histogram {
                self.metrics_schedule_latency_histogram = histogram;
            }
        }
        #[cfg(not(all(any(unix, windows), target_pointer_width = "64")))]
        let _ = (kind, a, b, c);
    }
}
