use std::{error::Error, path::Path};

use r#override::{item, root};

use super::{mount, patch};

pub(super) fn patch_io(generated: &Path) -> Result<(), Box<dyn Error>> {
    patch(&generated.join("src/io/interest.rs"), |source| {
        for name in ["is_aio", "is_lio"] {
            source
                .select(root().implementation("Interest").item(name))?
                .set_visibility("pub(crate)")?;
        }
        Ok(())
    })?;
    patch(&generated.join("src/io/bsd/poll_aio.rs"), |source| {
        mount(source, None, "telekio", "guest/io/bsd/poll_aio.rs")
    })?;
    patch(&generated.join("src/runtime/io/mod.rs"), |source| {
        mount(
            source,
            Some("pub(crate)"),
            "telekio",
            "guest/runtime/io/mod.rs",
        )
    })?;
    patch(&generated.join("src/runtime/io/driver.rs"), |source| {
        source
            .select(item("Handle"))?
            .add_field("telekio_uring: super::telekio::Uring")?;
        source
            .select(item("Driver::new").record("Handle"))?
            .add_field("telekio_uring: super::telekio::Uring::new()")
    })?;
    patch(
        &generated.join("src/runtime/io/driver/uring.rs"),
        |source| {
            source
                .select(item("Handle::try_init").call("add_uring_source"))?
                .redirect("add_uring_source_host")?;
            source
                .select(item("Handle::add_uring_source"))?
                .add_attribute("#[expect(dead_code)]")?;
            mount(source, None, "telekio", "guest/runtime/io/driver/uring.rs")
        },
    )?;
    patch(
        &generated.join("src/runtime/io/registration.rs"),
        |source| {
            for (name, replacement) in [
                ("new_with_interest_and_handle", "register_local"),
                ("deregister", "deregister_local"),
                ("clear_readiness", "clear_local_readiness"),
                ("poll_read_ready", "poll_local_read_ready"),
                ("poll_write_ready", "poll_local_write_ready"),
                ("poll_ready", "poll_local_ready"),
                ("readiness", "local_readiness"),
                ("try_io", "local_try_io"),
            ] {
                source
                    .select(root().implementation("Registration").item(name))?
                    .rename(replacement)?;
                source
                    .select(root().implementation("Registration").item(replacement))?
                    .add_attribute("#[cfg_attr(feature = \"rt\", expect(dead_code))]")?;
            }
            mount(source, None, "telekio", "guest/runtime/io/registration.rs")
        },
    )?;
    patch(
        &generated.join("src/runtime/io/scheduled_io.rs"),
        |source| {
            source
                .select(item("ScheduledIo"))?
                .add_field("telekio: std::sync::Mutex<telekio::State>")?;
            source
                .select(item("ScheduledIo::default").record("ScheduledIo"))?
                .add_field("telekio: std::sync::Mutex::new(telekio::State::default())")?;
            mount(source, None, "telekio", "guest/runtime/io/scheduled_io.rs")
        },
    )?;
    patch(&generated.join("src/io/poll_evented.rs"), |source| {
        source
            .select(
                root()
                    .implementation("PollEvented<E>")
                    .has(item("new_with_interest_and_handle"))
                    .generic("E"),
            )?
            .set_bounds("Source + crate::runtime::io::telekio::Source")
    })?;
    patch(&generated.join("src/io/async_fd.rs"), |source| {
        source
            .select(root().import("SourceFd"))?
            .redirect("self::telekio::SourceFd")?;
        mount(source, None, "telekio", "guest/io/async_fd.rs")
    })?;
    patch(&generated.join("src/net/windows/named_pipe.rs"), |source| {
        source
            .select(
                item("NamedPipeServer::connect")
                    .call("async_io")
                    .child(root().closure())
                    .call("connect"),
            )?
            .redirect("connect_host")?;
        source
            .select(item("NamedPipeServer::connect").call("connect"))?
            .redirect("connect_host")?;
        source
            .select(item("NamedPipeServer::disconnect").call("disconnect"))?
            .redirect("disconnect_host")?;
        for owner in ["NamedPipeServer", "NamedPipeClient"] {
            for (method, replacement) in [
                ("poll_read", "poll_read_host"),
                ("poll_write", "poll_write_host"),
                ("poll_write_vectored", "poll_write_vectored_host"),
            ] {
                source
                    .select(root().implementation(owner).item(method).call(method))?
                    .redirect(replacement)?;
            }
        }
        for owner in ["NamedPipeServer", "NamedPipeClient"] {
            for (method, kind, data) in [
                ("try_read", "Read", "buf.as_mut_ptr()"),
                ("try_write", "Write", "buf.as_ptr().cast_mut()"),
            ] {
                source
                    .select(
                        root()
                            .implementation(owner)
                            .item(method)
                            .call("try_io")
                            .child(root().closure()),
                    )?
                    .delegate(
                        "crate::runtime::io::telekio::delegate",
                        &[
                            "self.io.registration()",
                            &format!("::telekio_abi::IoOperationKind::{kind}"),
                            data,
                            "buf.len()",
                        ],
                    )?;
            }
            for (method, helper, data, len) in [
                (
                    "try_read_vectored",
                    "crate::runtime::io::telekio::delegate_read_vectored",
                    "bufs.as_mut_ptr().cast()",
                    "bufs.len()",
                ),
                (
                    "try_write_vectored",
                    "crate::runtime::io::telekio::delegate_write_vectored",
                    "buf.as_ptr().cast()",
                    "buf.len()",
                ),
            ] {
                source
                    .select(
                        root()
                            .implementation(owner)
                            .item(method)
                            .call("try_io")
                            .child(root().closure()),
                    )?
                    .delegate(helper, &["self.io.registration()", data, len])?;
            }
            source
                .select(
                    root()
                        .implementation(owner)
                        .item("try_read_buf")
                        .call("try_io")
                        .child(root().closure()),
                )?
                .delegate(
                    "crate::runtime::io::telekio::delegate_read_buf",
                    &["self.io.registration()", "std::ptr::from_mut(buf)"],
                )?;
        }
        mount(source, None, "telekio", "guest/net/windows/named_pipe.rs")
    })
}
