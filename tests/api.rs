use std::ffi::c_void;

#[repr(C)]
pub struct PluginFuture {
    pub data: *mut c_void,
    pub poll: unsafe extern "C" fn(*mut c_void, *const telekio_abi::Waker) -> PluginPoll,
    pub release: unsafe extern "C" fn(*mut c_void) -> telekio_abi::CallResult,
}

#[repr(C)]
pub struct PluginPoll {
    pub state: telekio_abi::Poll,
    pub value: u64,
    pub task_id: u64,
}
