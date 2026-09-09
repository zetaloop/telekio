# Development

[README.md](README.md) describes public usage. Build and validation recipes live in [.github/workflows/check.yml](.github/workflows/check.yml).

## Execution and ownership

Host Tokio owns the runtime services, scheduler, and real task identities. Guest `UnownedTask` retains the artifact-local future, output, panic payload, and join state required by Rust's types. `LocalSet` retains Tokio's local scheduler. Shared `ExecutionState` carries the current task identity, cooperative budget, and RNG across artifact calls.

Guest `Runtime` and `LocalRuntime` own their `telekio_abi::Runtime` through `BlockingPool`, preserving Tokio's shutdown sequence. Handles share only `Connection` state. Organize task, scheduler, hooks, metrics, dump, I/O, and time code around their corresponding Tokio services.

`Attachment` retains a native Handle and internal per-plugin ownership state; the application owns its Runtime. Detachment closes plugin work, drains active calls, and reclaims host references abandoned by guest destructors. Child-runtime shutdown uses the detach caller's blocking pool. Resources can outlive their creating Runtime; incoming callback ownership must also be released when registration fails.

Source locations outlive tasks because Tokio exposes them as `&'static Location`. The host retains foreign locations at task creation; hooks read the location already associated with the task. The representation adapters in `telekio-host/src/bridge/runtime/location.rs` and `telekio-abi/src/runtime/location.rs` depend on the local standard library's private layout and require review when changing the supported toolchain.

## Compilation contexts

`telekio-abi` defines the native ABI and artifact-side adapters. Rust-owned values and callback destruction stay in their defining artifact, with panics reported through ABI results.

`telekio-host` compiles native Tokio and its host bridge in one crate. The bridge implementation lives in `src/bridge`; the crate root follows Tokio's module layout. Generated host patches re-export its Tokio API. The workspace release version and the upstream Tokio source version selected in `telekio-build/src/source.rs` are separate.

`telekio-build/src/transform` contains generation-time code. Files under `telekio-build/tokio/{guest,host,shared}` compile inside Tokio, so their `crate::` paths refer to Tokio. Mounted paths follow the upstream module being extended.

`telekio` owns user manifests, package-role selection, and Cargo orchestration. `telekio cargo` prepares the dependency graph and lockfile through Cargo metadata before executing the original user command. This preparation is independent of the final command's locking and offline flags.

## Upstream integration

Cargo supplies the fixed upstream source. Changes belong in the transformation program and mounted helpers; generated trees are regenerated from those inputs.

Preserve upstream control flow through symbol- and syntax-scoped edits from `telekio-build/src/edit.rs`. Singular edits require one target across direct syntax and nested macros. Discuss additions to the edit API before extending it. Exact `#[expect(dead_code)]` annotations identify upstream execution symbols displaced by a transformation.

Cargo root patches select the generated Tokio package. Publication strips application-root patches, so source installation uses the CLI to supply the same patch.

## Validation

`tests/upstream` generates Tokio's original suite through `prepare_tests()`. These tests cover the transformed Tokio API; `tests/host` and `tests/plugin` exercise independently compiled artifacts and their invocation, cancellation, detach, and unload lifecycle. Both forms of validation are needed for cross-artifact behavior.

Run the complete upstream suites. Compare failures with unmodified Tokio under the same configuration and preserve upstream-equivalent behavior. Native systems provide execution evidence; other targets receive source review and cross-compilation.

The CI recipes include the separate rustfmt pass for mounted Tokio source, feature combinations, stable and unstable tests, doctests, and package verification. Taskdump requires the corresponding guest and host features.
