use std::sync::OnceLock;

use libloading::Library;
pub use telekio_abi::*;

pub fn attached() -> Handle {
    static HOST: OnceLock<Library> = OnceLock::new();
    HOST.get_or_init(|| unsafe {
        unsafe extern "C" {
            fn telekio_guest_context(handle: RawHandle) -> AttachResult;
        }
        type Entry = unsafe extern "C" fn(RawHandle) -> AttachResult;
        type Prepare = unsafe extern "C" fn(Entry) -> CallResult;

        let path = std::env::var_os("TELEKIO_TEST_HOST").expect("test host path is not configured");
        let library = Library::new(path).expect("failed to load the test host");
        let prepare = library
            .get::<Prepare>(b"prepare")
            .expect("test host has no prepare entry");
        prepare(telekio_guest_context).resume("failed to attach the test guest");
        library
    });
    telekio_abi::attached()
}
