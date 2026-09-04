use std::{error::Error, path::Path};

use super::{mount, patch};
use crate::edit;

pub(super) fn patch_io(generated: &Path) -> Result<(), Box<dyn Error>> {
    patch(&generated.join("src/io/interest.rs"), |source| {
        for name in ["is_aio", "is_lio"] {
            edit::set_method_visibility(source, "Interest", name, "pub(crate)")?;
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
        edit::append_fields(
            source,
            "Handle",
            &[edit::Field {
                visibility: None,
                name: "telekio_uring",
                ty: "super::telekio::Uring",
            }],
        )?;
        edit::append_record_fields(
            source,
            edit::Scope::Method {
                owner: "Driver",
                name: "new",
            },
            "Handle",
            &[edit::FieldInit {
                name: "telekio_uring",
                value: "super::telekio::Uring::new()",
            }],
        )
    })?;
    patch(
        &generated.join("src/runtime/io/driver/uring.rs"),
        |source| {
            edit::redirect_call(
                source,
                edit::Scope::Method {
                    owner: "Handle",
                    name: "try_init",
                },
                "add_uring_source",
                "add_uring_source_host",
            )?;
            edit::add_attr(
                source,
                edit::AttrTarget::Method {
                    owner: "Handle",
                    name: "add_uring_source",
                },
                "#[expect(dead_code)]",
            )?;
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
                edit::rename_method(source, "Registration", name, replacement)?;
                edit::add_attr(
                    source,
                    edit::AttrTarget::Method {
                        owner: "Registration",
                        name: replacement,
                    },
                    "#[cfg_attr(feature = \"rt\", expect(dead_code))]",
                )?;
            }
            mount(source, None, "telekio", "guest/runtime/io/registration.rs")
        },
    )?;
    patch(
        &generated.join("src/runtime/io/scheduled_io.rs"),
        |source| {
            edit::append_fields(
                source,
                "ScheduledIo",
                &[edit::Field {
                    visibility: None,
                    name: "telekio",
                    ty: "std::sync::Mutex<telekio::State>",
                }],
            )?;
            edit::append_record_fields(
                source,
                edit::Scope::Method {
                    owner: "ScheduledIo",
                    name: "default",
                },
                "ScheduledIo",
                &[edit::FieldInit {
                    name: "telekio",
                    value: "std::sync::Mutex::new(telekio::State::default())",
                }],
            )?;
            mount(source, None, "telekio", "guest/runtime/io/scheduled_io.rs")
        },
    )?;
    patch(&generated.join("src/io/poll_evented.rs"), |source| {
        edit::set_type_parameter(
            source,
            "PollEvented<E>",
            "new_with_interest_and_handle",
            "E",
            "E: Source + crate::runtime::io::telekio::Source",
        )
    })?;
    patch(&generated.join("src/io/async_fd.rs"), |source| {
        edit::retarget_use(source, "SourceFd", "self::telekio::SourceFd")?;
        mount(source, None, "telekio", "guest/io/async_fd.rs")
    })?;
    patch(&generated.join("src/net/windows/named_pipe.rs"), |source| {
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "NamedPipeServer",
                name: "connect",
            },
            "connect",
            "connect_host",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::MethodArgument {
                owner: "NamedPipeServer",
                name: "connect",
                call: edit::Call::Method("async_io"),
            },
            "connect",
            "connect_host",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "NamedPipeServer",
                name: "disconnect",
            },
            "disconnect",
            "disconnect_host",
        )?;
        for owner in ["NamedPipeServer", "NamedPipeClient"] {
            for (method, replacement) in [
                ("poll_read", "poll_read_host"),
                ("poll_write", "poll_write_host"),
                ("poll_write_vectored", "poll_write_vectored_host"),
            ] {
                edit::redirect_call(
                    source,
                    edit::Scope::Method {
                        owner,
                        name: method,
                    },
                    method,
                    replacement,
                )?;
            }
        }
        for owner in ["NamedPipeServer", "NamedPipeClient"] {
            for (method, kind, data) in [
                ("try_read", "Read", "buf.as_mut_ptr()"),
                ("try_write", "Write", "buf.as_ptr().cast_mut()"),
            ] {
                edit::delegate_closure(
                    source,
                    edit::Scope::Method {
                        owner,
                        name: method,
                    },
                    edit::Call::Method("try_io"),
                    1,
                    "crate::runtime::io::telekio::delegate",
                    &[
                        "self.io.registration()",
                        &format!("::telekio::IoOperationKind::{kind}"),
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
                edit::delegate_closure(
                    source,
                    edit::Scope::Method {
                        owner,
                        name: method,
                    },
                    edit::Call::Method("try_io"),
                    1,
                    helper,
                    &["self.io.registration()", data, len],
                )?;
            }
            edit::delegate_closure(
                source,
                edit::Scope::Method {
                    owner,
                    name: "try_read_buf",
                },
                edit::Call::Method("try_io"),
                1,
                "crate::runtime::io::telekio::delegate_read_buf",
                &["self.io.registration()", "std::ptr::from_mut(buf)"],
            )?;
        }
        mount(source, None, "telekio", "guest/net/windows/named_pipe.rs")
    })
}
