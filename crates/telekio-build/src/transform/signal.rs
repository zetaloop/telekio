use std::{error::Error, path::Path};

use r#override::{item, root};

use super::{mount, patch};

pub(super) fn patch_signal(generated: &Path) -> Result<(), Box<dyn Error>> {
    patch(&generated.join("src/runtime/driver.rs"), |source| {
        mount(source, None, "telekio", "guest/runtime/driver.rs")
    })?;
    let unused_without_process = "#[cfg_attr(all(unix, not(test), feature = \"rt\", feature = \"signal\", not(feature = \"process\")), expect(dead_code))]";
    patch(&generated.join("src/runtime/signal/mod.rs"), |source| {
        for target in [item("Handle"), item("Handle::check_inner")] {
            source
                .select(target)?
                .add_attribute(unused_without_process)?;
        }
        Ok(())
    })?;
    patch(&generated.join("src/signal/registry.rs"), |source| {
        for target in [
            item("EventId"),
            item("Storage"),
            root()
                .implementation("Registry<S>")
                .has(item("register_listener")),
            root()
                .implementation("Globals")
                .has(item("register_listener")),
        ] {
            source
                .select(target)?
                .add_attribute(unused_without_process)?;
        }
        Ok(())
    })?;
    patch(&generated.join("src/signal/mod.rs"), |source| {
        for target in [
            item("RxFuture"),
            item("make_future"),
            root().implementation("RxFuture").has(item("new")),
            item("reusable_box"),
        ] {
            source.select(target)?.add_attribute("#[cfg_attr(all(not(test), feature = \"rt\", feature = \"signal\"), expect(dead_code))]")?;
        }
        Ok(())
    })?;
    patch(&generated.join("src/signal/unix.rs"), |source| {
        let unused = "#[cfg_attr(all(not(test), feature = \"rt\", feature = \"signal\", not(feature = \"process\")), expect(dead_code))]";
        for target in [
            item("OsExtraData"),
            item("SignalInfo"),
            root().implementation("OsStorage").has(item("get")),
            item("action"),
            item("signal_enable"),
            item("signal_with_handle"),
        ] {
            source.select(target)?.add_attribute(unused)?;
        }
        source
            .select(root().import("RxFuture"))?
            .redirect("self::telekio::RxFuture")?;
        source
            .select(root().child(item("signal")).call("signal"))?
            .redirect("telekio_signal")?;
        source
            .select(root().child(item("signal")).call("signal_with_handle"))?
            .redirect("telekio::signal")?;
        mount(source, Some("pub(crate)"), "telekio", "guest/signal.rs")
    })?;
    patch(&generated.join("src/process/unix/mod.rs"), |source| {
        source
            .select(item("GlobalOrphanQueue::push_orphan").call("push_orphan"))?
            .redirect("reap_host_orphan")?;
        mount(source, None, "telekio", "guest/process/unix/mod.rs")
    })?;
    patch(&generated.join("src/signal/windows.rs"), |source| {
        for condition in ["cfg(windows)", "cfg(not(windows))"] {
            source
                .select(item("imp").has(root().attribute(condition)))?
                .add_attribute("#[cfg_attr(not(test), expect(dead_code))]")?;
        }
        source
            .select(root().import("RxFuture"))?
            .redirect("self::telekio::RxFuture")?;
        for name in [
            "ctrl_c",
            "ctrl_break",
            "ctrl_close",
            "ctrl_logoff",
            "ctrl_shutdown",
        ] {
            source
                .select(item(name).call(&format!("self::imp::{name}")))?
                .redirect(&format!("telekio::{name}"))?;
        }
        mount(source, None, "telekio", "guest/signal.rs")
    })
}

pub(super) fn host(source: &Path) -> Result<(), Box<dyn Error>> {
    patch(&source.join("src/runtime/process.rs"), |contents| {
        for name in ["park", "park_timeout"] {
            contents
                .select(
                    root()
                        .implementation("Driver")
                        .item(name)
                        .call("GlobalOrphanQueue::reap_orphans"),
                )?
                .redirect("telekio::reap_orphans")?;
        }
        Ok(())
    })
}
