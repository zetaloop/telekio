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
        type Prepare = unsafe extern "C" fn(
            Entry,
            Option<unsafe extern "C" fn(OwnedBytes) -> !>,
        ) -> CallResult;
        #[cfg(panic = "abort")]
        let on_panic = Some(on_panic as unsafe extern "C" fn(OwnedBytes) -> !);
        #[cfg(panic = "unwind")]
        let on_panic = None;

        let path = std::env::var_os("TELEKIO_TEST_HOST").expect("test host path is not configured");
        let library = Library::new(path).expect("failed to load the test host");
        let prepare = library
            .get::<Prepare>(b"prepare")
            .expect("test host has no prepare entry");
        prepare(telekio_guest_context, on_panic).resume("failed to attach the test guest");
        library
    });
    telekio_abi::attached()
}

#[cfg(panic = "abort")]
unsafe extern "C" fn on_panic(payload: OwnedBytes) -> ! {
    std::panic::panic_any(unsafe { payload.into_string() })
}
