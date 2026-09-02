use super::Builder;

impl Builder {
    #[doc(hidden)]
    pub fn telekio_disable_lifo_slot(&mut self) {
        self.disable_lifo_slot = true;
    }
}
