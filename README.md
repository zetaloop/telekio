# Telekio

Tokio across independently compiled native Rust libraries.

Telekio lets Rust plugins use their host's Tokio runtime. Plugins use ordinary `tokio::*` APIs and Tokio-based libraries, with scheduling, timers, and I/O driven by the host.

## Installation

Install the Cargo wrapper:

```sh
cargo binstall telekio
```

To build the wrapper from source, use `cargo install telekio`.

### Applications

For applications that provide Telekio-enabled release binaries:

```sh
cargo binstall my-app
```

For a source installation:

```sh
telekio cargo install my-app
```

## Development

Keep your application's usual Tokio dependency and code. Run Cargo through Telekio:

```sh
telekio cargo run
telekio cargo test
telekio cargo clippy
```

Telekio patches crates.io Tokio across the dependency graph and prepares `Cargo.lock` before invoking Cargo.

For a persistent source-project patch:

```sh
telekio init
cargo run
```

Commit the generated `.telekio` directory together with the manifest change. `cargo update -p tokio` refreshes the Tokio selection in an existing lockfile. Use `telekio remove` to remove the persistent patch.

## Host and plugin

Each plugin is attached to the host's current Tokio runtime. The resulting Attachment provides the runtime context for calls into the plugin.

Create a workspace containing `host` and `plugin`:

`Cargo.toml`:

```toml
[workspace]
members = ["host", "plugin"]
resolver = "3"
```

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
telekio-abi = "0.1"
tokio = { version = "1", features = ["rt"] }
```

`plugin/src/lib.rs`:

```rust
telekio_abi::plugin!();

#[unsafe(no_mangle)]
pub extern "C" fn workers() -> usize {
    tokio::runtime::Handle::current().metrics().num_workers()
}
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
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

`host/src/main.rs`:

```rust
use std::{env, error::Error};

use libloading::Library;
use telekio_host::Attach;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let path = env::args_os()
        .nth(1)
        .expect("expected a plugin library path");
    let library = unsafe { Library::new(path)? };
    let attach = unsafe { *library.get::<Attach>(b"telekio_attach")? };
    let workers = unsafe { *library.get::<unsafe extern "C" fn() -> usize>(b"workers")? };

    let mut attachment = unsafe { telekio_host::attach(attach)? };

    let count = attachment.enter(|| unsafe { workers() });
    println!("Host workers: {count}");

    attachment.detach().await?;
    library.close()?;
    Ok(())
}
```

Build and run from the workspace root. On Linux:

```sh
telekio cargo build
telekio cargo run -p host -- target/debug/libplugin.so
```

Use `target/debug/plugin.dll` on Windows or `target/debug/libplugin.dylib` on macOS.

### Async calls and shutdown

For an async plugin interface, the interface adapter enters the Attachment around plugin calls, future polling, and future destruction. Business code can then use Tokio normally.

To unload a plugin, release its returned futures and other callable objects, await `attachment.detach()` from outside the plugin call, then unload the library. Detachment stops the plugin's Tokio work and waits for active calls to finish.

In synchronous host code, select an existing runtime with Tokio's `Handle::enter()` while attaching:

```rust
let mut attachment = {
    let _guard = handle.enter();
    unsafe { telekio_host::attach(entry)? }
};
```

The application owns the Runtime and determines when it shuts down; Attachment retains a Handle.

## Workspaces

Package dependencies determine the role used by the wrapper:

| Dependencies | Role |
| --- | --- |
| `telekio-host` | Host |
| `telekio-abi` without `telekio-host` | Guest |
| Neither | Host |

The wrapper reads normal direct dependencies, including aliases named `telekio-host`.

Workspace commands build host and guest packages in groups, sharing the workspace lockfile and target directory. Select an individual package with `-p`, for example `telekio cargo test -p plugin`.

## Publishing applications

Build release binaries with Telekio and publish source packages with Cargo:

```sh
telekio cargo build --release
cargo publish
```

These binaries include the runtime integration. For source users, document `telekio cargo install my-app`; Cargo omits project patches from published packages.

Applications using Tokio directly can display a startup build notice by adding the ABI crate under the `telekio-host` alias:

```toml
[dependencies]
telekio-host = { package = "telekio-abi", version = "0.1" }
```

```rust
telekio_host::require!();
```

Call it from the application's entry point. In ordinary-Tokio builds, it prints the Telekio build instructions once.

## Project layout

| Crate | Responsibility |
| --- | --- |
| `telekio-abi` | Dependency-free ABI descriptors, artifact-side adapters, and attachment entry |
| `telekio-host` | Native Tokio runtime, cross-artifact services, and per-plugin ownership |
| `telekio-build` | Upstream source preparation, structural transformations, and sparse patch generation |
| `telekio` | Cargo wrapper and persistent project patches |

## License

MIT. Generated Tokio source retains its upstream license.
