# Telekio

Tokio across independently compiled native Rust libraries.

Telekio lets Rust plugins use their host's Tokio runtime. Plugins use ordinary `tokio::*` APIs and Tokio-based libraries, with scheduling, timers, and I/O driven by the host.

## Use with a Tokio application

Install the Cargo wrapper:

```sh
cargo install telekio-cli
```

An application can keep its usual Tokio dependency and code:

```toml
[dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread", "time"] }
```

```rust
use std::time::Duration;

#[tokio::main]
async fn main() {
    let answer = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        42
    })
    .await
    .unwrap();

    println!("{answer}");
}
```

Run Cargo through Telekio:

```sh
telekio cargo run
telekio cargo test
telekio cargo clippy
```

The wrapper applies a Cargo patch to Tokio, including transitive dependencies. The generated implementation forwards runtime operations to the host through a C ABI.

For a persistent source-project patch:

```sh
telekio init
cargo run
```

Commit the generated `.telekio` directory together with the manifest change. Use `telekio remove` to remove the persistent patch.

If Cargo still selects its previous Tokio dependency, run `telekio cargo update -p tokio`, or `cargo update -p tokio` after `telekio init`.

## Host and plugin

Each plugin is attached to an Owner on the host. The resulting Attachment provides the Tokio runtime context for calls into the plugin.

This example uses two packages, `plugin` and `host`. The plugin queries its current runtime, and the host checks that both see the same worker count.

### Plugin

`plugin/Cargo.toml`:

```toml
[package]
name = "plugin"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
telekio = "0.1"
tokio = { version = "1", features = ["rt"] }
```

`plugin/src/lib.rs`:

```rust
telekio::plugin!();

#[unsafe(no_mangle)]
pub extern "C" fn workers() -> usize {
    tokio::runtime::Handle::current().metrics().num_workers()
}
```

Build the plugin with its guest patch:

```sh
telekio cargo build --manifest-path plugin/Cargo.toml
```

### Host

`host/Cargo.toml`:

```toml
[package]
name = "host"
version = "0.1.0"
edition = "2024"

[dependencies]
libloading = "0.9"
telekio-host = "0.1"
```

`host/src/main.rs`:

```rust
use std::{env, error::Error};

use libloading::Library;
use telekio_host::{Attach, Runtime};

fn main() -> Result<(), Box<dyn Error>> {
    let path = env::args_os().nth(1).expect("expected a plugin library path");
    let library = unsafe { Library::new(path)? };
    let attach = unsafe { *library.get::<Attach>(b"telekio_attach")? };
    let workers = unsafe { *library.get::<unsafe extern "C" fn() -> usize>(b"workers")? };

    let runtime = Runtime::new()?;
    let owner = runtime.owner();
    let mut attachment = unsafe { owner.attach(attach)? };

    let count = attachment.enter(|| unsafe { workers() });
    assert_eq!(count, runtime.tokio().handle().metrics().num_workers());
    println!("Plugin is attached to the {count}-worker host runtime");

    runtime.tokio().block_on(owner.detach(&mut attachment))?;
    drop(library);
    Ok(())
}
```

The host above uses the backend directly and builds with ordinary Cargo. On Linux:

```sh
cargo run --manifest-path host/Cargo.toml -- plugin/target/debug/libplugin.so
```

Use `plugin/target/debug/plugin.dll` on Windows or `plugin/target/debug/libplugin.dylib` on macOS. Workspace builds place artifacts in the workspace target directory instead.

### Async calls and shutdown

For an async plugin interface, the interface adapter enters the Attachment around plugin calls, future polling, and future destruction. Business code can then use Tokio normally.

To unload a plugin, release its returned futures and other callable objects, await `owner.detach(&mut attachment)` from outside the plugin call, then unload the library. Detachment stops the plugin's Tokio work and waits for active calls to finish.

## Workspaces

Package dependencies determine the role used by the wrapper:

| Dependencies | Role |
| --- | --- |
| `telekio-host` | Host |
| `telekio` without `telekio-host` | Guest |
| Neither | Host |

The wrapper reads normal direct dependencies, including aliases named `telekio-host`.

```sh
telekio cargo build --workspace
telekio cargo test -p plugin
telekio cargo +stable clippy --workspace --all-targets -- -D warnings
```

Workspace commands build host and guest packages in groups, sharing the workspace lockfile and target directory. Use `telekio cargo` for workspaces containing both roles, and `-p` to select a package for individual commands such as `run`.

## Publishing and installation

Cargo strips a root patch and excludes the nested Tokio package when publishing an application. Installing that published application through ordinary `cargo install` consequently uses official Tokio. Installing through Telekio supplies the patch during compilation:

```sh
telekio cargo install my-app
```

An application can display a notice when built with ordinary Tokio by adding the ABI crate under the `telekio-host` alias:

```toml
[dependencies]
telekio-host = { package = "telekio", version = "0.1" }
```

```rust
telekio_host::require!();
```

Call it from the application's entry point. It prints the Telekio installation instructions once when running with ordinary Tokio. Patched builds stay silent.

## Project layout

| Crate | Responsibility |
| --- | --- |
| `telekio` | Dependency-free ABI descriptors, artifact-side adapters, and attachment entry |
| `telekio-host` | Runtime services and per-plugin ownership |
| `telekio-tokio` | Native Tokio backend, with Rust crate name `tokio` |
| `telekio-build` | Upstream source preparation, structural transformations, and sparse patch generation |
| `telekio-cli` | Cargo wrapper and persistent project patches; executable name `telekio` |

## License

MIT. Generated Tokio source retains its upstream license.
