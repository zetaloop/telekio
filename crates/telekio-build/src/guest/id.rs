use super::Id;

impl Id {
    pub(crate) fn next() -> Self {
        #[cfg(any(telekio_host, feature = "telekio-test"))]
        let value = ::telekio_host::next_task_id();
        #[cfg(not(any(telekio_host, feature = "telekio-test")))]
        let value = ::telekio::attached().next_task_id();
        Self(std::num::NonZeroU64::new(value).expect("invalid host Tokio task ID"))
    }
}
