fn main() {
    let source = telekio_build::prepare_tokio_host().unwrap();
    println!(
        "cargo::rustc-env=TELEKIO_TOKIO_SOURCE={}",
        source.join("src/lib.rs").display()
    );
}
