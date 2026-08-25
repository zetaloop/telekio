pub(crate) trait Source {
    fn telekio_resource(&self) -> ::telekio::IoResource;
}

#[cfg(unix)]
impl<T: std::os::fd::AsRawFd> Source for T {
    fn telekio_resource(&self) -> ::telekio::IoResource {
        ::telekio::IoResource::fd(self.as_raw_fd())
    }
}

#[cfg(windows)]
macro_rules! socket_source {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl Source for $ty {
                fn telekio_resource(&self) -> ::telekio::IoResource {
                    use std::os::windows::io::AsRawSocket;
                    ::telekio::IoResource::socket(self.as_raw_socket() as u64)
                }
            }
        )+
    };
}

#[cfg(windows)]
socket_source!(mio::net::TcpListener, mio::net::TcpStream, mio::net::UdpSocket);

#[cfg(windows)]
impl Source for mio::windows::NamedPipe {
    fn telekio_resource(&self) -> ::telekio::IoResource {
        use std::os::windows::io::AsRawHandle;
        ::telekio::IoResource::handle(self.as_raw_handle() as usize as u64)
    }
}
