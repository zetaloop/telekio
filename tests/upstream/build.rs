fn main() {
    let support = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../support");
    let source = telekio_build::prepare_tests(&support).unwrap();
    println!("cargo::rustc-env=TOKIO_TESTS={}", source.display());
}
