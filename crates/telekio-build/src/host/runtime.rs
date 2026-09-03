#[cfg(target_os = "freebsd")]
pub use super::handle::telekio::TelekioAio;
#[cfg(target_os = "linux")]
pub use super::handle::telekio::TelekioIo;

#[cfg(feature = "rt")]
pub fn next_task_id() -> u64 {
    super::task::Id::next_local().telekio_value()
}

#[cfg(feature = "rt")]
pub fn with_task_id<R>(id: u64, call: impl FnOnce() -> R) -> R {
    super::task::Id::with_telekio(id, call)
}

#[cfg(unix)]
pub fn reap_process(id: u32) {
    crate::process::unix::telekio::push(id);
}
