use super::{Builder, TimerFlavor};
use crate::runtime::HistogramBuilder;

impl Builder {
    #[doc(hidden)]
    pub fn telekio_disable_lifo_slot(&mut self) {
        self.disable_lifo_slot = true;
    }

    #[doc(hidden)]
    pub fn telekio_enable_eager_driver_handoff(&mut self) {
        self.enable_eager_driver_handoff = true;
    }

    #[doc(hidden)]
    pub fn telekio_enable_alt_timer(&mut self) {
        self.enable_time();
        self.timer_flavor = TimerFlavor::Alternative;
    }

    #[doc(hidden)]
    pub fn telekio_poll_histogram(&mut self, kind: u8, a: u64, b: u64, c: u64) {
        let histogram = HistogramBuilder::telekio_from_parts(kind, a, b, c);
        self.metrics_poll_count_histogram_enable = histogram.is_some();
        if let Some(histogram) = histogram {
            self.metrics_poll_count_histogram = histogram;
        }
    }

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
