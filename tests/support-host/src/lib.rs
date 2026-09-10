use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::OnceLock,
};

use telekio_abi::{CallResult, OwnedBytes};
use telekio_host::{Attach, Attachment};
use tokio::runtime::{Builder, Runtime};

/// Supplies the current process's Guest with a Host runtime.
///
/// # Safety
///
/// The Guest entry and callback must remain valid for the process lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prepare(
    entry: Attach,
    on_panic: Option<unsafe extern "C" fn(OwnedBytes) -> !>,
) -> CallResult {
    static HOST: OnceLock<io::Result<(Runtime, Attachment)>> = OnceLock::new();
    match catch_unwind(AssertUnwindSafe(|| {
        match HOST.get_or_init(|| {
            if let Some(on_panic) = on_panic {
                std::panic::set_hook(Box::new(move |info| unsafe {
                    on_panic(CallResult::panicked(info.payload()).payload)
                }));
            }
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
