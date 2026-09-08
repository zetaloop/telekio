impl super::PollEvented<super::mio_windows::NamedPipe> {
    pub(super) unsafe fn poll_read_host(
        &self,
        context: &mut std::task::Context<'_>,
        buffer: &mut crate::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let data = unsafe { buffer.unfilled_mut() }.as_mut_ptr().cast();
        let len = buffer.remaining();
        let read = std::task::ready!(self.registration().poll_read_io(context, || {
            crate::runtime::io::telekio::operation(
                self.registration(),
                ::telekio_abi::IoOperationKind::Read,
                data,
                len,
            )
        }))?;
        unsafe { buffer.assume_init(read) };
        buffer.advance(read);
        std::task::Poll::Ready(Ok(()))
    }

    pub(super) fn poll_write_host(
        &self,
        context: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.registration().poll_write_io(context, || {
            crate::runtime::io::telekio::operation(
                self.registration(),
                ::telekio_abi::IoOperationKind::Write,
                buffer.as_ptr().cast_mut(),
                buffer.len(),
            )
        })
    }

    pub(super) fn poll_write_vectored_host(
        &self,
        context: &mut std::task::Context<'_>,
        buffers: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let buffer = buffers
            .iter()
            .find(|buffer| !buffer.is_empty())
            .map_or(&[][..], |buffer| &**buffer);
        self.poll_write_host(context, buffer)
    }

    pub(super) fn connect_host(&self) -> std::io::Result<()> {
        crate::runtime::io::telekio::operation(
            self.registration(),
            ::telekio_abi::IoOperationKind::Connect,
            std::ptr::null_mut(),
            0,
        )
        .map(drop)
    }

    pub(super) fn disconnect_host(&self) -> std::io::Result<()> {
        crate::runtime::io::telekio::operation(
            self.registration(),
            ::telekio_abi::IoOperationKind::Disconnect,
            std::ptr::null_mut(),
            0,
        )
        .map(drop)
    }
}
