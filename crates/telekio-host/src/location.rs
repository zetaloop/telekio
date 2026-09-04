use std::{
    collections::HashMap,
    marker::PhantomData,
    panic::Location,
    ptr::NonNull,
    sync::{Mutex, OnceLock},
};

use telekio::SourceLocation;

#[derive(Eq, Hash, PartialEq)]
struct Key {
    file: String,
    line: u32,
    column: u32,
}

#[repr(C)]
struct LocationRepr {
    filename: NonNull<str>,
    line: u32,
    column: u32,
    lifetime: PhantomData<&'static str>,
}

const _: () = {
    assert!(std::mem::size_of::<LocationRepr>() == std::mem::size_of::<Location<'static>>());
    assert!(std::mem::align_of::<LocationRepr>() == std::mem::align_of::<Location<'static>>());
};

pub(super) fn intern(source: SourceLocation) -> &'static Location<'static> {
    static LOCATIONS: OnceLock<Mutex<HashMap<Key, &'static Location<'static>>>> = OnceLock::new();

    let key = Key {
        file: unsafe { source.file() }.to_owned(),
        line: source.line(),
        column: source.column(),
    };
    let mut locations = LOCATIONS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    if let Some(location) = locations.get(&key) {
        return location;
    }
    assert!(
        !key.file.as_bytes().contains(&0),
        "Tokio source path contains NUL"
    );
    let filename = Box::leak(format!("{}\0", key.file).into_boxed_str());
    let repr = LocationRepr {
        filename: NonNull::from(&mut filename[..key.file.len()]),
        line: key.line,
        column: key.column,
        lifetime: PhantomData,
    };
    let location = Box::leak(Box::new(unsafe {
        std::mem::transmute::<LocationRepr, Location<'static>>(repr)
    }));
    assert_eq!(location.file(), key.file);
    assert_eq!(location.file_as_c_str().to_bytes(), key.file.as_bytes());
    assert_eq!(location.line(), key.line);
    assert_eq!(location.column(), key.column);
    locations.insert(key, location);
    location
}
