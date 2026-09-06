fn main() {
    let source = telekio_build::prepare_tests().unwrap();
    if cfg!(feature = "std-lock") {
        // Tokio's test targets require the full feature name.
        let path = source.join("Cargo.toml");
        let mut manifest = std::fs::read_to_string(&path)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        manifest["features"]["full"]
            .as_array_mut()
            .unwrap()
            .retain(|feature| feature.as_str() != Some("parking_lot"));
        std::fs::write(path, manifest.to_string()).unwrap();
    }
    println!("cargo::rustc-env=TOKIO_TESTS={}", source.display());
}
