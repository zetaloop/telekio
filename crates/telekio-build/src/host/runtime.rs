#[cfg(target_os = "freebsd")]
pub use super::handle::telekio::TelekioAio;
#[cfg(target_os = "linux")]
pub use super::handle::telekio::TelekioIo;

#[cfg(unix)]
pub fn reap_process(id: u32) {
    crate::process::unix::telekio::push(id);
}
