use std::{error::Error, fs, path::Path};

use super::{mount, patch};
use crate::edit;

pub(super) fn patch_signal(generated: &Path) -> Result<(), Box<dyn Error>> {
    patch(&generated.join("src/runtime/driver.rs"), |source| {
        mount(source, None, "telekio", "guest/runtime/driver.rs")
    })?;
    let unused_without_process = "#[cfg_attr(all(unix, not(test), feature = \"rt\", feature = \"signal\", not(feature = \"process\")), expect(dead_code))]";
    patch(&generated.join("src/runtime/signal/mod.rs"), |source| {
        for target in [
            edit::AttrTarget::Struct("Handle"),
            edit::AttrTarget::Method {
                owner: "Handle",
                name: "check_inner",
            },
        ] {
            edit::add_attr(source, target, unused_without_process)?;
        }
        Ok(())
    })?;
    patch(&generated.join("src/signal/registry.rs"), |source| {
        for target in [
            edit::AttrTarget::TypeAlias("EventId"),
            edit::AttrTarget::Trait("Storage"),
            edit::AttrTarget::Impl {
                owner: "Registry<S>",
                method: "register_listener",
            },
            edit::AttrTarget::Impl {
                owner: "Globals",
                method: "register_listener",
            },
        ] {
            edit::add_attr(source, target, unused_without_process)?;
        }
        Ok(())
    })?;
    patch(&generated.join("src/signal/mod.rs"), |source| {
        for target in [
            edit::AttrTarget::Struct("RxFuture"),
            edit::AttrTarget::Function("make_future"),
            edit::AttrTarget::Impl {
                owner: "RxFuture",
                method: "new",
            },
            edit::AttrTarget::Module("reusable_box"),
        ] {
            edit::add_attr(
                source,
                target,
                "#[cfg_attr(all(not(test), feature = \"rt\", feature = \"signal\"), expect(dead_code))]",
            )?;
        }
        Ok(())
    })?;
    patch(&generated.join("src/signal/unix.rs"), |source| {
        let unused = "#[cfg_attr(all(not(test), feature = \"rt\", feature = \"signal\", not(feature = \"process\")), expect(dead_code))]";
        for target in [
            edit::AttrTarget::Struct("OsExtraData"),
            edit::AttrTarget::Struct("SignalInfo"),
            edit::AttrTarget::Impl {
                owner: "OsStorage",
                method: "get",
            },
            edit::AttrTarget::Function("action"),
            edit::AttrTarget::Function("signal_enable"),
            edit::AttrTarget::Function("signal_with_handle"),
        ] {
            edit::add_attr(source, target, unused)?;
        }
        edit::retarget_use(source, "RxFuture", "self::telekio::RxFuture")?;
        edit::redirect_call(
            source,
            edit::Scope::Function("signal"),
            "signal",
            "telekio_signal",
        )?;
        edit::redirect_call(
            source,
            edit::Scope::Function("signal"),
            "signal_with_handle",
            "telekio::signal",
        )?;
        mount(source, Some("pub(crate)"), "telekio", "guest/signal.rs")
    })?;
    patch(&generated.join("src/process/unix/orphan.rs"), |source| {
        edit::add_attr(
            source,
            edit::AttrTarget::Impl {
                owner: "OrphanQueueImpl<T>",
                method: "push_orphan",
            },
            "#[cfg_attr(all(not(test), any(telekio_host, feature = \"telekio-test\")), expect(dead_code))]",
        )
    })?;
    patch(&generated.join("src/process/unix/mod.rs"), |source| {
        edit::redirect_call(
            source,
            edit::Scope::Method {
                owner: "GlobalOrphanQueue",
                name: "push_orphan",
            },
            "push_orphan",
            "reap_host_orphan",
        )?;
        mount(source, None, "telekio", "guest/process/unix/mod.rs")
    })?;
    patch(&generated.join("src/signal/windows.rs"), |source| {
        edit::add_attr(
            source,
            edit::AttrTarget::Modules("imp"),
            "#[cfg_attr(not(test), expect(dead_code))]",
        )?;
        edit::retarget_use(source, "RxFuture", "self::telekio::RxFuture")?;
        for name in [
            "ctrl_c",
            "ctrl_break",
            "ctrl_close",
            "ctrl_logoff",
            "ctrl_shutdown",
        ] {
            edit::redirect_call(
                source,
                edit::Scope::Function(name),
                &format!("self::imp::{name}"),
                &format!("telekio::{name}"),
            )?;
        }
        mount(source, None, "telekio", "guest/signal.rs")
    })
}

pub(super) fn host(source: &Path) -> Result<(), Box<dyn Error>> {
    let target = source.join("src/runtime/process.rs");
    let mut contents = fs::read_to_string(&target)?;
    for name in ["park", "park_timeout"] {
        edit::redirect_call(
            &mut contents,
            edit::Scope::Method {
                owner: "Driver",
                name,
            },
            "GlobalOrphanQueue::reap_orphans",
            "telekio::reap_orphans",
        )?;
    }
    fs::write(target, contents)?;
    Ok(())
}
