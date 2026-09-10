use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::OnceLock,
};

use telekio_abi::CallResult;
use telekio_host::{Attach, Attachment};
use tokio::runtime::{Builder, Runtime};

/// Supplies the current process's Guest with a Host runtime.
///
/// # Safety
///
/// The Guest entry and its code must remain loaded for the process lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prepare(entry: Attach) -> CallResult {
    static HOST: OnceLock<io::Result<(Runtime, Attachment)>> = OnceLock::new();
    match catch_unwind(AssertUnwindSafe(|| {
        match HOST.get_or_init(|| {
            let runtime = Builder::new_current_thread().enable_all().build()?;
            let attachment = {
                let _guard = runtime.enter();
                unsafe { telekio_host::attach(entry)? }
            };
            Ok((runtime, attachment))
        }) {
            Ok(_) => CallResult::ok(),
            Err(error) => CallResult::error(&error.to_string()),
        }
    })) {
        Ok(result) => result,
        Err(payload) => CallResult::panicked(&*payload),
    }
}
