fn main() {
    telekio_build::prepare_tokio_host(env!("CARGO_PKG_VERSION")).unwrap();
}
