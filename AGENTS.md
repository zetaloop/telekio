# Development

Read [README.md](README.md) for the public integration model. Workspace package metadata is defined in `Cargo.toml`; the fixed upstream Tokio version is selected in `crates/telekio-build/src/source.rs`.

## Architecture

Host Tokio executes tasks and provides runtime services. Guest `UnownedTask` holds artifact-local futures, outputs, panic payloads, and join state; `LocalSet` uses Tokio's local scheduler. Both artifacts access the current task's identity, cooperative budget, and RNG through `ExecutionState`.

Guest `Runtime` and `LocalRuntime` own their `telekio::Runtime` through `BlockingPool`. Cloned Handles share a `Connection`. Keep runtime shutdown with the owning runtime and service access with the connection.

Organize services by their Tokio modules: task, scheduler, hooks, metrics, dump, I/O, and time. `Owner` and `Attachment` handle plugin lifetime separately. Project manifests and Cargo command handling belong to `telekio-cli`.

## Source transformations

`telekio-build/src/transform` runs during generation. `telekio-build/tokio/{guest,host,shared}` is source compiled inside Tokio: its `crate::` paths refer to Tokio. Mounted paths follow the upstream module being extended.

Cargo provides upstream source. Apply changes through the transformation program and mounted helpers, then regenerate the complete tree.

Use symbol- and syntax-scoped edits from `src/edit.rs` and `src/edit/call.rs`. A singular edit must match exactly one target across the source and its macros. Implement replacement behavior in mounted helpers; discuss additions to the edit API before extending it.

Exact `#[expect(dead_code)]` annotations identify upstream symbols made unreachable by a transformation.

## ABI and ownership

Raw descriptors use private fields and unsafe constructors. Adapters own their release; rejected operations release incoming references. Callbacks and destruction can reenter or panic, so invoke them outside state locks and report panics through ABI results.

Resources may outlive the runtime wrapper that created them. Detach also reclaims descriptors whose guest destructors never ran. `LocalRuntime` stays on its creating thread through destruction.

`telekio-host/src/runtime/location.rs` constructs host-owned source locations; `telekio/src/runtime/location.rs` adapts their representation for hook calls. Both mirror the local standard library's private `Location` layout; review that layout when changing the supported Rust toolchain.

## Checks

Run from the workspace root:

```sh
cargo fmt
rustfmt --edition 2021 $(git ls-files crates/telekio-build/tokio)
cargo clippy --workspace --all-targets --features guest,full --fix --allow-dirty -- -D warnings
cargo build --workspace
cargo doc --workspace --no-deps
```

Mounted source needs the separate rustfmt invocation because Cargo does not discover it. Use Tokio's edition for those files.

### Behavior

The `tests` workspace generates the complete upstream Tokio tree through `telekio_build::prepare_tests()`. Use cargo-nextest for test targets and Cargo for doctests:

```sh
export CARGO_TARGET_DIR="$PWD/target"
suite=$(cargo run --quiet --manifest-path tests/Cargo.toml -p upstream-tests)
cargo nextest run --manifest-path "$suite/Cargo.toml" --config-file .config/nextest.toml --features full,test-util,telekio-test
cargo test --manifest-path "$suite/Cargo.toml" --features full,test-util,telekio-test --doc
```

Unstable checks set both `RUSTFLAGS='--cfg tokio_unstable'` and `RUSTDOCFLAGS='--cfg tokio_unstable'`, and enable the relevant Tokio features such as `tracing`, `schedule-latency`, `taskdump`, and `io-uring`. Exercise reduced feature sets as well as `full`, including Tokio's standard-lock branch without `parking_lot`.

Run the complete upstream suite. Investigate failures with temporary reproductions and compare them against unmodified Tokio in the same environment.

The `plugin-host` executable in `tests` accepts the built `plugin` library path and exercises invocation, future polling, cancellation, detach, and unload. Build both roles with `cargo run -p telekio-cli -- cargo build --manifest-path tests/Cargo.toml --workspace`. The platform and cargo-hack feature matrices are in `.github/workflows/check.yml`.

Reuse target directories. Test on available native systems and use cross-compilation for other Tokio targets. Taskdump checks enable `taskdump` in both the guest and host.

### Packaging

Local patches let Cargo verify the workspace packages together. In Bash or Zsh:

```sh
patches=()
for package in telekio telekio-build telekio-tokio telekio-host telekio-cli; do
    patches+=(--config "patch.crates-io.$package.path=\"crates/$package\"")
done
cargo package --workspace --allow-dirty "${patches[@]}"
```

Check that packages include the mounted sources and README. Exercise application installation with ordinary Cargo and with `telekio cargo`, since publication removes the application's root patch.
