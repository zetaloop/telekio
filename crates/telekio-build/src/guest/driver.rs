#[cfg(all(unix, any(feature = "process", feature = "signal")))]
use super::*;

#[cfg(all(unix, any(feature = "process", feature = "signal")))]
impl Handle {
    #[track_caller]
    pub(crate) fn telekio_signal(&self) -> &crate::runtime::signal::Handle {
        if cfg!(all(
            not(test),
            feature = "rt",
            any(feature = "process", feature = "signal")
        )) {
            static HANDLE: std::sync::OnceLock<crate::runtime::signal::Handle> =
                std::sync::OnceLock::new();
            HANDLE.get_or_init(Default::default)
        } else {
            self.signal()
        }
    }
}
