fn main() {
    // When building with the `embed` feature (which enables pyo3's
    // `extension-module`), we need to add linker args so that undefined
    // Python symbols are resolved at runtime (via dlopen in loader.rs)
    // instead of at link time.
    //
    // On macOS this adds `-undefined dynamic_lookup`.
    // On Linux/WASM this is typically a no-op.
    #[cfg(feature = "embed")]
    pyo3_build_config::add_extension_module_link_args();
}
