use super::Id;

impl Id {
    #[doc(hidden)]
    pub fn telekio_value(&self) -> u64 {
        self.0.get()
    }
}
