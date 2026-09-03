use super::Id;

impl Id {
    pub(crate) fn from_telekio(value: u64) -> Self {
        Self(std::num::NonZeroU64::new(value).expect("invalid host Tokio task ID"))
    }

    pub(crate) fn telekio_value(self) -> u64 {
        self.0.get()
    }

    pub(crate) fn next() -> Self {
        #[cfg(any(telekio_host, feature = "telekio-test"))]
        let value = ::telekio_host::next_task_id();
        #[cfg(not(any(telekio_host, feature = "telekio-test")))]
        let value = ::telekio::attached().next_task_id();
        Self::from_telekio(value)
    }
}
