use std::{panic::Location, ptr};

use super::SourceLocation;

const _: () = {
    assert!(std::mem::size_of::<SourceLocation>() == std::mem::size_of::<Location<'static>>());
    assert!(std::mem::align_of::<SourceLocation>() == std::mem::align_of::<Location<'static>>());
};

pub(super) fn borrow(location: &'static Location<'static>) -> *const SourceLocation {
    let source = ptr::from_ref(location).cast::<SourceLocation>();
    validate(unsafe { &*source }, location);
    source
}

/// # Safety
///
/// The record and its NUL-terminated filename must remain valid indefinitely.
pub(super) unsafe fn resolve(source: *const SourceLocation) -> &'static Location<'static> {
    let location = unsafe { &*source.cast::<Location<'static>>() };
    validate(unsafe { &*source }, location);
    location
}

fn validate(source: &SourceLocation, location: &Location<'_>) {
    let file = unsafe { source.file() };
    assert_eq!(location.file(), file);
    assert_eq!(location.file_as_c_str().to_bytes(), file.as_bytes());
    assert_eq!(location.line(), source.line());
    assert_eq!(location.column(), source.column());
}
