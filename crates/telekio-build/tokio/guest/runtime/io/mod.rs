#[cfg(windows)]
use super::Registration;

pub(crate) trait Source {
    fn telekio_resource(&mut self) -> ::telekio::IoResource;
}

pub(crate) struct Uring {
    #[cfg(all(
        tokio_unstable,
        feature = "io-uring",
        feature = "rt",
        feature = "fs",
        target_os = "linux"
    ))]
    registration: std::sync::Mutex<Option<::telekio::IoDriverRegistration>>,
}

impl Uring {
    pub(crate) const fn new() -> Self {
        Self {
            #[cfg(all(
                tokio_unstable,
                feature = "io-uring",
                feature = "rt",
                feature = "fs",
                target_os = "linux"
            ))]
            registration: std::sync::Mutex::new(None),
        }
    }

    #[cfg(all(
        tokio_unstable,
        feature = "io-uring",
        feature = "rt",
        feature = "fs",
        target_os = "linux"
    ))]
    pub(crate) fn install(
        &self,
        registration: ::telekio::IoDriverRegistration,
    ) -> std::io::Result<()> {
        let mut current = self.registration.lock().unwrap();
        if current.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Tokio io_uring driver is already registered",
            ));
        }
        *current = Some(registration);
        Ok(())
    }

    #[cfg(all(
        tokio_unstable,
        feature = "io-uring",
        feature = "rt",
        feature = "fs",
        target_os = "linux"
    ))]
    pub(crate) fn close(&mut self) {
        _ = self.registration.get_mut().unwrap().take();
    }
}

#[cfg(all(
    tokio_unstable,
    feature = "io-uring",
    feature = "rt",
    feature = "fs",
    target_os = "linux"
))]
impl Drop for Uring {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(unix)]
impl<T: std::os::fd::AsRawFd> Source for T {
    fn telekio_resource(&mut self) -> ::telekio::IoResource {
        unsafe { ::telekio::IoResource::fd(self.as_raw_fd()) }
    }
}

#[cfg(windows)]
macro_rules! socket_source {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl Source for $ty {
                fn telekio_resource(&mut self) -> ::telekio::IoResource {
                    use std::os::windows::io::AsRawSocket;
                    unsafe { ::telekio::IoResource::socket(self.as_raw_socket() as u64) }
                }
            }
        )+
    };
}

#[cfg(windows)]
socket_source!(
    mio::net::TcpListener,
    mio::net::TcpStream,
    mio::net::UdpSocket
);

#[cfg(windows)]
impl Source for mio::windows::NamedPipe {
    fn telekio_resource(&mut self) -> ::telekio::IoResource {
        use std::os::windows::io::AsRawHandle;
        unsafe { ::telekio::IoResource::handle(self.as_raw_handle() as usize as u64) }
    }
}

#[cfg(windows)]
pub(crate) fn delegate<'a>(
    registration: &'a Registration,
    kind: ::telekio::IoOperationKind,
    data: *mut u8,
    len: usize,
    guest: impl FnOnce() -> std::io::Result<usize> + 'a,
) -> impl FnOnce() -> std::io::Result<usize> + 'a {
    move || {
        drop(guest);
        operation(registration, kind, data, len)
    }
}

#[cfg(windows)]
pub(crate) fn delegate_read_vectored<'a>(
    registration: &'a Registration,
    buffers: *mut std::ffi::c_void,
    len: usize,
    guest: impl FnOnce() -> std::io::Result<usize> + 'a,
) -> impl FnOnce() -> std::io::Result<usize> + 'a {
    move || {
        drop(guest);
        let buffers = unsafe {
            std::slice::from_raw_parts_mut(buffers.cast::<std::io::IoSliceMut<'_>>(), len)
        };
        let buffer = buffers
            .iter_mut()
            .find(|buffer| !buffer.is_empty())
            .map_or(&mut [][..], |buffer| &mut **buffer);
        operation(
            registration,
            ::telekio::IoOperationKind::Read,
            buffer.as_mut_ptr(),
            buffer.len(),
        )
    }
}

#[cfg(windows)]
pub(crate) fn delegate_write_vectored<'a>(
    registration: &'a Registration,
    buffers: *const std::ffi::c_void,
    len: usize,
    guest: impl FnOnce() -> std::io::Result<usize> + 'a,
) -> impl FnOnce() -> std::io::Result<usize> + 'a {
    move || {
        drop(guest);
        let buffers =
            unsafe { std::slice::from_raw_parts(buffers.cast::<std::io::IoSlice<'_>>(), len) };
        let buffer = buffers
            .iter()
            .find(|buffer| !buffer.is_empty())
            .map_or(&[][..], |buffer| &**buffer);
        operation(
            registration,
            ::telekio::IoOperationKind::Write,
            buffer.as_ptr().cast_mut(),
            buffer.len(),
        )
    }
}

#[cfg(all(windows, feature = "io-util"))]
pub(crate) fn delegate_read_buf<'a, B: bytes::BufMut + 'a>(
    registration: &'a Registration,
    buffer: *mut B,
    guest: impl FnOnce() -> std::io::Result<usize> + 'a,
) -> impl FnOnce() -> std::io::Result<usize> + 'a {
    move || {
        drop(guest);
        let buffer = unsafe { &mut *buffer };
        let chunk = buffer.chunk_mut();
        let read = operation(
            registration,
            ::telekio::IoOperationKind::Read,
            chunk.as_mut_ptr(),
            chunk.len(),
        )?;
        unsafe { buffer.advance_mut(read) };
        Ok(read)
    }
}

#[cfg(windows)]
pub(crate) fn operation(
    registration: &Registration,
    kind: ::telekio::IoOperationKind,
    data: *mut u8,
    len: usize,
) -> std::io::Result<usize> {
    let result =
        registration.try_operate(unsafe { ::telekio::IoRequest::from_raw(kind, data, len) });
    match result.state {
        ::telekio::Poll::Pending => {
            unsafe { result.call.payload.release() };
            Err(std::io::ErrorKind::WouldBlock.into())
        }
        ::telekio::Poll::Ready => {
            result.error.into_io_result(result.call)?;
            Ok(result.value)
        }
        ::telekio::Poll::Panicked => {
            result.error.into_io_result(result.call)?;
            unreachable!()
        }
    }
}
