use std::{slice, str};

#[repr(C)]
pub struct OwnedBytes {
    data: *mut u8,
    len: usize,
    release: unsafe extern "C" fn(*mut u8, usize),
}

impl OwnedBytes {
    pub fn empty() -> Self {
        Self {
            data: std::ptr::null_mut(),
            len: 0,
            release: release_bytes,
        }
    }

    pub fn from_string(value: String) -> Self {
        Self::from_vec(value.into_bytes())
    }

    #[doc(hidden)]
    pub fn from_vec(value: Vec<u8>) -> Self {
        let bytes = value.into_boxed_slice();
        let len = bytes.len();
        if len == 0 {
            return Self::empty();
        }
        Self {
            data: Box::into_raw(bytes).cast(),
            len,
            release: release_bytes,
        }
    }

    /// # Safety
    ///
    /// This descriptor must still own the allocation supplied by its producer.
    #[doc(hidden)]
    pub unsafe fn release(self) {
        unsafe { (self.release)(self.data, self.len) };
    }

    /// # Safety
    ///
    /// This descriptor must contain one uniquely owned UTF-8 allocation.
    #[doc(hidden)]
    pub unsafe fn into_string(self) -> String {
        String::from_utf8(unsafe { self.into_vec() })
            .unwrap_or_else(|_| "host Tokio runtime panicked".to_owned())
    }

    /// # Safety
    ///
    /// This descriptor must contain one uniquely owned allocation.
    #[doc(hidden)]
    pub unsafe fn into_vec(self) -> Vec<u8> {
        let bytes = if self.len == 0 {
            Vec::new()
        } else {
            unsafe { slice::from_raw_parts(self.data, self.len) }.to_vec()
        };
        unsafe { self.release() };
        bytes
    }
}

unsafe extern "C" fn release_bytes(data: *mut u8, len: usize) {
    if !data.is_null() {
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, len)) });
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Bytes {
    data: *const u8,
    len: usize,
}

unsafe impl Send for Bytes {}
unsafe impl Sync for Bytes {}

impl Bytes {
    /// # Safety
    ///
    /// The borrowed string must remain valid until the host build call returns.
    #[doc(hidden)]
    pub unsafe fn borrow(value: Option<&str>) -> Self {
        value.map_or(
            Self {
                data: std::ptr::null(),
                len: 0,
            },
            |value| Self {
                data: value.as_ptr(),
                len: value.len(),
            },
        )
    }

    /// # Safety
    ///
    /// The bytes must remain readable and contain UTF-8 for the returned borrow.
    pub unsafe fn as_str(&self) -> &str {
        if self.len == 0 {
            return "";
        }
        unsafe { str::from_utf8_unchecked(slice::from_raw_parts(self.data, self.len)) }
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}
