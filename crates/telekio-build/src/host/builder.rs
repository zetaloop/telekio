use super::{Builder, TimerFlavor};

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
}
